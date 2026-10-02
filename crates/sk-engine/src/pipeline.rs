//! The scan pipeline (SPEC-01 §4.4).

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use sk_core::collector::{CollectContext, CollectOutput, Collector, PostCollector, PriorResults};
use sk_core::config::Config;
use sk_core::env::Environment;
use sk_core::error::{CollectorError, EngineError};
use sk_core::events::{Event, EventSink, ScanPhase};
use sk_core::fs::FsScanner;
use sk_core::model::{
    CollectorToggles, EnvironmentSnapshot, Finding, IssueSeverity, LlmMode, ScanIssue,
    ScanOptionsSnapshot, ScanReport, Totals,
};
use sk_core::path::PathSet;
use sk_core::template::PathTemplate;
use sk_core::CancellationToken;
use time::OffsetDateTime;
use tokio::task::{JoinError, JoinSet};
use uuid::Uuid;

use crate::reports;
use crate::unavailable_fs::UnavailableFs;

/// What to scan.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanOptions {
    /// Extra roots for heuristics (other drives ...).
    pub roots: Vec<PathBuf>,
    /// Collectors switched on or off.
    pub collectors: CollectorToggles,
    /// LLM mode.
    pub llm: LlmMode,
    /// Depth limit.
    pub max_depth: Option<u32>,
}

/// Runs collectors through the phases and builds a [`ScanReport`].
pub struct ScanPipeline {
    config: Arc<Config>,
    collectors: Vec<Arc<dyn Collector>>,
    post_collectors: Vec<Arc<dyn PostCollector>>,
    scanner: Arc<dyn FsScanner>,
    environment: Option<Environment>,
    scans_dir: Option<PathBuf>,
    app_version: String,
}

