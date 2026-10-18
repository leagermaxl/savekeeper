//! The `Classify` phase of `ScanPipeline` (SPEC-08 T-08-12) with
//! `MockClassifier`, without network: the `Unknown` findings with a summary
//! are classified, results applied, issues and progress reported, the cache
//! reused, cloud confirmation and content peek respected.

// Helpers outside `#[test]` functions are not covered by `allow-unwrap-in-tests`.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use sk_core::collector::{CollectContext, CollectOutput, Collector, PostCollector, PriorResults};
use sk_core::config::Config;
use sk_core::env::{Environment, KnownFolder};
use sk_core::error::{CollectorError, EngineError};
use sk_core::events::{Event, ScanPhase};
use sk_core::model::{
    Category, Evidence, EvidenceSource, Finding, FindingId, FolderSummary, LlmMode, ScanReport,
    Sensitivity, Target,
};
use sk_core::registry::MemRegistry;
use sk_core::template::PathTemplate;
use sk_core::CancellationToken;
use sk_engine::{ScanOptions, ScanPipeline};
use sk_llm::mock::{MockClassifier, MockFailure};
use sk_llm::{
    Classification, Classifier, ClassifierInfo, ClassifyItem, HealthReport, LlmError, UsageStats,
};
use sk_scan::MemFs;
use sk_system::SystemCollector;
use tempfile::TempDir;
use time::OffsetDateTime;
use tokio::sync::mpsc::unbounded_channel;

fn env() -> Environment {
    Environment::fake(Path::new(if cfg!(windows) { r"C:\fake" } else { "/fake" }))
}

fn app_data(name: &str) -> PathBuf {
    env().known_folder(KnownFolder::AppData).unwrap().join(name)
}

fn template(name: &str) -> PathTemplate {
    PathTemplate::parse(&format!(r"{{APPDATA}}\{name}")).unwrap()
}

fn data_dir() -> TempDir {
    tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap()
}

