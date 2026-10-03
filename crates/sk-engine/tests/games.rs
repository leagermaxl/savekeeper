//! The built-in `games` collector of `ScanPipeline` (SPEC-05 T-05-12).
//!
//! The manifest is the 20-game excerpt put into the cache of the data folder;
//! no test downloads anything: `games.auto_update` is off, the network is
//! switched off, or the URL has a scheme that fails before any connection.
//! Folders written by the tests live under `CARGO_TARGET_TMPDIR`.

// Helpers outside `#[test]` functions are not covered by `allow-unwrap-in-tests`.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use sk_core::collector::{CollectContext, CollectOutput, Collector};
use sk_core::config::Config;
use sk_core::env::Environment;
use sk_core::error::CollectorError;
use sk_core::events::Event;
use sk_core::model::{
    Category, CollectorToggles, EvidenceSource, Finding, IssueSeverity, RegHive, ScanReport,
};
use sk_core::registry::{KeyState, MemRegistry, RegistryReader};
use sk_core::CancellationToken;
use sk_engine::{ScanOptions, ScanPipeline};
use sk_scan::MemFs;
use sk_testkit::materialize_profile;
use tempfile::TempDir;
use tokio::sync::mpsc::unbounded_channel;

const MINI: &str = include_str!("../../../fixtures/samples/ludusavi/manifest-mini.yaml");
const OFFLINE_KEY: &str = "issue.games.manifest_offline";

/// A temporary folder on the drive of the build output.
fn tmp() -> TempDir {
    tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap()
}

/// A data folder whose manifest cache holds the 20-game excerpt.
fn data_dir() -> TempDir {
    let data = tmp();
    let cache = data.path().join("cache");
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(cache.join("ludusavi-manifest.yaml"), MINI).unwrap();
    data
}

/// A config that never downloads the manifest.
fn offline_config() -> Config {
    let mut config = Config::default();
    config.games.auto_update = false;
    config
}

/// A config whose manifest update is always due and always fails without
/// a connection (unsupported URL scheme): `issue.games.manifest_offline`
/// whenever the store may use the network.
fn failing_config() -> Config {
    let mut config = Config::default();
    config.games.auto_update = true;
    config.games.update_interval_hours = 0;
    config.games.manifest_url = "unsupported://savekeeper.invalid/manifest.yaml".to_owned();
    config
}

fn pipeline(config: Config, env: Environment) -> ScanPipeline {
    ScanPipeline::new(Arc::new(config))
        .with_environment(env)
        .with_rules_registry(Arc::new(MemRegistry::new()))
        .with_games_registry(Arc::new(MemRegistry::new()))
}

/// An empty fake profile in memory.
fn mem_pipeline(config: Config, data: &Path) -> ScanPipeline {
    let env = Environment::fake(Path::new(if cfg!(windows) { r"C:\fake" } else { "/fake" }));
    pipeline(config, env)
        .with_scanner(Arc::new(MemFs::new()))
        .with_data_dir(data.to_path_buf())
}

