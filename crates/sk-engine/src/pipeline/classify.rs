//! The `Classify` phase (SPEC-01 §4.4, SPEC-08 T-08-12): the `Unknown`
//! findings of the heuristics with their `FolderSummary` go to
//! `sk_llm::ClassifyRunner`, the results are applied with
//! `apply_classification`, the issues of the run go to the report and to
//! `Event::Issue`.
//!
//! - `ScanOptions::llm = Off`: nothing is built, no client exists (FR-08-01).
//! - The phase needs the data folder ([`ScanPipeline::with_data_dir`]) for
//!   the cache (`cache/llm-cache.jsonl`); without it nothing is classified.
//! - The runner always gets the scan environment (`with_env`) for
//!   `final_check` (SPEC-08 §4.12).
//! - A classifier that sends data off the machine with
//!   `confirm_cloud_each_scan = true` needs a confirmation of the preview
//!   (FR-08-05). The engine has no way to ask for it yet, so such a phase is
//!   skipped with `llm.cloud_declined` unless nothing would be sent.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sk_core::collector::CollectContext;
use sk_core::events::EventSink;
use sk_core::model::{Category, Finding, FolderSummary, IssueSeverity, LlmMode, ScanIssue, Target};
use sk_core::template::PathTemplate;
use sk_core::CancellationToken;
use sk_llm::{
    apply_classification, content_peek, no_api_key_issue, Classifier, ClassifierInfo, ClassifyItem,
    ClassifyRunner, CloudLlmKind, FallbackStore, LlmCache, LlmConfig, LlmError, LocalLlmKind,
    RunOutput, SecretStore, ANTHROPIC_PROVIDER, ISSUE_SOURCE, OLLAMA_PROVIDER,
    OPENAI_COMPAT_PROVIDER,
};
use tokio::task::JoinSet;

use super::{issue, Accumulated, Run, ScanPipeline};

/// Issue of a cloud classification that was not confirmed (SPEC-08 §4.8).
pub(crate) const CLOUD_DECLINED_KEY: &str = "llm.cloud_declined";

/// The classifier of the pipeline.
#[derive(Default)]
pub(crate) struct BuiltinLlm {
    /// Replaces the classifier of `llm.mode` ([`ScanPipeline::with_classifier`]).
    pub(crate) classifier: Option<Arc<dyn Classifier>>,
    /// Replaces `FallbackStore::for_cloud` (unit tests).
    pub(crate) secrets: Option<Arc<dyn SecretStore>>,
}

/// An `Unknown` folder to classify.
struct UnknownFolder {
    id: String,
    summary: FolderSummary,
    folder: PathBuf,
}

impl ScanPipeline {
    /// Classifies with `classifier` instead of the provider of `llm.mode`
    /// (tests: `MockClassifier`). It is used only when `ScanOptions::llm`
    /// is not `Off`; the rest of the `llm` config section still applies.
    pub fn with_classifier(mut self, classifier: Arc<dyn Classifier>) -> Self {
        self.llm.classifier = Some(classifier);
        self
    }

    /// The `Classify` phase for the mode `mode` of this scan.
    pub(super) async fn classify(
        &self,
        ctx: &CollectContext,
        mode: LlmMode,
        acc: &mut Accumulated,
        run: &Run<'_>,
    ) {
        if mode == LlmMode::Off {
            return;
        }
        let Some(data_dir) = self.games.data_dir.as_deref() else {
            return;
        };
        let mut cfg = self.config.llm.clone();
        cfg.mode = mode;
        let runner = match self.runner(&cfg, data_dir) {
            Ok(Some(runner)) => runner.with_env((*ctx.env).clone()),
            Ok(None) => return,
            Err(issue) => {
                acc.push_issue(issue, run);
                return;
            }
        };
        let targets = targets(&acc.findings, &acc.unknown_summaries);
        if targets.is_empty() {
            return;
        }
        let info = runner.info();
        let allow_peek = if info.is_local {
            cfg.allow_content_peek
        } else {
            cfg.allow_content_peek_cloud
        };
        let items = items(ctx, targets, allow_peek).await;
        if runner.needs_cloud_confirmation() && runner.preview(&items).item_count > 0 {
            acc.push_issue(cloud_declined(&info, items.len()), run);
            return;
        }
        let output = match classify_task(runner, items, &ctx.events, &ctx.cancel).await {
            Ok(output) => output,
            Err(error) => {
                acc.push_issue(issue("engine", "classify.panicked", &error), run);
                return;
            }
        };
        for issue in output.issues {
            acc.push_issue(issue, run);
        }
        apply(acc, &output.results, &info, cfg.min_confidence_to_apply);
    }

