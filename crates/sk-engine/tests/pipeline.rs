//! `ScanPipeline` with fake collectors (SPEC-01 §6, T-01-06).

// Helpers outside `#[test]` functions are not covered by `allow-unwrap-in-tests`.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use sk_core::collector::{CollectContext, CollectOutput, Collector, PostCollector, PriorResults};
use sk_core::config::Config;
use sk_core::env::{Environment, KnownFolder};
use sk_core::error::CollectorError;
use sk_core::events::{Event, ScanPhase};
use sk_core::model::{
    Category, CollectorToggles, Evidence, EvidenceSource, Finding, FindingId, IssueSeverity, Score,
    Sensitivity, Target,
};
use sk_core::template::PathTemplate;
use sk_core::CancellationToken;
use sk_engine::{EngineError, ScanOptions, ScanPipeline};
use sk_rules::MemRegistry;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

fn env() -> Environment {
    Environment::fake(Path::new(if cfg!(windows) { r"C:\fake" } else { "/fake" }))
}

fn finding(template: &str, category: Category, score: Option<f32>) -> Finding {
    let target = Target::FileSet {
        root: PathTemplate::parse(template).unwrap(),
        resolved: PathBuf::new(),
        include: vec![],
        exclude: vec![],
    };
    Finding {
        id: FindingId::for_target(&target),
        target,
        category,
        app: None,
        title: template.to_owned(),
        evidence: vec![Evidence {
            source: EvidenceSource::User,
            message_key: "evidence.test".to_owned(),
            message_args: BTreeMap::new(),
            confidence: 1.0,
            importance: None,
        }],
        stats: None,
        sensitivity: Sensitivity::None,
        score: score.map(|value| Score {
            value,
            components: BTreeMap::new(),
        }),
        default_selected: false,
        requires_elevation: false,
        tags: vec![],
        children: vec![],
        notes_key: None,
    }
}

enum Behavior {
    Ok(Vec<Finding>, Vec<&'static str>),
    Fail,
    Panic,
    Hang,
}

struct Fake {
    id: &'static str,
    behavior: Behavior,
    calls: AtomicUsize,
}

impl Fake {
    fn new(id: &'static str, behavior: Behavior) -> Arc<Self> {
        Arc::new(Self {
            id,
            behavior,
            calls: AtomicUsize::new(0),
        })
    }
}

#[async_trait]
impl Collector for Fake {
    fn id(&self) -> &'static str {
        self.id
    }
    fn display_key(&self) -> &'static str {
        "collector.fake"
    }
    async fn collect(&self, ctx: &CollectContext) -> Result<CollectOutput, CollectorError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match &self.behavior {
            Behavior::Ok(findings, claims) => {
                let app_data = ctx.env.known_folder(KnownFolder::AppData).unwrap();
                Ok(CollectOutput {
                    findings: findings.clone(),
                    claimed_paths: claims.iter().map(|c| app_data.join(c)).collect(),
                    issues: vec![],
                })
            }
            Behavior::Fail => Err(CollectorError::Other("broken rules".to_owned())),
            Behavior::Panic => panic!("collector bug"),
            Behavior::Hang => {
                tokio::time::sleep(Duration::from_secs(30)).await;
                Ok(CollectOutput::default())
            }
        }
    }
}

/// Adds one `Unknown` finding for each candidate not claimed yet.
struct Heuristics;

#[async_trait]
impl PostCollector for Heuristics {
    fn id(&self) -> &'static str {
        "heuristics"
    }
    async fn collect(
        &self,
        ctx: &CollectContext,
        prior: &PriorResults<'_>,
    ) -> Result<CollectOutput, CollectorError> {
        let app_data = ctx.env.known_folder(KnownFolder::AppData).unwrap();
        let findings = ["Code", "Mystery"]
            .into_iter()
            .filter(|name| !prior.claimed.covers(&app_data.join(name)))
            .map(|name| finding(&format!(r"{{APPDATA}}\{name}"), Category::Unknown, None))
            .collect();
        assert_eq!(prior.findings.len(), 2);
        Ok(CollectOutput {
            findings,
            ..CollectOutput::default()
        })
    }
}

