//! `ClassifyRunner`: orchestrator of the Classify phase (SPEC-08 §4.1, §4.12, §5).
//!
//! Order of a run: content peek policy → dedupe by cache key → cache hits →
//! privacy check (§4.8) → selection (FR-08-06) → sort by path → batches →
//! health check → batches in parallel (`buffer_unordered`) with retries,
//! halving and re-asking → cache → progress. Errors never fail the run: they
//! become issues of [`RunOutput`].

mod exec;
mod issues;
mod plan;
mod setup;

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use futures::stream::{self, StreamExt};
use sk_core::env::Environment;
use sk_core::events::{Event, EventSink, ScanPhase};
use sk_core::model::{IssueSeverity, ScanIssue};
use sk_core::CancellationToken;

pub use exec::{BACKOFF_BASE, MAX_RETRIES, MAX_RETRY_AFTER};
pub use issues::{no_api_key_issue, ISSUE_SOURCE};
pub use plan::{
    selection_priority, MAX_BATCH_INPUT_CHARS, MAX_PRIORITY_BYTES, RECENCY_HALF_LIFE_DAYS,
};

use self::issues::{issue, FailureIssues};
use self::plan::Group;
use crate::{
    Classification, Classifier, ClassifierInfo, ClassifyItem, CostEstimate, LlmCache, LlmConfig,
    LlmError, LlmMode, ModelPricing, RequestPreview, SecretStore, UsageStats,
};

/// Lower bound of the time budget of the phase (§4.12).
pub const MIN_PHASE_BUDGET: Duration = Duration::from_secs(60);
/// Batches in a row ending with `Unreachable`/`Auth` that stop the phase (§4.12).
pub const MAX_PROVIDER_FAILURES: usize = 3;

/// Result of [`ClassifyRunner::run`]; possibly partial (cancellation, errors).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunOutput {
    /// One classification per classified item id, in the order of the input.
    pub results: Vec<Classification>,
    /// Problems of the phase (`source = "llm"`).
    pub issues: Vec<ScanIssue>,
    /// Usage of this run: the difference of [`Classifier::usage`] before and after.
    pub usage: UsageStats,
}

/// Orchestrator of the Classify phase: cache, selection, batching,
/// parallelism, retries, progress.
pub struct ClassifyRunner {
    classifier: Arc<dyn Classifier>,
    cache: Mutex<CacheSlot>,
    cfg: LlmConfig,
    env: Environment,
}

/// The cache, or why it could not be opened (then nothing is cached).
#[derive(Debug)]
struct CacheSlot {
    cache: Option<LlmCache>,
    open_error: Option<String>,
}

/// What a run sends and what it settles without the provider.
struct Plan {
    cached: Vec<Classification>,
    blocked: Vec<(Group, crate::PrivacyViolation)>,
    dropped: Vec<Group>,
    send: Vec<Group>,
}

enum Health {
    Ok,
    Skip(ScanIssue),
    Cancelled,
}

impl ClassifyRunner {
    /// The runner for `cfg`; `None` with `mode = off` (no client is created,
    /// FR-08-01). The cache is `data_dir/cache/llm-cache.jsonl`; if it cannot
    /// be read, every run reports `llm.cache_error` and caches nothing.
    ///
    /// - `local`: `kind = ollama` or `openai_compat` from `cfg.local`;
    /// - `cloud`: `kind = anthropic` with the key [`API_KEY_NAME`](crate::API_KEY_NAME)
    ///   from `secrets` (`FallbackStore::for_cloud`); no key is
    ///   [`LlmError::Auth`] (issue `llm.no_api_key`, [`no_api_key_issue`]).
    ///   `kind = openai_compat` has no endpoint in `cfg.cloud` (§4.2.2) and is
    ///   [`LlmError::Unreachable`].
    pub fn from_config(
        cfg: &LlmConfig,
        data_dir: &Path,
        secrets: &dyn SecretStore,
    ) -> Result<Option<Self>, LlmError> {
        let Some(classifier) = setup::classifier(cfg, secrets)? else {
            return Ok(None);
        };
        let slot = match LlmCache::open(&LlmCache::file_path(data_dir)) {
            Ok(cache) => CacheSlot {
                cache: Some(cache),
                open_error: None,
            },
            Err(e) => CacheSlot {
                cache: None,
                open_error: Some(e.to_string()),
            },
        };
        Ok(Some(Self::with_slot(classifier, slot, cfg.clone())))
    }