    /// The runner of `cfg` (`mode` is not `Off`): over the classifier of
    /// [`Self::with_classifier`], else `ClassifyRunner::from_config`.
    fn runner(
        &self,
        cfg: &LlmConfig,
        data_dir: &Path,
    ) -> Result<Option<ClassifyRunner>, ScanIssue> {
        if let Some(classifier) = &self.llm.classifier {
            let cache = LlmCache::open(&LlmCache::file_path(data_dir)).map_err(|e| {
                llm_issue(
                    IssueSeverity::Warning,
                    "llm.cache_error",
                    vec![("error", e.to_string())],
                )
            })?;
            return Ok(Some(ClassifyRunner::new(
                Arc::clone(classifier),
                cache,
                cfg.clone(),
            )));
        }
        let fallback;
        let secrets: &dyn SecretStore = match &self.llm.secrets {
            Some(secrets) => &**secrets,
            None => {
                fallback = FallbackStore::for_cloud(&cfg.cloud);
                &fallback
            }
        };
        match ClassifyRunner::from_config(cfg, data_dir, secrets) {
            Ok(runner) => Ok(runner),
            Err(LlmError::Auth) if cfg.mode == LlmMode::Cloud => Err(no_api_key_issue()),
            Err(error) => {
                let (provider, model) = configured_provider(cfg);
                Err(llm_issue(
                    IssueSeverity::Error,
                    "llm.unreachable",
                    vec![
                        ("provider", provider.to_owned()),
                        ("model", model.to_owned()),
                        ("error", error.to_string()),
                    ],
                ))
            }
        }
    }
}

/// `Unknown` findings of a folder (`Target::FileSet`) whose root has a
/// summary, in the order of the findings; the first summary of a path wins.
fn targets(findings: &[Finding], summaries: &[FolderSummary]) -> Vec<UnknownFolder> {
    let mut by_path: HashMap<&PathTemplate, &FolderSummary> = HashMap::new();
    for summary in summaries {
        by_path.entry(&summary.path).or_insert(summary);
    }
    findings
        .iter()
        .filter(|f| f.category == Category::Unknown)
        .filter_map(|f| match &f.target {
            Target::FileSet { root, resolved, .. } => by_path.get(root).map(|s| UnknownFolder {
                id: f.id.as_str().to_owned(),
                summary: (*s).clone(),
                folder: resolved.clone(),
            }),
            _ => None,
        })
        .collect()
}

/// The items of `targets`, with content peek (SPEC-08 §4.9) on a blocking
/// thread when `allow_peek`; a panicking peek leaves the items without it.
async fn items(
    ctx: &CollectContext,
    targets: Vec<UnknownFolder>,
    allow_peek: bool,
) -> Vec<ClassifyItem> {
    let mut items: Vec<ClassifyItem> = targets
        .iter()
        .map(|t| ClassifyItem {
            id: t.id.clone(),
            summary: t.summary.clone(),
            peek: Vec::new(),
            hint: None,
        })
        .collect();
    if !allow_peek {
        return items;
    }
    let folders: Vec<PathBuf> = targets.into_iter().map(|t| t.folder).collect();
    let scanner = Arc::clone(&ctx.scanner);
    let env = Arc::clone(&ctx.env);
    let cancel = ctx.cancel.clone();
    let task = tokio::task::spawn_blocking(move || {
        folders
            .iter()
            .map(|folder| {
                if cancel.is_cancelled() {
                    return Vec::new();
                }
                // The candidate of an `Unknown` folder is `Unknown`; files
                // with credential extensions are never read anyway.
                content_peek(&*scanner, folder, Category::Unknown, &env)
            })
            .collect::<Vec<_>>()
    });
    if let Ok(peeks) = task.await {
        for (item, peek) in items.iter_mut().zip(peeks) {
            item.peek = peek;
        }
    }
    items
}

