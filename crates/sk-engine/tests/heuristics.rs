//! The heuristics of SPEC-07 in `ScanPipeline` (T-07-01): the built-in
//! `heuristics` post-collector, still without heuristics, runs in the
//! `Heuristics` phase, and `with_post_collector` replaces it.

// Helpers outside `#[test]` functions are not covered by `allow-unwrap-in-tests`.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use sk_core::collector::{CollectContext, CollectOutput, Collector, PostCollector, PriorResults};
use sk_core::config::Config;
use sk_core::env::{Environment, KnownFolder};
use sk_core::error::CollectorError;
use sk_core::events::{Event, ScanPhase};
use sk_core::model::{
    Category, CollectorToggles, Evidence, EvidenceSource, Finding, FindingId, Sensitivity, Target,
};
use sk_core::registry::MemRegistry;
use sk_core::template::PathTemplate;
use sk_core::CancellationToken;
use sk_engine::{ScanOptions, ScanPipeline};
use sk_heuristics::{HeuristicsCollector, HeuristicsConfig};
use sk_scan::MemFs;
use sk_system::SystemCollector;
use tokio::sync::mpsc::unbounded_channel;

fn env() -> Environment {
    Environment::fake(Path::new(if cfg!(windows) { r"C:\fake" } else { "/fake" }))
}

fn app_data(name: &str) -> PathBuf {
    env().known_folder(KnownFolder::AppData).unwrap().join(name)
}

fn finding(name: &str) -> Finding {
    let target = Target::FileSet {
        root: PathTemplate::parse(&format!(r"{{APPDATA}}\{name}")).unwrap(),
        resolved: app_data(name),
        include: vec![],
        exclude: vec![],
    };
    Finding {
        id: FindingId::for_target(&target),
        target,
        category: Category::AppConfig,
        app: None,
        title: name.to_owned(),
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

/// Replaces the built-in rules: finds and claims `{APPDATA}\Code`.
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
            findings: vec![finding("Code")],
            claimed_paths: vec![app_data("Code")],
            issues: vec![],
        })
    }
}

/// Records how often it ran and whether it saw the claims of the rules.
#[derive(Default)]
struct Probe {
    id: &'static str,
    calls: AtomicUsize,
    saw_claim: AtomicUsize,
}

#[async_trait]
impl PostCollector for Probe {
    fn id(&self) -> &'static str {
        self.id
    }
    async fn collect(
        &self,
        _: &CollectContext,
        prior: &PriorResults<'_>,
    ) -> Result<CollectOutput, CollectorError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if prior.claimed.covers(&app_data("Code").join("User")) && prior.findings.len() == 1 {
            self.saw_claim.fetch_add(1, Ordering::SeqCst);
        }
        Ok(CollectOutput::default())
    }
}

/// Nothing of this machine: fake environment, empty registries, empty file
/// system, no system exporters, a fake `rules` collector.
fn pipeline() -> ScanPipeline {
    ScanPipeline::new(Arc::new(Config::default()))
        .with_environment(env())
        .with_scanner(Arc::new(MemFs::new()))
        .with_rules_registry(Arc::new(MemRegistry::new()))
        .with_games_registry(Arc::new(MemRegistry::new()))
        .with_system_registry(Arc::new(MemRegistry::new()))
        .with_collector(Arc::new(SystemCollector::with_exporters(Vec::new())))
        .with_collector(Arc::new(Rules))
        .with_app_version("test".to_owned())
}

async fn run(pipeline: &ScanPipeline, toggles: CollectorToggles) -> (Vec<String>, Vec<Event>) {
    let (tx, mut rx) = unbounded_channel();
    let opts = ScanOptions {
        collectors: toggles,
        ..ScanOptions::default()
    };
    let report = pipeline
        .run(opts, tx, CancellationToken::new())
        .await
        .unwrap();
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    let titles = report.findings.into_iter().map(|f| f.title).collect();
    (titles, events)
}

/// T-07-01: the built-in heuristics collector, with no heuristics yet, runs
/// in the `Heuristics` phase without findings or issues of its own and passes
/// the claims of the collectors on to the post-collectors after it.
#[tokio::test]
async fn builtin_heuristics_run_in_the_heuristics_phase() {
    let probe = Arc::new(Probe {
        id: "after-heuristics",
        ..Probe::default()
    });
    let pipeline = pipeline().with_post_collector(Arc::clone(&probe) as Arc<dyn PostCollector>);
    let (titles, events) = run(&pipeline, CollectorToggles::default()).await;

    assert_eq!(titles, ["Code"]);
    assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
    assert_eq!(probe.saw_claim.load(Ordering::SeqCst), 1);
    let heuristics_phase = events.iter().filter(|e| {
        matches!(
            e,
            Event::PhaseStarted {
                phase: ScanPhase::Heuristics
            } | Event::PhaseFinished {
                phase: ScanPhase::Heuristics,
                ..
            }
        )
    });
    assert_eq!(heuristics_phase.count(), 2);
    let added: u32 = events
        .iter()
        .filter_map(|e| match e {
            Event::FindingsAdded { count } => Some(*count),
            _ => None,
        })
        .sum();
    assert_eq!(added, 1, "only the finding of the rules");
}

/// A post-collector with the id `heuristics` replaces the built-in one;
/// switching the heuristics off skips it.
#[tokio::test]
async fn with_post_collector_replaces_builtin_heuristics() {
    let probe = Arc::new(Probe {
        id: "heuristics",
        ..Probe::default()
    });
    let pipeline = pipeline().with_post_collector(Arc::clone(&probe) as Arc<dyn PostCollector>);
    run(&pipeline, CollectorToggles::default()).await;
    assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
    assert_eq!(probe.saw_claim.load(Ordering::SeqCst), 1);

    let off = CollectorToggles {
        heuristics: false,
        ..CollectorToggles::default()
    };
    let (titles, _) = run(&pipeline, off).await;
    assert_eq!(titles, ["Code"]);
    assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
}

/// An explicitly added `HeuristicsCollector` works as the built-in one.
#[tokio::test]
async fn explicit_heuristics_collector_runs() {
    let collector = HeuristicsCollector::new(HeuristicsConfig::default());
    let pipeline = pipeline().with_post_collector(Arc::new(collector));
    let (titles, _) = run(&pipeline, CollectorToggles::default()).await;
    assert_eq!(titles, ["Code"]);
}
