use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use sk_core::collector::{CollectContext, Collector};
use sk_core::config::Config;
use sk_core::env::Environment;
use sk_core::fs::{
    DirEntryInfo, EntryMeta, FsError, FsScanner, Readability, WalkControl, WalkOptions, WalkStats,
};
use sk_core::model::{
    AppKind, AppRef, Category, Evidence, EvidenceSource, Finding, FindingId, Sensitivity, Target,
};
use sk_core::CancellationToken;
use tokio::sync::mpsc::unbounded_channel;

use super::{SystemCollector, COLLECT_BUDGET};
use crate::{Availability, ExportContext, ExportError, ExportResult, SystemExporter};

/// A file system with nothing in it: the system collector does not use it.
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

/// What a fake exporter's `detect` returns.
#[derive(Clone, Copy)]
enum Detect {
    Available,
    NeedsElevation,
    Unavailable,
    Panic,
}

/// An exporter whose `detect` takes `delay` and returns `detect`.
struct FakeExporter {
    id: &'static str,
    detect: Detect,
    delay: Duration,
    planned: AtomicBool,
}

impl FakeExporter {
    fn new(id: &'static str, detect: Detect) -> Self {
        Self {
            id,
            detect,
            delay: Duration::ZERO,
            planned: AtomicBool::new(false),
        }
    }

    fn slow(id: &'static str, delay: Duration) -> Self {
        Self {
            delay,
            ..Self::new(id, Detect::Available)
        }
    }
}