/// An `Unknown` finding of the heuristics for `{APPDATA}\<name>`.
fn unknown(name: &str) -> Finding {
    let target = Target::FileSet {
        root: template(name),
        resolved: app_data(name),
        include: vec![],
        exclude: vec![],
    };
    Finding {
        id: FindingId::for_target(&target),
        target,
        category: Category::Unknown,
        app: None,
        title: name.to_owned(),
        evidence: vec![Evidence {
            source: EvidenceSource::Heuristic {
                heuristic_id: "unk".to_owned(),
            },
            message_key: "evidence.test".to_owned(),
            message_args: BTreeMap::new(),
            confidence: 0.1,
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

fn summary(name: &str, sample: &str) -> FolderSummary {
    FolderSummary {
        path: template(name),
        total_bytes: 4096,
        file_count: 2,
        dir_count: 0,
        max_depth: 0,
        newest_mtime: Some(OffsetDateTime::now_utc()),
        oldest_mtime: None,
        ext_histogram: vec![],
        sample_names: vec![sample.to_owned()],
        top_children: vec![],
        markers: vec![],
        truncated: false,
    }
}

fn answer(category: Category, confidence: f32) -> Classification {
    Classification {
        id: "ignored".to_owned(),
        category,
        app_guess: Some("Foo App".to_owned()),
        importance: 0.7,
        confidence,
        reason: "Settings of Foo.".to_owned(),
        suggested_include: vec![],
        suggested_exclude: vec!["logs/**".to_owned()],
        from_cache: false,
    }
}

/// Replaces a built-in collector with one that finds nothing.
struct Empty(&'static str);

#[async_trait]
impl Collector for Empty {
    fn id(&self) -> &'static str {
        self.0
    }
    fn display_key(&self) -> &'static str {
        "collector.test"
    }
    async fn collect(&self, _: &CollectContext) -> Result<CollectOutput, CollectorError> {
        Ok(CollectOutput::default())
    }
}

/// Replaces the heuristics: `Unknown` findings with their summaries.
struct Unknowns(Vec<(&'static str, &'static str)>);

#[async_trait]
impl PostCollector for Unknowns {
    fn id(&self) -> &'static str {
        "heuristics"
    }
    async fn collect(
        &self,
        _: &CollectContext,
        _: &PriorResults<'_>,
    ) -> Result<CollectOutput, CollectorError> {
        Ok(CollectOutput {
            findings: self.0.iter().map(|(name, _)| unknown(name)).collect(),
            claimed_paths: self.0.iter().map(|(name, _)| app_data(name)).collect(),
            unknown_summaries: self.0.iter().map(|(n, s)| summary(n, s)).collect(),
            ..CollectOutput::default()
        })
    }
}

/// Delegates to a `MockClassifier` and records the items it was sent.
struct Recording {
    inner: MockClassifier,
    items: Mutex<Vec<ClassifyItem>>,
}

#[async_trait]
impl Classifier for Recording {
    fn info(&self) -> ClassifierInfo {
        self.inner.info()
    }
    async fn health(&self) -> Result<HealthReport, LlmError> {
        self.inner.health().await
    }
    async fn classify(
        &self,
        items: &[ClassifyItem],
        cancel: &CancellationToken,
    ) -> Result<Vec<Classification>, LlmError> {
        self.items.lock().unwrap().extend_from_slice(items);
        self.inner.classify(items, cancel).await
    }
    fn usage(&self) -> UsageStats {
        self.inner.usage()
    }
}

fn mem_fs() -> MemFs {
    let mut fs = MemFs::new();
    let file = app_data("Foo").join("settings.ini");
    fs.add_file(
        file.to_str().unwrap(),
        13,
        "2026-10-01T00:00:00Z",
        Some(b"volume=3\nx=1\n"),
    );
    let other = app_data("Bar").join("data.bin");
    fs.add_file(other.to_str().unwrap(), 10, "2026-10-01T00:00:00Z", None);
    fs
}

fn pipeline(config: Config, fs: Arc<MemFs>, classifier: Arc<dyn Classifier>) -> ScanPipeline {
    ScanPipeline::new(Arc::new(config))
        .with_environment(env())
        .with_scanner(fs)
        .with_rules_registry(Arc::new(MemRegistry::new()))
        .with_games_registry(Arc::new(MemRegistry::new()))
        .with_system_registry(Arc::new(MemRegistry::new()))
        .with_collector(Arc::new(SystemCollector::with_exporters(Vec::new())))
        .with_collector(Arc::new(Empty("rules")))
        .with_collector(Arc::new(Empty("games")))
        .with_post_collector(Arc::new(Unknowns(vec![
            ("Foo", "settings.ini"),
            ("Bar", "data.bin"),
        ])))
        .with_classifier(classifier)
        .with_app_version("test".to_owned())
}

fn mock() -> MockClassifier {
    MockClassifier::new()
        .with_response(template("Foo"), answer(Category::AppConfig, 0.9))
        .with_response(template("Bar"), answer(Category::AppData, 0.2))
}

async fn scan(pipeline: &ScanPipeline, llm: LlmMode) -> (ScanReport, Vec<Event>) {
    let (tx, mut rx) = unbounded_channel();
    let opts = ScanOptions {
        llm,
        ..ScanOptions::default()
    };
    let report = pipeline
        .run(opts, tx, CancellationToken::new())
        .await
        .unwrap();
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    (report, events)
}

fn by_title<'a>(report: &'a ScanReport, title: &str) -> &'a Finding {
    report.findings.iter().find(|f| f.title == title).unwrap()
}

fn keys(report: &ScanReport) -> Vec<&str> {
    report
        .issues
        .iter()
        .map(|i| i.message_key.as_str())
        .collect()
}

fn llm_evidence(finding: &Finding) -> Option<&Evidence> {
    finding
        .evidence
        .iter()
        .find(|e| matches!(e.source, EvidenceSource::Llm { .. }))
}

#[tokio::test]
async fn off_mode_classifies_nothing() {
    let dir = data_dir();
    let mock = Arc::new(mock());
    let p = pipeline(Config::default(), Arc::new(mem_fs()), mock.clone())
        .with_data_dir(dir.path().to_path_buf());
    let (report, events) = scan(&p, LlmMode::Off).await;
    assert_eq!(mock.call_count(), 0);
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    assert_eq!(by_title(&report, "Foo").category, Category::Unknown);
    assert_eq!(report.unknown_summaries.len(), 2);
    assert!(events.iter().any(|e| matches!(
        e,
        Event::PhaseFinished {
            phase: ScanPhase::Classify,
            ..
        }
    )));
    assert!(!dir.path().join("cache").exists(), "no cache without LLM");
}