async fn run(pipeline: &ScanPipeline, opts: ScanOptions) -> (ScanReport, Vec<Event>) {
    let (tx, mut rx) = unbounded_channel();
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

/// Manifest games named by the evidence of `finding`.
fn games_of(finding: &Finding) -> impl Iterator<Item = &str> {
    finding.evidence.iter().filter_map(|e| match &e.source {
        EvidenceSource::Ludusavi { game, .. } => Some(game.as_str()),
        _ => None,
    })
}

fn manifest_games(report: &ScanReport) -> BTreeSet<&str> {
    report.findings.iter().flat_map(games_of).collect()
}

fn has_offline_issue(report: &ScanReport) -> bool {
    report.issues.iter().any(|i| i.message_key == OFFLINE_KEY)
}

/// The `gamer` profile (SPEC-12 §4.3) on disk, scanned by `RealFs`: the
/// installed Steam game and the leftovers of the others become findings.
#[tokio::test]
async fn gamer_profile_gives_game_findings() {
    let profile = tmp();
    let env = materialize_profile("gamer", profile.path()).unwrap();
    let data = data_dir();
    let pipeline = pipeline(offline_config(), env)
        .with_data_dir(data.path().to_path_buf())
        .with_network(false);

    let (report, events) = run(&pipeline, ScanOptions::default()).await;

    assert!(
        report.issues.iter().all(|i| i.source != "games"),
        "{:?}",
        report.issues
    );
    let games = manifest_games(&report);
    for game in [
        "ELDEN RING",
        "Hollow Knight",
        "Cyberpunk 2077",
        "Satisfactory",
    ] {
        assert!(games.contains(game), "{game} missing in {games:?}");
    }

    let of = |game: &str| -> Vec<&Finding> {
        report
            .findings
            .iter()
            .filter(|f| games_of(f).any(|g| g == game))
            .collect()
    };
    // ELDEN RING is installed through Steam: its save is tagged with the
    // launcher and measured, its install folder is reinstallable.
    let elden = of("ELDEN RING");
    let save = elden
        .iter()
        .find(|f| f.category == Category::GameSave)
        .unwrap();
    assert!(save.tags.iter().any(|t| t == "steam"), "{:?}", save.tags);
    assert_eq!(save.title, "ELDEN RING — games.title.save");
    assert!(save.stats.as_ref().unwrap().total_bytes >= 2 * 28_311_552);
    assert_eq!(save.app.as_ref().unwrap().installed, Some(true));
    let install = report
        .findings
        .iter()
        .find(|f| {
            f.category == Category::Reinstallable
                && f.app.as_ref().is_some_and(|a| a.id == "elden-ring")
        })
        .unwrap();
    assert_eq!(install.title, "ELDEN RING — games.title.install_dir");

    // Hollow Knight is not installed: its saves are leftovers.
    let hollow = of("Hollow Knight");
    assert!(!hollow.is_empty());
    assert!(hollow
        .iter()
        .all(|f| f.tags.iter().any(|t| t == "not-installed")));

    assert!(events
        .iter()
        .any(|e| matches!(e, Event::FindingsAdded { .. })));
}

/// The issues of the manifest store reach the report and the events; with
/// the network switched off the store does not try to download.
#[tokio::test]
async fn store_issues_reach_the_report_and_offline_skips_the_download() {
    let data = data_dir();

    let (report, events) = run(
        &mem_pipeline(failing_config(), data.path()),
        ScanOptions::default(),
    )
    .await;
    let issue = report
        .issues
        .iter()
        .find(|i| i.message_key == OFFLINE_KEY)
        .unwrap();
    assert_eq!(issue.source, "games");
    assert_eq!(issue.severity, IssueSeverity::Info);
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::Issue { issue: sent } if sent == issue)));
    assert!(
        report
            .issues
            .iter()
            .all(|i| i.message_key != "collector.failed"),
        "{:?}",
        report.issues
    );

    let offline = mem_pipeline(failing_config(), data.path()).with_network(false);
    let (report, _) = run(&offline, ScanOptions::default()).await;
    assert!(report.issues.is_empty(), "{:?}", report.issues);
}

/// `CollectorToggles.games = false` skips the collector: no manifest load.
#[tokio::test]
async fn games_toggle_off_skips_the_collector() {
    let data = data_dir();
    let opts = ScanOptions {
        collectors: CollectorToggles {
            games: false,
            ..CollectorToggles::default()
        },
        ..ScanOptions::default()
    };
    let (report, _) = run(&mem_pipeline(failing_config(), data.path()), opts).await;
    assert!(!has_offline_issue(&report), "{:?}", report.issues);
}

/// Without a data folder there is no built-in games collector.
#[tokio::test]
async fn no_data_dir_no_games_collector() {
    let env = Environment::fake(Path::new(if cfg!(windows) { r"C:\fake" } else { "/fake" }));
    let pipeline = pipeline(failing_config(), env).with_scanner(Arc::new(MemFs::new()));
    let (report, _) = run(&pipeline, ScanOptions::default()).await;
    assert!(!has_offline_issue(&report), "{:?}", report.issues);
}

struct Replacement;

#[async_trait]
impl Collector for Replacement {
    fn id(&self) -> &'static str {
        "games"
    }
    fn display_key(&self) -> &'static str {
        "collector.games"
    }
    async fn collect(&self, _: &CollectContext) -> Result<CollectOutput, CollectorError> {
        Ok(CollectOutput::default())
    }
}