#[async_trait]
impl SystemExporter for FakeExporter {
    fn id(&self) -> &'static str {
        self.id
    }

    fn detect(&self, _env: &Environment) -> Availability {
        std::thread::sleep(self.delay);
        match self.detect {
            Detect::Available => Availability::Available {
                estimated_bytes: Some(10),
                details: BTreeMap::new(),
            },
            Detect::NeedsElevation => Availability::NeedsElevation {
                estimated_bytes: None,
            },
            Detect::Unavailable => Availability::Unavailable {
                reason_key: "system.not_windows".to_owned(),
            },
            Detect::Panic => panic!("detect of {} failed", self.id),
        }
    }

    fn plan(&self, _env: &Environment, avail: &Availability) -> Option<Finding> {
        self.planned.store(true, Ordering::SeqCst);
        let target = Target::SystemExport {
            exporter_id: self.id.to_owned(),
            params: serde_json::json!({}),
        };
        Some(Finding {
            id: FindingId::for_target(&target),
            target,
            category: Category::SystemSettings,
            app: Some(AppRef {
                id: self.id.to_owned(),
                name: self.id.to_owned(),
                kind: AppKind::System,
                source_ids: BTreeMap::new(),
                installed: None,
                process_names: Vec::new(),
            }),
            title: self.id.to_owned(),
            evidence: vec![Evidence {
                source: EvidenceSource::System {
                    exporter_id: self.id.to_owned(),
                },
                message_key: "evidence.system".to_owned(),
                message_args: BTreeMap::new(),
                confidence: 1.0,
                importance: None,
            }],
            stats: None,
            sensitivity: Sensitivity::None,
            score: None,
            default_selected: true,
            requires_elevation: matches!(avail, Availability::NeedsElevation { .. }),
            tags: Vec::new(),
            children: Vec::new(),
            notes_key: None,
        })
    }

    async fn run(
        &self,
        _ctx: &ExportContext<'_>,
        _params: &serde_json::Value,
    ) -> Result<ExportResult, ExportError> {
        Err(ExportError::NotAvailable)
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

fn collector(exporters: Vec<Arc<FakeExporter>>) -> SystemCollector {
    SystemCollector::with_exporters(
        exporters
            .into_iter()
            .map(|e| e as Arc<dyn SystemExporter>)
            .collect(),
    )
}

fn exporter_ids(findings: &[Finding]) -> Vec<&str> {
    findings
        .iter()
        .map(|f| match &f.target {
            Target::SystemExport { exporter_id, .. } => exporter_id.as_str(),
            other => panic!("unexpected target {other:?}"),
        })
        .collect()
}

#[test]
fn id_and_display_key() {
    let collector = SystemCollector::with_exporters(Vec::new());
    assert_eq!(collector.id(), "system");
    assert_eq!(collector.display_key(), "collector.system");
}

#[tokio::test]
async fn plans_available_exporters_in_order() {
    let unavailable = Arc::new(FakeExporter::new("wifi", Detect::Unavailable));
    let collector = collector(vec![
        Arc::new(FakeExporter::new("winget", Detect::Available)),
        Arc::clone(&unavailable),
        Arc::new(FakeExporter::new("drivers", Detect::NeedsElevation)),
        Arc::new(FakeExporter::new("hosts", Detect::Available)),
    ]);
    let out = collector.collect(&context()).await.unwrap();

    assert_eq!(exporter_ids(&out.findings), ["winget", "drivers", "hosts"]);
    let elevation: Vec<bool> = out.findings.iter().map(|f| f.requires_elevation).collect();
    assert_eq!(elevation, [false, true, false]);
    assert!(
        !unavailable.planned.load(Ordering::SeqCst),
        "plan is not called for an unavailable export"
    );
    assert!(out.claimed_paths.is_empty());
    assert!(out.issues.is_empty());
}

#[tokio::test]
async fn disabled_exporters_are_skipped() {
    let disabled = Arc::new(FakeExporter::new("wifi", Detect::Available));
    let collector = collector(vec![
        Arc::new(FakeExporter::new("winget", Detect::Available)),
        Arc::clone(&disabled),
    ]);
    let mut ctx = context();
    let mut config = Config::default();
    config.system.disabled_exporters = vec!["wifi".to_owned()];
    ctx.config = Arc::new(config);

    let out = collector.collect(&ctx).await.unwrap();
    assert_eq!(exporter_ids(&out.findings), ["winget"]);
    assert!(!disabled.planned.load(Ordering::SeqCst));
}

/// NFR-06-01: detections run in parallel, so the collector takes about as long as the
/// slowest one, not the sum (8 × 400 ms = 3.2 s sequentially).
#[tokio::test]
async fn detections_run_in_parallel() {
    const IDS: [&str; 8] = ["a", "b", "c", "d", "e", "f", "g", "h"];
    let exporters = IDS
        .iter()
        .map(|id| Arc::new(FakeExporter::slow(id, Duration::from_millis(400))))
        .collect();
    let collector = collector(exporters);

    let started = Instant::now();
    let out = collector.collect(&context()).await.unwrap();
    let elapsed = started.elapsed();

    assert_eq!(exporter_ids(&out.findings), IDS);
    assert!(elapsed < Duration::from_secs(2), "took {elapsed:?}");
}

/// NFR-06-01 with the real exporter list of this machine.
#[tokio::test]
async fn registry_collects_within_budget() {
    let started = Instant::now();
    let out = SystemCollector::new().collect(&context()).await.unwrap();
    let elapsed = started.elapsed();
    assert!(elapsed <= COLLECT_BUDGET, "took {elapsed:?}");
    assert!(out.claimed_paths.is_empty());
}

#[tokio::test]
async fn cancelled_scan_gives_empty_output() {
    let planned = Arc::new(FakeExporter::new("winget", Detect::Available));
    let collector = collector(vec![Arc::clone(&planned)]);
    let ctx = context();
    ctx.cancel.cancel();

    let out = collector.collect(&ctx).await.unwrap();
    assert!(out.findings.is_empty());
    assert!(!planned.planned.load(Ordering::SeqCst));
}

#[tokio::test]
async fn cancellation_does_not_wait_for_slow_detection() {
    let collector = collector(vec![Arc::new(FakeExporter::slow(
        "drivers",
        Duration::from_secs(3),
    ))]);
    let ctx = context();
    let cancel = ctx.cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel.cancel();
    });

    let started = Instant::now();
    let out = collector.collect(&ctx).await.unwrap();
    assert!(out.findings.is_empty());
    assert!(started.elapsed() < Duration::from_secs(2));
}

/// The engine turns a panicking collector into `collector.panicked` (SPEC-01 §4.4).
#[tokio::test]
#[should_panic(expected = "detect of broken failed")]
async fn exporter_panic_is_passed_on() {
    let collector = collector(vec![
        Arc::new(FakeExporter::new("winget", Detect::Available)),
        Arc::new(FakeExporter::new("broken", Detect::Panic)),
    ]);
    let _ = collector.collect(&context()).await;
}