fn drain(rx: &mut UnboundedReceiver<Event>) -> Vec<Event> {
    let mut out = Vec::new();
    while let Ok(event) = rx.try_recv() {
        out.push(event);
    }
    out
}

/// The built-in rules run on the fake environment with an empty registry, so
/// that nothing of this machine's registry gets into the reports.
fn pipeline() -> ScanPipeline {
    ScanPipeline::new(Arc::new(Config::default()))
        .with_environment(env())
        .with_rules_registry(Arc::new(MemRegistry::new()))
        .with_app_version("test".to_owned())
}

#[tokio::test]
async fn runs_phases_and_isolates_failures() {
    let rules = Fake::new(
        "rules",
        Behavior::Ok(
            vec![
                finding(r"{APPDATA}\Low", Category::AppConfig, Some(0.2)),
                finding(r"{APPDATA}\Save", Category::GameSave, Some(0.9)),
            ],
            vec!["Code"],
        ),
    );
    let pipeline = pipeline()
        .with_collector(rules)
        .with_collector(Fake::new("system", Behavior::Fail))
        .with_collector(Fake::new("games", Behavior::Panic))
        .with_post_collector(Arc::new(Heuristics));
    let (tx, mut rx) = unbounded_channel();
    let report = pipeline
        .run(ScanOptions::default(), tx, CancellationToken::new())
        .await
        .unwrap();

    // Sorted by category order: GameSave, AppConfig, Unknown.
    let titles: Vec<_> = report.findings.iter().map(|f| f.title.as_str()).collect();
    assert_eq!(
        titles,
        [r"{APPDATA}\Save", r"{APPDATA}\Low", r"{APPDATA}\Mystery"]
    );

    let mut issues: Vec<_> = report
        .issues
        .iter()
        .map(|i| (i.source.as_str(), i.message_key.as_str(), i.severity))
        .collect();
    issues.sort();
    assert_eq!(
        issues,
        [
            ("games", "collector.panicked", IssueSeverity::Error),
            ("system", "collector.failed", IssueSeverity::Error),
        ]
    );
    let failed = report.issues.iter().find(|i| i.source == "system").unwrap();
    assert_eq!(failed.message_args["error"], "broken rules");

    let events = drain(&mut rx);
    let started: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::PhaseStarted { phase } => Some(*phase),
            _ => None,
        })
        .collect();
    assert_eq!(
        started,
        [
            ScanPhase::Environment,
            ScanPhase::Collect,
            ScanPhase::Heuristics,
            ScanPhase::Measure,
            ScanPhase::Classify,
            ScanPhase::Score,
            ScanPhase::Done,
        ]
    );
    let finished = events
        .iter()
        .filter(|e| matches!(e, Event::PhaseFinished { .. }))
        .count();
    assert_eq!(finished, 7);
    let added: u32 = events
        .iter()
        .filter_map(|e| match e {
            Event::FindingsAdded { count } => Some(*count),
            _ => None,
        })
        .sum();
    assert_eq!(added, 3);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, Event::Issue { .. }))
            .count(),
        2
    );

    assert_eq!(report.schema_version, 1);
    assert_eq!(report.app_version, "test");
    assert!(report.finished_at >= report.started_at);
}