#[tokio::test]
async fn local_mode_applies_results_and_reports_progress() {
    let dir = data_dir();
    let mock = Arc::new(mock());
    let p = pipeline(Config::default(), Arc::new(mem_fs()), mock.clone())
        .with_data_dir(dir.path().to_path_buf());
    let (report, events) = scan(&p, LlmMode::Local).await;
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    assert_eq!(mock.call_count(), 1);

    let foo = by_title(&report, "Foo");
    assert_eq!(foo.category, Category::AppConfig);
    assert_eq!(foo.app.as_ref().unwrap().name, "Foo App");
    let Target::FileSet { exclude, .. } = &foo.target else {
        panic!("file set")
    };
    assert_eq!(exclude, &["logs/**".to_owned()]);
    assert!(llm_evidence(foo).is_some());

    // Below `min_confidence_to_apply`: stays Unknown, the evidence is added.
    let bar = by_title(&report, "Bar");
    assert_eq!(bar.category, Category::Unknown);
    assert!(llm_evidence(bar).is_some());

    // Only the folder that stayed Unknown is left in the summaries.
    let left: Vec<&str> = report
        .unknown_summaries
        .iter()
        .map(|s| s.path.as_str())
        .collect();
    assert_eq!(left, [template("Bar").as_str()]);

    let progress = events.iter().any(|e| {
        matches!(
            e,
            Event::Progress {
                phase: ScanPhase::Classify,
                done: 2,
                total: Some(2),
                ..
            }
        )
    });
    assert!(progress, "{events:?}");
}

#[tokio::test]
async fn second_scan_is_served_from_the_cache() {
    let dir = data_dir();
    let mock = Arc::new(mock());
    let p = pipeline(Config::default(), Arc::new(mem_fs()), mock.clone())
        .with_data_dir(dir.path().to_path_buf());
    scan(&p, LlmMode::Local).await;
    assert_eq!(mock.call_count(), 1);
    let (report, _) = scan(&p, LlmMode::Local).await;
    assert_eq!(mock.call_count(), 1, "0 requests on the second scan");
    assert_eq!(by_title(&report, "Foo").category, Category::AppConfig);
}

#[tokio::test]
async fn runner_issues_reach_the_report_and_the_events() {
    let dir = data_dir();
    let mock = Arc::new(mock().with_health_failure(MockFailure::Unreachable));
    let p = pipeline(Config::default(), Arc::new(mem_fs()), mock.clone())
        .with_data_dir(dir.path().to_path_buf());
    let (report, events) = scan(&p, LlmMode::Local).await;
    assert_eq!(keys(&report), ["llm.unreachable"]);
    assert_eq!(report.issues[0].source, "llm");
    assert_eq!(mock.call_count(), 0);
    assert_eq!(by_title(&report, "Foo").category, Category::Unknown);
    assert!(events.iter().any(|e| matches!(
        e,
        Event::Issue { issue } if issue.message_key == "llm.unreachable"
    )));
}

#[tokio::test]
async fn the_scan_environment_is_used_by_the_privacy_check() {
    let dir = data_dir();
    let mock = Arc::new(mock());
    let fs = Arc::new(mem_fs());
    // The fake user name in a sample name blocks the item (SPEC-08 §4.8).
    let p = pipeline(Config::default(), fs, mock.clone())
        .with_post_collector(Arc::new(Unknowns(vec![("Baz", "user notes.txt")])))
        .with_data_dir(dir.path().to_path_buf());
    let (report, _) = scan(&p, LlmMode::Local).await;
    assert_eq!(keys(&report), ["llm.privacy_blocked"]);
    assert_eq!(mock.calls().concat().len(), 2, "Foo and Bar only");
}