    /// A runner over any classifier (tests, `MockClassifier`).
    pub fn new(classifier: Arc<dyn Classifier>, cache: LlmCache, cfg: LlmConfig) -> Self {
        let slot = CacheSlot {
            cache: Some(cache),
            open_error: None,
        };
        Self::with_slot(classifier, slot, cfg)
    }

    fn with_slot(classifier: Arc<dyn Classifier>, cache: CacheSlot, cfg: LlmConfig) -> Self {
        // Without the scan environment the check still catches paths,
        // emails and tokens; names shorter than 2 characters are ignored.
        let mut env = Environment::fake(Path::new(""));
        env.user_name.clear();
        env.machine_name.clear();
        Self {
            classifier,
            cache: Mutex::new(cache),
            cfg,
            env,
        }
    }

    /// The environment of the scan, whose user and machine names
    /// [`final_check`](crate::final_check) looks for before sending (§4.8).
    #[must_use]
    pub fn with_env(mut self, env: Environment) -> Self {
        self.env = env;
        self
    }

    /// Provider, model and limits of the classifier.
    pub fn info(&self) -> ClassifierInfo {
        self.classifier.info()
    }

    /// The classifier sends data off the machine (`!info().is_local`, also
    /// an `openai_compat` with an external URL) and `confirm_cloud_each_scan`
    /// is set: the UI shows [`preview`](Self::preview) first (FR-08-05).
    pub fn needs_cloud_confirmation(&self) -> bool {
        !self.info().is_local && self.cfg.confirm_cloud_each_scan
    }

    /// What [`run`](Self::run) would send for `items` (§4.8): deduplicated,
    /// without cache hits, selected and sorted as in a run, with the cost
    /// estimate of that many items. No privacy check, no request.
    pub fn preview(&self, items: &[ClassifyItem]) -> RequestPreview {
        let plan = self.plan(items.to_vec(), false);
        let send: Vec<ClassifyItem> = plan.send.into_iter().map(|g| g.item).collect();
        let cost = self.estimate_cost(send.len()).map(|e| e.usd);
        RequestPreview::new(&self.info(), &send, cost)
    }

    /// Cost estimate of classifying `n_items` items; `None` for local
    /// providers and models without a known price.
    pub fn estimate_cost(&self, n_items: usize) -> Option<CostEstimate> {
        let info = self.info();
        ModelPricing::for_model(info.provider, &info.model).map(|p| CostEstimate::new(&p, n_items))
    }

    /// Classifies `items`. Never fails: problems become issues, and on
    /// cancellation or a stopped phase the output is partial. Results of
    /// finished batches are in the cache.
    pub async fn run(
        &self,
        items: Vec<ClassifyItem>,
        events: &EventSink,
        cancel: &CancellationToken,
    ) -> RunOutput {
        let started = Instant::now();
        let info = self.info();
        let usage_before = self.classifier.usage();
        let total = items.len();
        let mut order: HashMap<String, usize> = HashMap::new();
        for (i, item) in items.iter().enumerate() {
            order.entry(item.id.clone()).or_insert(i);
        }
        let mut out = RunOutput::default();
        if let Some(error) = self.lock_cache().open_error.clone() {
            out.issues.push(issues::cache_error(&error));
        }
        let plan = self.plan(items, true);
        let mut done = plan.cached.len();
        out.results.extend(plan.cached);
        for (g, violation) in &plan.blocked {
            done += g.ids.len();
            out.issues.push(issue(
                IssueSeverity::Warning,
                "llm.privacy_blocked",
                Some(g.item.summary.path.as_str().to_owned()),
                vec![
                    ("reason", violation.to_string()),
                    ("count", g.ids.len().to_string()),
                ],
            ));
        }
        if !plan.dropped.is_empty() {
            let skipped: usize = plan.dropped.iter().map(|g| g.ids.len()).sum();
            done += skipped;
            out.issues.push(issue(
                IssueSeverity::Info,
                "llm.limit_reached",
                None,
                vec![
                    ("limit", self.cfg.max_items_per_scan.to_string()),
                    ("skipped", skipped.to_string()),
                ],
            ));
        }
        progress(events, done, total, None);
        let sent = plan.send.len();
        if !plan.send.is_empty() {
            match self.check_health(&info, cancel).await {
                Health::Ok => {
                    let batches = plan::batches(plan.send, info.max_batch);
                    self.classify_batches(batches, &info, events, cancel, done, total, &mut out)
                        .await;
                }
                Health::Skip(issue) => out.issues.push(issue),
                Health::Cancelled => {}
            }
        }
        out.results
            .sort_by_key(|r| order.get(&r.id).copied().unwrap_or(usize::MAX));
        out.usage = usage_diff(&self.classifier.usage(), &usage_before);
        tracing::info!(
            provider = info.provider,
            model = %info.model,
            items = total,
            sent,
            results = out.results.len(),
            elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            requests = out.usage.requests,
            input_tokens = out.usage.input_tokens,
            output_tokens = out.usage.output_tokens,
            cost_usd = out.usage.cost_usd,
            "llm classify finished"
        );
        out
    }