impl ScanPipeline {
    /// A pipeline with the built-in collectors for `config` (none yet in P0).
    pub fn new(config: Arc<Config>) -> Self {
        Self {
            config,
            collectors: Vec::new(),
            post_collectors: Vec::new(),
            scanner: Arc::new(UnavailableFs),
            environment: None,
            scans_dir: None,
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }

    /// Adds a collector.
    pub fn with_collector(mut self, collector: Arc<dyn Collector>) -> Self {
        self.collectors.push(collector);
        self
    }

    /// Adds a post-collector.
    pub fn with_post_collector(mut self, collector: Arc<dyn PostCollector>) -> Self {
        self.post_collectors.push(collector);
        self
    }

    /// Replaces the file system scanner.
    pub fn with_scanner(mut self, scanner: Arc<dyn FsScanner>) -> Self {
        self.scanner = scanner;
        self
    }

    /// Uses `env` instead of `Environment::detect()`.
    pub fn with_environment(mut self, env: Environment) -> Self {
        self.environment = Some(env);
        self
    }

    /// Saves reports to `dir` (`DataDir::scans`).
    pub fn with_scans_dir(mut self, dir: PathBuf) -> Self {
        self.scans_dir = Some(dir);
        self
    }

    /// Version written to reports.
    pub fn with_app_version(mut self, version: String) -> Self {
        self.app_version = version;
        self
    }

    /// Runs a scan. Cancelling `cancel` stops it with [`EngineError::Cancelled`].
    pub async fn run(
        &self,
        opts: ScanOptions,
        events: EventSink,
        cancel: CancellationToken,
    ) -> Result<ScanReport, EngineError> {
        let started_at = OffsetDateTime::now_utc();
        let run = Run {
            events: &events,
            cancel: &cancel,
        };

        let env = run
            .phase(ScanPhase::Environment, async {
                match &self.environment {
                    Some(env) => Ok(env.clone()),
                    None => Environment::detect(),
                }
            })
            .await??;
        let ctx = CollectContext {
            env: Arc::new(env),
            config: Arc::clone(&self.config),
            scanner: Arc::clone(&self.scanner),
            events: events.clone(),
            cancel: cancel.child_token(),
        };

        let mut acc = Accumulated::default();
        let collected = run
            .phase(ScanPhase::Collect, self.collect(&ctx, &opts.collectors))
            .await?;
        acc.extend(collected, &run);
        let post = run
            .phase(
                ScanPhase::Heuristics,
                self.post_collect(&ctx, &opts.collectors, &acc),
            )
            .await?;
        acc.extend(post, &run);

        // Filled by SPEC-03 (T-03-07), SPEC-08 and SPEC-09.
        for phase in [ScanPhase::Measure, ScanPhase::Classify, ScanPhase::Score] {
            run.phase(phase, async {}).await?;
        }

        run.phase(ScanPhase::Done, async {
            self.finish(&ctx.env, opts, acc, started_at, &run)
        })
        .await?
    }

    /// Collectors in parallel tasks; dropping the future (cancellation) aborts them.
    async fn collect(&self, ctx: &CollectContext, toggles: &CollectorToggles) -> Vec<Outcome> {
        let mut tasks = JoinSet::new();
        let mut ids = HashMap::new();
        for collector in self.collectors.iter().filter(|c| enabled(toggles, c.id())) {
            let collector = Arc::clone(collector);
            let ctx = ctx.clone();
            let id = collector.id();
            let handle = tasks.spawn(async move { collector.collect(&ctx).await });
            ids.insert(handle.id(), id);
        }
        let mut outcomes = Vec::new();
        let name = |task| ids.get(&task).copied().unwrap_or("unknown");
        while let Some(joined) = tasks.join_next_with_id().await {
            let outcome = match joined {
                Ok((task, result)) => Outcome::new(name(task), result),
                Err(error) => Outcome::from_join(name(error.id()), &error),
            };
            outcomes.extend(outcome);
        }
        outcomes
    }

    /// Post-collectors one after another, each seeing the results so far.
    async fn post_collect(
        &self,
        ctx: &CollectContext,
        toggles: &CollectorToggles,
        acc: &Accumulated,
    ) -> Vec<Outcome> {
        let mut findings = acc.findings.clone();
        let mut claimed = acc.claimed.clone();
        let mut outcomes = Vec::new();
        for collector in self
            .post_collectors
            .iter()
            .filter(|c| enabled(toggles, c.id()))
        {
            let collector = Arc::clone(collector);
            let ctx = ctx.clone();
            let id = collector.id();
            let (task_findings, task_claimed) = (findings.clone(), claimed.clone());
            // A separate task, so that a panic becomes an issue; the JoinSet
            // aborts it when the future is dropped on cancellation.
            let mut task = JoinSet::new();
            task.spawn(async move {
                let prior = PriorResults {
                    findings: &task_findings,
                    claimed: &task_claimed,
                };
                collector.collect(&ctx, &prior).await
            });
            let outcome = match task.join_next().await {
                Some(Ok(result)) => Outcome::new(id, result),
                Some(Err(error)) => Outcome::from_join(id, &error),
                None => None,
            };
            if let Some(Outcome::Output(out)) = &outcome {
                findings.extend(out.findings.iter().cloned());
                claimed.extend(out.claimed_paths.iter().cloned());
            }
            outcomes.extend(outcome);
        }
        outcomes
    }

    fn finish(
        &self,
        env: &Environment,
        opts: ScanOptions,
        acc: Accumulated,
        started_at: OffsetDateTime,
        run: &Run<'_>,
    ) -> Result<ScanReport, EngineError> {
        let Accumulated {
            mut findings,
            mut issues,
            ..
        } = acc;
        findings.sort_by(|a, b| {
            let score = |f: &Finding| f.score.as_ref().map_or(0.0, |s| s.value);
            a.category
                .cmp(&b.category)
                .then(score(b).total_cmp(&score(a)))
                .then_with(|| a.id.cmp(&b.id))
        });
        let mut report = ScanReport {
            schema_version: ScanReport::SCHEMA_VERSION,
            scan_id: Uuid::new_v4(),
            app_version: self.app_version.clone(),
            started_at,
            finished_at: OffsetDateTime::now_utc(),
            environment: EnvironmentSnapshot::from_env(env),
            options: ScanOptionsSnapshot {
                roots: opts
                    .roots
                    .iter()
                    .map(|r| PathTemplate::from_path(r, env))
                    .collect(),
                collectors: opts.collectors,
                llm: opts.llm,
                max_depth: opts.max_depth,
            },
            findings,
            unknown_summaries: Vec::new(),
            issues: Vec::new(),
            totals: Totals::default(),
        };
        if let Some(dir) = &self.scans_dir {
            if let Err(error) = reports::save(dir, &report) {
                let issue = issue("engine", "report.save_failed", &error.to_string());
                run.send(Event::Issue {
                    issue: issue.clone(),
                });
                issues.push(issue);
            }
        }
        report.issues = issues;
        Ok(report)
    }
}

fn enabled(toggles: &CollectorToggles, id: &str) -> bool {
    match id {
        "rules" => toggles.rules,
        "games" => toggles.games,
        "system" => toggles.system,
        "heuristics" => toggles.heuristics,
        _ => true,
    }
}

fn issue(source: &str, message_key: &str, error: &str) -> ScanIssue {
    ScanIssue {
        severity: IssueSeverity::Error,
        source: source.to_owned(),
        path: None,
        message_key: message_key.to_owned(),
        message_args: BTreeMap::from([("error".to_owned(), error.to_owned())]),
    }
}

/// Result of one collector.
enum Outcome {
    Output(CollectOutput),
    Failed(ScanIssue),
}

impl Outcome {
    fn new(id: &str, result: Result<CollectOutput, CollectorError>) -> Option<Self> {
        Some(match result {
            Ok(out) => Outcome::Output(out),
            Err(error) => Outcome::Failed(issue(id, "collector.failed", &error.to_string())),
        })
    }

