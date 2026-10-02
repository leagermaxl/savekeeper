//! Collectors: sources of findings run by the scan pipeline (SPEC-01 §4.3).

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;

use crate::config::Config;
use crate::env::Environment;
use crate::error::CollectorError;
use crate::events::EventSink;
use crate::fs::FsScanner;
use crate::model::{Finding, ScanIssue};
use crate::path::PathSet;
use crate::CancellationToken;

/// Everything a collector may use.
#[derive(Clone)]
pub struct CollectContext {
    /// Known folders, user, drives (SPEC-02 §3).
    pub env: Arc<Environment>,
    /// Configuration (SPEC-01 §4.8).
    pub config: Arc<Config>,
    /// File system access; `RealFs` or `MemFs` from `sk-scan`.
    pub scanner: Arc<dyn FsScanner>,
    /// Events and progress (SPEC-01 §4.5).
    pub events: EventSink,
    /// Cancellation of the scan.
    pub cancel: CancellationToken,
}

/// A source of findings that runs in the Collect phase: rules, games, system.
#[async_trait]
pub trait Collector: Send + Sync {
    /// Stable identifier: "rules", "games", "system".
    fn id(&self) -> &'static str;

    /// i18n key of the name shown in the UI.
    fn display_key(&self) -> &'static str;

    /// Produces findings. Problems with single items go to
    /// [`CollectOutput::issues`]; `Err` only if the collector cannot work at all.
    async fn collect(&self, ctx: &CollectContext) -> Result<CollectOutput, CollectorError>;
}

/// What a collector found.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CollectOutput {
    /// Findings (SPEC-02 §2).
    pub findings: Vec<Finding>,
    /// Paths the collector explained; heuristics do not treat them as unknown (SPEC-07).
    pub claimed_paths: Vec<PathBuf>,
    /// Problems and notes.
    pub issues: Vec<ScanIssue>,
}

/// A collector that runs after the others and sees their results: heuristics (SPEC-07).
#[async_trait]
pub trait PostCollector: Send + Sync {
    /// Stable identifier: "heuristics".
    fn id(&self) -> &'static str;

    /// Produces findings, knowing what the other collectors found.
    async fn collect(
        &self,
        ctx: &CollectContext,
        prior: &PriorResults<'_>,
    ) -> Result<CollectOutput, CollectorError>;
}

/// Results of the Collect phase passed to post-collectors.
#[derive(Debug, Clone, Copy)]
pub struct PriorResults<'a> {
    /// Findings of all collectors.
    pub findings: &'a [Finding],
    /// Claimed paths of all collectors.
    pub claimed: &'a PathSet,
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tokio::sync::mpsc::unbounded_channel;

    use super::*;
    use crate::env::KnownFolder;
    use crate::fs::{
        DirEntryInfo, EntryMeta, FsError, Readability, WalkControl, WalkOptions, WalkStats,
    };

    /// A file system with nothing in it.
    struct EmptyFs;

    impl FsScanner for EmptyFs {
        fn metadata(&self, _: &Path) -> Result<EntryMeta, FsError> {
            Err(FsError::NotFound)
        }
        fn exists(&self, _: &Path) -> bool {
            false
        }
        fn read_dir(&self, _: &Path) -> Result<Vec<DirEntryInfo>, FsError> {
            Err(FsError::NotFound)
        }
        fn walk(
            &self,
            _: &Path,
            _: &WalkOptions,
            _: &mut dyn FnMut(&DirEntryInfo) -> WalkControl,
            _: &CancellationToken,
        ) -> Result<WalkStats, FsError> {
            Err(FsError::NotFound)
        }
        fn read_head(&self, _: &Path, _: usize) -> Result<Vec<u8>, FsError> {
            Err(FsError::NotFound)
        }
        fn read_small(&self, _: &Path, _: usize) -> Result<Vec<u8>, FsError> {
            Err(FsError::NotFound)
        }
        fn probe_readable(&self, _: &Path) -> Readability {
            Readability::Missing
        }
    }

    /// Claims `{APPDATA}\Code`.
    struct ClaimingCollector;

    #[async_trait]
    impl Collector for ClaimingCollector {
        fn id(&self) -> &'static str {
            "rules"
        }
        fn display_key(&self) -> &'static str {
            "collector.rules"
        }
        async fn collect(&self, ctx: &CollectContext) -> Result<CollectOutput, CollectorError> {
            let app_data = ctx
                .env
                .known_folder(KnownFolder::AppData)
                .ok_or_else(|| CollectorError::Other("no AppData".to_owned()))?;
            Ok(CollectOutput {
                claimed_paths: vec![app_data.join("Code")],
                ..CollectOutput::default()
            })
        }
    }

    /// Reports which candidate folders are already claimed.
    struct CheckingPostCollector;

    #[async_trait]
    impl PostCollector for CheckingPostCollector {
        fn id(&self) -> &'static str {
            "heuristics"
        }
        async fn collect(
            &self,
            ctx: &CollectContext,
            prior: &PriorResults<'_>,
        ) -> Result<CollectOutput, CollectorError> {
            let app_data = ctx.env.known_folder(KnownFolder::AppData).unwrap();
            let issues = ["Code", "Code - Insiders"]
                .into_iter()
                .filter(|name| !prior.claimed.covers(&app_data.join(name).join("User")))
                .map(|name| ScanIssue {
                    severity: crate::model::IssueSeverity::Info,
                    source: self.id().to_owned(),
                    path: Some(name.to_owned()),
                    message_key: "test.unclaimed".to_owned(),
                    message_args: Default::default(),
                })
                .collect();
            Ok(CollectOutput {
                issues,
                ..CollectOutput::default()
            })
        }
    }

    fn context() -> CollectContext {
        let (events, _rx) = unbounded_channel();
        CollectContext {
            env: Arc::new(Environment::fake(Path::new("/fake"))),
            config: Arc::new(Config::default()),
            scanner: Arc::new(EmptyFs),
            events,
            cancel: CancellationToken::new(),
        }
    }

    /// T-01-03: claimed paths of a collector reach a post-collector via `PriorResults`.
    #[tokio::test]
    async fn claimed_paths_reach_post_collector() {
        let ctx = context();
        let collectors: Vec<Box<dyn Collector>> = vec![Box::new(ClaimingCollector)];
        let mut findings = Vec::new();
        let mut claimed = PathSet::new();
        for collector in &collectors {
            let out = collector.collect(&ctx).await.unwrap();
            findings.extend(out.findings);
            claimed.extend(out.claimed_paths);
        }
        assert_eq!(claimed.len(), 1);

        let post: Box<dyn PostCollector> = Box::new(CheckingPostCollector);
        let prior = PriorResults {
            findings: &findings,
            claimed: &claimed,
        };
        let out = post.collect(&ctx, &prior).await.unwrap();
        let unclaimed: Vec<_> = out
            .issues
            .iter()
            .filter_map(|i| i.path.as_deref())
            .collect();
        assert_eq!(unclaimed, ["Code - Insiders"]);
    }

    #[tokio::test]
    async fn collector_error_is_returned() {
        let mut ctx = context();
        let mut env = Environment::fake(Path::new("/fake"));
        env.known_folders.clear();
        ctx.env = Arc::new(env);
        let err = ClaimingCollector.collect(&ctx).await.unwrap_err();
        assert_eq!(err.to_string(), "no AppData");
    }
}