    #[allow(clippy::too_many_arguments)]
    async fn classify_batches(
        &self,
        batches: Vec<Vec<Group>>,
        info: &ClassifierInfo,
        events: &EventSink,
        cancel: &CancellationToken,
        mut done: usize,
        total: usize,
        out: &mut RunOutput,
    ) {
        let concurrency = info.max_concurrency.max(1);
        let budget = self.phase_budget(batches.len(), concurrency);
        let classifier: &dyn Classifier = &*self.classifier;
        let mut stream = stream::iter(batches.into_iter().map(|groups| async move {
            let items = groups.iter().map(|g| g.item.clone()).collect();
            let outcome = exec::run_batch(classifier, items, cancel).await;
            (groups, outcome)
        }))
        .buffer_unordered(concurrency);
        let deadline = tokio::time::sleep(budget);
        tokio::pin!(deadline);
        let mut failures = FailureIssues::default();
        let mut provider_failures = 0usize;
        let mut cache_failed = false;
        loop {
            let next = tokio::select! {
                biased;
                () = cancel.cancelled() => break,
                () = &mut deadline => {
                    out.issues.push(issue(
                        IssueSeverity::Warning,
                        "llm.phase_timeout",
                        None,
                        vec![("budget_s", budget.as_secs().to_string())],
                    ));
                    break;
                }
                next = stream.next() => next,
            };
            let Some((groups, outcome)) = next else { break };
            let by_id: HashMap<&str, &Group> =
                groups.iter().map(|g| (g.item.id.as_str(), g)).collect();
            let mut current = None;
            {
                let mut slot = self.lock_cache();
                for r in &outcome.results {
                    let Some(g) = by_id.get(r.id.as_str()) else {
                        continue;
                    };
                    if let Some(cache) = slot.cache.as_mut() {
                        cache.insert(g.key.clone(), info, r);
                    }
                    if current.is_none() {
                        current = Some(
                            r.app_guess
                                .clone()
                                .unwrap_or_else(|| g.item.summary.path.as_str().to_owned()),
                        );
                    }
                    out.results.extend(g.ids.iter().map(|id| Classification {
                        id: id.clone(),
                        ..r.clone()
                    }));
                }
                if let Some(Err(e)) = slot.cache.as_mut().map(LlmCache::flush) {
                    if !cache_failed {
                        cache_failed = true;
                        out.issues.push(issues::cache_error(&e.to_string()));
                    }
                }
            }
            for (failure, ids) in &outcome.failures {
                let count = ids
                    .iter()
                    .map(|id| by_id.get(id.as_str()).map_or(1, |g| g.ids.len()))
                    .sum();
                failures.add(failure, count);
            }
            done += groups.iter().map(|g| g.ids.len()).sum::<usize>();
            let current = current.or_else(|| {
                groups
                    .first()
                    .map(|g| g.item.summary.path.as_str().to_owned())
            });
            progress(events, done, total, current);
            if outcome.cancelled {
                break;
            }
            provider_failures = if outcome.provider_failed() {
                provider_failures + 1
            } else {
                0
            };
            if provider_failures >= MAX_PROVIDER_FAILURES {
                out.issues.push(issue(
                    IssueSeverity::Error,
                    "llm.provider_failed",
                    None,
                    vec![
                        ("provider", info.provider.to_owned()),
                        ("model", info.model.clone()),
                    ],
                ));
                break;
            }
        }
        out.issues.extend(failures.into_issues(info));
    }