/// `runner.run` in a task, so that a panic becomes an issue; dropping the
/// future (cancellation of the scan) aborts it.
async fn classify_task(
    runner: ClassifyRunner,
    items: Vec<ClassifyItem>,
    events: &EventSink,
    cancel: &CancellationToken,
) -> Result<RunOutput, String> {
    let (events, cancel) = (events.clone(), cancel.clone());
    let mut task = JoinSet::new();
    task.spawn(async move { runner.run(items, &events, &cancel).await });
    match task.join_next().await {
        Some(Ok(output)) => Ok(output),
        Some(Err(error)) => Err(error.to_string()),
        None => Ok(RunOutput::default()),
    }
}

/// Applies `results` to the findings by id and drops the summaries of the
/// folders that are no longer `Unknown` from `unknown_summaries`.
fn apply(
    acc: &mut Accumulated,
    results: &[sk_llm::Classification],
    info: &ClassifierInfo,
    min: f32,
) {
    let by_id: HashMap<&str, &sk_llm::Classification> =
        results.iter().map(|r| (r.id.as_str(), r)).collect();
    let mut recognized: HashSet<PathTemplate> = HashSet::new();
    for finding in &mut acc.findings {
        let Some(result) = by_id.get(finding.id.as_str()) else {
            continue;
        };
        if finding.category != Category::Unknown {
            continue;
        }
        apply_classification(finding, result, info, min);
        if finding.category != Category::Unknown {
            if let Target::FileSet { root, .. } = &finding.target {
                recognized.insert(root.clone());
            }
        }
    }
    acc.unknown_summaries
        .retain(|s| !recognized.contains(&s.path));
}

fn cloud_declined(info: &ClassifierInfo, items: usize) -> ScanIssue {
    llm_issue(
        IssueSeverity::Info,
        CLOUD_DECLINED_KEY,
        vec![
            ("provider", info.provider.to_owned()),
            ("model", info.model.clone()),
            ("count", items.to_string()),
        ],
    )
}

/// Provider and model of the config section of `cfg.mode`.
fn configured_provider(cfg: &LlmConfig) -> (&'static str, &str) {
    match cfg.mode {
        LlmMode::Cloud => match cfg.cloud.kind {
            CloudLlmKind::Anthropic => (ANTHROPIC_PROVIDER, cfg.cloud.model.as_str()),
            CloudLlmKind::OpenaiCompat => (OPENAI_COMPAT_PROVIDER, cfg.cloud.model.as_str()),
        },
        _ => match cfg.local.kind {
            LocalLlmKind::Ollama => (OLLAMA_PROVIDER, cfg.local.model.as_str()),
            LocalLlmKind::OpenaiCompat => (OPENAI_COMPAT_PROVIDER, cfg.local.model.as_str()),
        },
    }
}

fn llm_issue(severity: IssueSeverity, key: &str, args: Vec<(&str, String)>) -> ScanIssue {
    ScanIssue {
        severity,
        source: ISSUE_SOURCE.to_owned(),
        path: None,
        message_key: key.to_owned(),
        message_args: args.into_iter().map(|(k, v)| (k.to_owned(), v)).collect(),
    }
}

#[cfg(test)]
#[path = "classify_tests.rs"]
mod tests;