#[tokio::test]
async fn cloud_classification_needs_a_confirmation() {
    let dir = data_dir();
    let mock = Arc::new(mock().with_local(false));
    let p = pipeline(Config::default(), Arc::new(mem_fs()), mock.clone())
        .with_data_dir(dir.path().to_path_buf());
    let (report, _) = scan(&p, LlmMode::Cloud).await;
    assert_eq!(keys(&report), ["llm.cloud_declined"]);
    assert_eq!(mock.call_count(), 0, "nothing sent without a confirmation");
    assert_eq!(by_title(&report, "Foo").category, Category::Unknown);

    // `confirm_cloud_each_scan = false`: classified.
    let mut config = Config::default();
    config.llm.confirm_cloud_each_scan = false;
    let p =
        pipeline(config, Arc::new(mem_fs()), mock.clone()).with_data_dir(dir.path().to_path_buf());
    let (report, _) = scan(&p, LlmMode::Cloud).await;
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    assert_eq!(mock.call_count(), 1);

    // Everything cached: nothing would be sent, no confirmation needed.
    let p = pipeline(Config::default(), Arc::new(mem_fs()), mock.clone())
        .with_data_dir(dir.path().to_path_buf());
    let (report, _) = scan(&p, LlmMode::Cloud).await;
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    assert_eq!(mock.call_count(), 1);
    assert_eq!(by_title(&report, "Foo").category, Category::AppConfig);
}

/// Items the classifier received in a scan with `allow_content_peek` and
/// `allow_content_peek_cloud` from the arguments, and the `read_head` calls.
async fn peeks(is_local: bool, local: bool, cloud: bool) -> (Vec<ClassifyItem>, u64) {
    let dir = data_dir();
    let recording = Arc::new(Recording {
        inner: mock().with_local(is_local),
        items: Mutex::new(Vec::new()),
    });
    let mut config = Config::default();
    config.llm.allow_content_peek = local;
    config.llm.allow_content_peek_cloud = cloud;
    config.llm.confirm_cloud_each_scan = false;
    let fs = Arc::new(mem_fs());
    let p = pipeline(config, fs.clone(), recording.clone()).with_data_dir(dir.path().to_path_buf());
    let mode = if is_local {
        LlmMode::Local
    } else {
        LlmMode::Cloud
    };
    let (report, _) = scan(&p, mode).await;
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    let items = recording.items.lock().unwrap().clone();
    (items, fs.calls().read_head)
}

#[tokio::test]
async fn content_peek_only_when_allowed() {
    let (items, reads) = peeks(true, true, false).await;
    let foo = items
        .iter()
        .find(|i| i.summary.path == template("Foo"))
        .unwrap();
    assert_eq!(foo.peek.len(), 1);
    assert_eq!(foo.peek[0].file, "settings.ini");
    assert!(foo.peek[0].head.contains("volume=3"), "{:?}", foo.peek);
    assert!(reads > 0);

    for (is_local, local, cloud) in [(true, false, true), (false, true, false)] {
        let (items, reads) = peeks(is_local, local, cloud).await;
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|i| i.peek.is_empty()));
        assert_eq!(reads, 0, "no file is read when peek is not allowed");
    }

    let (items, _) = peeks(false, false, true).await;
    assert!(
        items.iter().any(|i| !i.peek.is_empty()),
        "cloud peek allowed"
    );
}

#[tokio::test]
async fn without_a_data_folder_nothing_is_classified() {
    let mock = Arc::new(mock());
    let p = pipeline(Config::default(), Arc::new(mem_fs()), mock.clone());
    let (report, _) = scan(&p, LlmMode::Local).await;
    assert_eq!(mock.call_count(), 0);
    assert!(report.issues.is_empty(), "{:?}", report.issues);
}

#[tokio::test]
async fn cancellation_during_classify_stops_the_scan() {
    let dir = data_dir();
    let mock = Arc::new(mock().with_delay(Duration::from_secs(30)));
    let p = pipeline(Config::default(), Arc::new(mem_fs()), mock.clone())
        .with_data_dir(dir.path().to_path_buf());
    let (tx, mut rx) = unbounded_channel();
    let cancel = CancellationToken::new();
    let opts = ScanOptions {
        llm: LlmMode::Local,
        ..ScanOptions::default()
    };
    let scan = p.run(opts, tx, cancel.clone());
    let watcher = async {
        while let Some(event) = rx.recv().await {
            if matches!(
                event,
                Event::PhaseStarted {
                    phase: ScanPhase::Classify
                }
            ) {
                // Let the request start, then cancel.
                tokio::time::sleep(Duration::from_millis(50)).await;
                cancel.cancel();
                return Instant::now();
            }
        }
        Instant::now()
    };
    let (result, cancelled_at) = tokio::join!(scan, watcher);
    assert!(matches!(result, Err(EngineError::Cancelled)), "{result:?}");
    assert!(cancelled_at.elapsed() < Duration::from_secs(1));
}