    /// A task that panicked; a cancelled task gives nothing.
    fn from_join(id: &str, error: &JoinError) -> Option<Self> {
        error
            .is_panic()
            .then(|| Outcome::Failed(issue(id, "collector.panicked", &error.to_string())))
    }
}

#[derive(Default)]
struct Accumulated {
    findings: Vec<Finding>,
    claimed: PathSet,
    issues: Vec<ScanIssue>,
}

impl Accumulated {
    fn extend(&mut self, outcomes: Vec<Outcome>, run: &Run<'_>) {
        for outcome in outcomes {
            let (findings, claimed, issues) = match outcome {
                Outcome::Output(out) => (out.findings, out.claimed_paths, out.issues),
                Outcome::Failed(issue) => (Vec::new(), Vec::new(), vec![issue]),
            };
            if !findings.is_empty() {
                let count = u32::try_from(findings.len()).unwrap_or(u32::MAX);
                run.send(Event::FindingsAdded { count });
            }
            for issue in &issues {
                run.send(Event::Issue {
                    issue: issue.clone(),
                });
            }
            self.findings.extend(findings);
            self.claimed.extend(claimed);
            self.issues.extend(issues);
        }
    }
}

/// Events and cancellation of one run.
struct Run<'a> {
    events: &'a EventSink,
    cancel: &'a CancellationToken,
}

impl Run<'_> {
    fn send(&self, event: Event) {
        // Nobody listening is not an error.
        let _ = self.events.send(event);
    }

    /// Runs `work` as `phase`: events around it, cancellation before and during it.
    async fn phase<T>(
        &self,
        phase: ScanPhase,
        work: impl Future<Output = T>,
    ) -> Result<T, EngineError> {
        if self.cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        self.send(Event::PhaseStarted { phase });
        let start = Instant::now();
        let out = tokio::select! {
            biased;
            () = self.cancel.cancelled() => return Err(EngineError::Cancelled),
            out = work => out,
        };
        let elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.send(Event::PhaseFinished { phase, elapsed_ms });
        Ok(out)
    }
}