#[tokio::test]
async fn toggles_skip_collectors() {
    let games = Fake::new("games", Behavior::Ok(vec![], vec![]));
    let custom = Fake::new("custom", Behavior::Ok(vec![], vec![]));
    let pipeline = pipeline()
        .with_collector(Arc::clone(&games) as Arc<dyn Collector>)
        .with_collector(Arc::clone(&custom) as Arc<dyn Collector>);
    let opts = ScanOptions {
        collectors: CollectorToggles {
            games: false,
            ..CollectorToggles::default()
        },
        roots: vec![env()
            .known_folder(KnownFolder::Home)
            .unwrap()
            .join("Projects")],
        ..ScanOptions::default()
    };
    let (tx, _rx) = unbounded_channel();
    let report = pipeline
        .run(opts, tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(games.calls.load(Ordering::SeqCst), 0);
    assert_eq!(custom.calls.load(Ordering::SeqCst), 1);
    assert!(!report.options.collectors.games);
    assert_eq!(report.options.roots[0].as_str(), r"{HOME}\Projects");
}

#[tokio::test]
async fn cancellation_stops_a_hanging_collector() {
    let dir = tempfile::tempdir().unwrap();
    let pipeline = pipeline()
        .with_collector(Fake::new("rules", Behavior::Hang))
        .with_scans_dir(dir.path().to_path_buf());
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        trigger.cancel();
    });
    let (tx, _rx) = unbounded_channel();
    let start = Instant::now();
    let result = pipeline.run(ScanOptions::default(), tx, cancel).await;
    assert!(matches!(result, Err(EngineError::Cancelled)), "{result:?}");
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        0,
        "no report saved"
    );
}

#[tokio::test]
async fn already_cancelled_does_not_start() {
    let cancel = CancellationToken::new();
    cancel.cancel();
    let (tx, mut rx) = unbounded_channel();
    let result = pipeline().run(ScanOptions::default(), tx, cancel).await;
    assert!(matches!(result, Err(EngineError::Cancelled)));
    assert!(drain(&mut rx).is_empty());
}

#[tokio::test]
async fn reports_are_saved_and_pruned() {
    let dir = tempfile::tempdir().unwrap();
    let pipeline = pipeline().with_scans_dir(dir.path().to_path_buf());
    let mut ids = Vec::new();
    for _ in 0..12 {
        let (tx, _rx) = unbounded_channel();
        let report = pipeline
            .run(ScanOptions::default(), tx, CancellationToken::new())
            .await
            .unwrap();
        ids.push(report.scan_id);
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    let mut files: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    files.sort();
    let mut expected: Vec<String> = ids[2..].iter().map(|id| format!("{id}.json")).collect();
    expected.sort();
    assert_eq!(files, expected);

    let saved = std::fs::read_to_string(dir.path().join(format!("{}.json", ids[11]))).unwrap();
    let report = sk_core::model::ScanReport::from_json(&saved).unwrap();
    assert_eq!(report.scan_id, ids[11]);
}

/// Lists `dir` through the context's scanner and records what it saw.
struct ListDir {
    dir: PathBuf,
    seen: std::sync::Mutex<Option<Vec<PathBuf>>>,
}

#[async_trait]
impl Collector for ListDir {
    fn id(&self) -> &'static str {
        "list-dir"
    }
    fn display_key(&self) -> &'static str {
        "collector.list_dir"
    }
    async fn collect(&self, ctx: &CollectContext) -> Result<CollectOutput, CollectorError> {
        let entries = ctx.scanner.read_dir(&self.dir).unwrap();
        *self.seen.lock().unwrap() = Some(entries.into_iter().map(|e| e.rel).collect());
        Ok(CollectOutput::default())
    }
}

/// Without `with_scanner` the pipeline scans the real file system (SPEC-01 §4.4).
#[tokio::test]
async fn default_scanner_reads_the_real_file_system() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("save.dat"), b"x").unwrap();
    let collector = Arc::new(ListDir {
        dir: dir.path().to_path_buf(),
        seen: std::sync::Mutex::new(None),
    });
    let (tx, _rx) = unbounded_channel();
    pipeline()
        .with_collector(collector.clone())
        .run(ScanOptions::default(), tx, CancellationToken::new())
        .await
        .unwrap();
    let seen = collector.seen.lock().unwrap().clone();
    assert_eq!(seen, Some(vec![PathBuf::from("save.dat")]));
}