/// `with_collector` with the id `games` replaces the built-in collector.
#[tokio::test]
async fn a_games_collector_replaces_the_built_in_one() {
    let data = data_dir();
    let pipeline =
        mem_pipeline(failing_config(), data.path()).with_collector(Arc::new(Replacement));
    let (report, _) = run(&pipeline, ScanOptions::default()).await;
    assert!(!has_offline_issue(&report), "{:?}", report.issues);
}

/// The registry of the launcher detectors: its first read after `arm`
/// blocks the `Environment` phase until the manifest load of the run has
/// written the index into the cache (at most 30 s), and records whether it
/// did, i.e. whether the load ran before the phase finished.
struct WaitingRegistry {
    inner: MemRegistry,
    index: PathBuf,
    armed: AtomicBool,
    index_seen: AtomicBool,
}

impl WaitingRegistry {
    fn new(data: &Path) -> Self {
        Self {
            inner: MemRegistry::new(),
            index: data.join("cache").join("ludusavi-index.bin"),
            armed: AtomicBool::new(false),
            index_seen: AtomicBool::new(false),
        }
    }

    fn arm(&self) {
        self.index_seen.store(false, Ordering::SeqCst);
        self.armed.store(true, Ordering::SeqCst);
    }

    fn wait_for_index(&self) {
        if !self.armed.swap(false, Ordering::SeqCst) {
            return;
        }
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if self.index.exists() {
                self.index_seen.store(true, Ordering::SeqCst);
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl RegistryReader for WaitingRegistry {
    fn key_state(&self, hive: RegHive, key: &str) -> KeyState {
        self.wait_for_index();
        self.inner.key_state(hive, key)
    }
    fn string_value(&self, hive: RegHive, key: &str, name: &str) -> Option<String> {
        self.wait_for_index();
        self.inner.string_value(hive, key, name)
    }
    fn dword_value(&self, hive: RegHive, key: &str, name: &str) -> Option<u32> {
        self.wait_for_index();
        self.inner.dword_value(hive, key, name)
    }
    fn subkeys(&self, hive: RegHive, key: &str) -> Vec<String> {
        self.wait_for_index();
        self.inner.subkeys(hive, key)
    }
}

/// The manifest load starts with the `Environment` phase and runs alongside
/// it (SPEC-05 T-05-15): the detectors see the index it writes before the
/// phase finishes. The collector awaits that load instead of loading again:
/// every run gives exactly one `manifest_offline` issue (one failed update
/// per load), and the manifest is used (no `collector.failed`).
#[tokio::test]
async fn manifest_loads_during_the_environment_phase_once_per_run() {
    let data = data_dir();
    let registry = Arc::new(WaitingRegistry::new(data.path()));
    let pipeline = mem_pipeline(failing_config(), data.path())
        .with_games_registry(Arc::clone(&registry) as Arc<dyn RegistryReader>);

    for round in 0..2 {
        registry.arm();
        let (report, events) = run(&pipeline, ScanOptions::default()).await;
        if round == 0 {
            assert!(
                registry.index_seen.load(Ordering::SeqCst),
                "the manifest was not loaded during the Environment phase"
            );
        }
        let offline = report
            .issues
            .iter()
            .filter(|i| i.message_key == OFFLINE_KEY)
            .count();
        assert_eq!(offline, 1, "round {round}: {:?}", report.issues);
        assert!(
            report
                .issues
                .iter()
                .all(|i| i.source != "games" || i.message_key == OFFLINE_KEY),
            "round {round}: {:?}",
            report.issues
        );
        let environment_done = events
            .iter()
            .position(|e| {
                matches!(
                    e,
                    Event::PhaseFinished {
                        phase: sk_core::events::ScanPhase::Environment,
                        ..
                    }
                )
            })
            .unwrap();
        let offline_sent = events
            .iter()
            .position(|e| matches!(e, Event::Issue { issue } if issue.message_key == OFFLINE_KEY))
            .unwrap();
        assert!(environment_done < offline_sent);
    }
}
