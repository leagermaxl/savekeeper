//! The Measure phase of `ScanPipeline` (SPEC-01 §4.4, SPEC-03 T-03-07).

// Helpers outside `#[test]` functions are not covered by `allow-unwrap-in-tests`.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use sk_core::collector::{CollectContext, CollectOutput, Collector};
use sk_core::config::Config;
use sk_core::env::{Environment, KnownFolder};
use sk_core::error::CollectorError;
use sk_core::events::{Event, ScanPhase};
use sk_core::model::{
    Category, Evidence, EvidenceSource, Finding, FindingId, IssueSeverity, Sensitivity, Target,
};
use sk_core::registry::MemRegistry;
use sk_core::template::PathTemplate;
use sk_core::CancellationToken;
use sk_engine::{ScanOptions, ScanPipeline};
use sk_scan::{MemFs, ReparseKind};
use tokio::sync::mpsc::unbounded_channel;

fn env() -> Environment {
    Environment::fake(Path::new(if cfg!(windows) { r"C:\fake" } else { "/fake" }))
}

fn app_data(rel: &str) -> PathBuf {
    env().known_folder(KnownFolder::AppData).unwrap().join(rel)
}

fn finding(rel: &str) -> Finding {
    let resolved = app_data(rel);
    let target = Target::FileSet {
        root: PathTemplate::from_path(&resolved, &env()),
        resolved,
        include: vec![],
        exclude: vec![],
    };
    Finding {
        id: FindingId::for_target(&target),
        target,
        category: Category::AppConfig,
        app: None,
        title: rel.to_owned(),
        evidence: vec![Evidence {
            source: EvidenceSource::User,
            message_key: "evidence.test".to_owned(),
            message_args: BTreeMap::new(),
            confidence: 1.0,
            importance: None,
        }],
        stats: None,
        sensitivity: Sensitivity::None,
        score: None,
        default_selected: false,
        requires_elevation: false,
        tags: vec![],
        children: vec![],
        notes_key: None,
    }
}

struct Rules;

#[async_trait]
impl Collector for Rules {
    fn id(&self) -> &'static str {
        "rules"
    }
    fn display_key(&self) -> &'static str {
        "collector.rules"
    }
    async fn collect(&self, _: &CollectContext) -> Result<CollectOutput, CollectorError> {
        Ok(CollectOutput {
            findings: vec![finding("App"), finding("Link")],
            ..CollectOutput::default()
        })
    }
}

fn scanner() -> Arc<MemFs> {
    let mut fs = MemFs::new();
    let s = |rel: &str| app_data(rel).to_string_lossy().into_owned();
    fs.add_file(&s("App/settings.json"), 40, "-1d", None)
        .add_file(&s("App/node_modules/x.js"), 1000, "-1d", None)
        .add_file(&s("App/sub/data.bin"), 2, "-1d", None)
        .add_reparse(&s("Link"), ReparseKind::Junction);
    Arc::new(fs)
}

#[tokio::test]
async fn measure_phase_fills_stats_and_reports_issues() {
    let pipeline = ScanPipeline::new(Arc::new(Config::default()))
        .with_environment(env())
        .with_scanner(scanner())
        .with_games_registry(Arc::new(MemRegistry::new()))
        .with_collector(Arc::new(Rules));
    let (tx, mut rx) = unbounded_channel();
    let report = pipeline
        .run(ScanOptions::default(), tx, CancellationToken::new())
        .await
        .unwrap();

    let app = report.findings.iter().find(|f| f.title == "App").unwrap();
    let stats = app.stats.as_ref().unwrap();
    // node_modules is a built-in exclusion.
    assert_eq!((stats.total_bytes, stats.file_count), (42, 2));
    let link = report.findings.iter().find(|f| f.title == "Link").unwrap();
    assert_eq!(link.tags, ["reparse_root"]);
    assert_eq!(link.stats.as_ref().unwrap().file_count, 0);

    let issues: Vec<_> = report
        .issues
        .iter()
        .map(|i| (i.source.as_str(), i.message_key.as_str(), i.severity))
        .collect();
    assert_eq!(
        issues,
        [("measure", "issue.scan.reparse_root", IssueSeverity::Info)]
    );

    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }
    let started = events
        .iter()
        .position(|e| {
            matches!(
                e,
                Event::PhaseStarted {
                    phase: ScanPhase::Measure
                }
            )
        })
        .unwrap();
    let finished = events
        .iter()
        .position(|e| {
            matches!(
                e,
                Event::PhaseFinished {
                    phase: ScanPhase::Measure,
                    ..
                }
            )
        })
        .unwrap();
    let inside = &events[started..finished];
    let last_progress = inside.iter().rev().find_map(|e| match e {
        Event::Progress {
            phase: ScanPhase::Measure,
            done,
            total,
            ..
        } => Some((*done, *total)),
        _ => None,
    });
    assert_eq!(last_progress, Some((2, Some(2))));
    assert!(inside.iter().any(
        |e| matches!(e, Event::Issue { issue } if issue.message_key == "issue.scan.reparse_root")
    ));
}

#[tokio::test]
async fn bad_exclude_globs_fall_back_to_builtin() {
    let mut config = Config::default();
    config.scan.exclude_globs = vec!["a[b".to_owned()];
    let pipeline = ScanPipeline::new(Arc::new(config))
        .with_environment(env())
        .with_scanner(scanner())
        .with_games_registry(Arc::new(MemRegistry::new()))
        .with_collector(Arc::new(Rules));
    let (tx, _rx) = unbounded_channel();
    let report = pipeline
        .run(ScanOptions::default(), tx, CancellationToken::new())
        .await
        .unwrap();
    let bad = report
        .issues
        .iter()
        .find(|i| i.message_key == "issue.scan.bad_exclude_globs")
        .unwrap();
    assert_eq!(
        (bad.source.as_str(), bad.severity),
        ("engine", IssueSeverity::Warning)
    );
    let app = report.findings.iter().find(|f| f.title == "App").unwrap();
    assert_eq!(app.stats.as_ref().unwrap().total_bytes, 42);
}

async fn run_with_depth(config_depth: u32, scan_depth: Option<u32>) -> (u64, Option<u32>) {
    let mut config = Config::default();
    config.scan.max_depth = config_depth;
    let pipeline = ScanPipeline::new(Arc::new(config))
        .with_environment(env())
        .with_scanner(scanner())
        .with_games_registry(Arc::new(MemRegistry::new()))
        .with_collector(Arc::new(Rules));
    let (tx, _rx) = unbounded_channel();
    let opts = ScanOptions {
        max_depth: scan_depth,
        ..ScanOptions::default()
    };
    let report = pipeline
        .run(opts, tx, CancellationToken::new())
        .await
        .unwrap();
    let app = report.findings.iter().find(|f| f.title == "App").unwrap();
    (
        app.stats.as_ref().unwrap().file_count,
        report.options.max_depth,
    )
}

/// `ScanOptions.max_depth` wins over the config, and the report records the
/// depth used (SPEC-01 §4.4).
#[tokio::test]
async fn measure_depth_comes_from_scan_options_or_config() {
    // App/settings.json is at depth 1, App/sub/data.bin at depth 2.
    assert_eq!(run_with_depth(32, Some(1)).await, (1, Some(1)));
    assert_eq!(run_with_depth(1, Some(32)).await, (2, Some(32)));
    assert_eq!(run_with_depth(1, None).await, (1, Some(1)));
    assert_eq!(run_with_depth(32, None).await, (2, Some(32)));
}