    /// `max(60 s, 2 × n_batches × timeout_s / concurrency)` (§4.12); the
    /// timeout of the config section of `mode` (`local` for a runner built
    /// with `mode = off`).
    fn phase_budget(&self, n_batches: usize, concurrency: usize) -> Duration {
        let timeout_s = match self.cfg.mode {
            LlmMode::Cloud => self.cfg.cloud.timeout_s,
            _ => self.cfg.local.timeout_s,
        }
        .max(1);
        let n = u64::try_from(n_batches).unwrap_or(u64::MAX);
        let c = u64::try_from(concurrency.max(1)).unwrap_or(u64::MAX);
        let secs = n.saturating_mul(2).saturating_mul(u64::from(timeout_s)) / c;
        MIN_PHASE_BUDGET.max(Duration::from_secs(secs))
    }

    /// Health check at the start of the phase (§5).
    async fn check_health(&self, info: &ClassifierInfo, cancel: &CancellationToken) -> Health {
        let report = tokio::select! {
            biased;
            () = cancel.cancelled() => return Health::Cancelled,
            report = self.classifier.health() => report,
        };
        match report {
            Ok(r) if r.model_available => Health::Ok,
            Ok(_) | Err(LlmError::ModelNotFound(_)) => Health::Skip(issues::model_missing(info)),
            Err(LlmError::Unreachable(e)) => Health::Skip(issues::unreachable(info, &e)),
            Err(LlmError::Timeout) => Health::Skip(issues::unreachable(info, "timeout")),
            Err(LlmError::Auth) => Health::Skip(issues::auth_failed(info)),
            Err(LlmError::Cancelled) => Health::Cancelled,
            Err(e) => {
                // Transient (429, 5xx ...): the batches retry on their own.
                tracing::warn!(error = %e, "llm health check failed, classifying anyway");
                Health::Ok
            }
        }
    }

    /// Peek policy, dedupe, cache hits, privacy check (if `check_privacy`),
    /// selection and order.
    fn plan(&self, mut items: Vec<ClassifyItem>, check_privacy: bool) -> Plan {
        let info = self.info();
        let allow_peek = if info.is_local {
            self.cfg.allow_content_peek
        } else {
            self.cfg.allow_content_peek_cloud
        };
        if !allow_peek {
            for item in &mut items {
                item.peek.clear();
            }
        }
        let mut cached = Vec::new();
        let mut blocked = Vec::new();
        let mut rest = Vec::new();
        {
            let slot = self.lock_cache();
            for g in plan::group(items, &info) {
                let hit = slot.cache.as_ref().and_then(|c| c.get(&g.key, &g.ids[0]));
                if let Some(hit) = hit {
                    cached.extend(g.ids.iter().map(|id| Classification {
                        id: id.clone(),
                        ..hit.clone()
                    }));
                    continue;
                }
                if check_privacy {
                    let json = crate::item_for_prompt(&g.item).to_string();
                    if let Err(violation) = crate::final_check(&json, &self.env) {
                        blocked.push((g, violation));
                        continue;
                    }
                }
                rest.push(g);
            }
        }
        let limit = usize::try_from(self.cfg.max_items_per_scan).unwrap_or(usize::MAX);
        let (mut send, dropped) = plan::select(rest, limit, time::OffsetDateTime::now_utc());
        plan::sort(&mut send);
        Plan {
            cached,
            blocked,
            dropped,
            send,
        }
    }

    fn lock_cache(&self) -> MutexGuard<'_, CacheSlot> {
        self.cache.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn progress(events: &EventSink, done: usize, total: usize, current: Option<String>) {
    let as_u64 = |n: usize| u64::try_from(n).unwrap_or(u64::MAX);
    // A closed channel means nobody listens any more; that is not an error.
    let _ = events.send(Event::Progress {
        phase: ScanPhase::Classify,
        done: as_u64(done),
        total: Some(as_u64(total)),
        current,
    });
}

fn usage_diff(after: &UsageStats, before: &UsageStats) -> UsageStats {
    UsageStats {
        requests: after.requests.saturating_sub(before.requests),
        input_tokens: after.input_tokens.saturating_sub(before.input_tokens),
        output_tokens: after.output_tokens.saturating_sub(before.output_tokens),
        cache_creation_input_tokens: after
            .cache_creation_input_tokens
            .saturating_sub(before.cache_creation_input_tokens),
        cache_read_input_tokens: after
            .cache_read_input_tokens
            .saturating_sub(before.cache_read_input_tokens),
        cost_usd: (after.cost_usd - before.cost_usd).max(0.0),
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_errors;
