//! Launchers in the Environment phase of `ScanPipeline` (SPEC-05 T-05-05).

// Helpers outside `#[test]` functions are not covered by `allow-unwrap-in-tests`.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use sk_core::collector::{CollectContext, CollectOutput, Collector};
use sk_core::config::Config;
use sk_core::env::Environment;
use sk_core::error::CollectorError;
use sk_core::events::{Event, ScanPhase};
use sk_core::model::{EvidenceSource, IssueSeverity, LauncherSnapshot, ScanReport, Target};
use sk_core::registry::MemRegistry;
use sk_core::template::PathTemplate;
use sk_core::CancellationToken;
use sk_engine::{ScanOptions, ScanPipeline};
use sk_scan::MemFs;
use tokio::sync::mpsc::unbounded_channel;

const LIBRARYFOLDERS_KEY: &str = "issue.games.steam_libraryfolders_unreadable";

fn root() -> PathBuf {
    PathBuf::from(if cfg!(windows) { r"C:\fake" } else { "/fake" })
}

fn env() -> Environment {
    Environment::fake(&root())
}

/// `root()` joined with a `/`-separated relative path.
fn path(rel: &str) -> String {
    let path = rel.split('/').fold(root(), |p, c| p.join(c));
    path.to_string_lossy().into_owned()
}

const STEAM: &str = "Program Files (x86)/Steam";

const ACF: &str = r#""AppState"
{
    "appid"      "10"
    "name"       "Counter-Strike"
    "installdir" "Half-Life"
    "SizeOnDisk" "5"
}
"#;

/// Steam with one game and the account 111, without `libraryfolders.vdf`.
fn steam_profile() -> MemFs {
    let mut fs = MemFs::new();
    fs.add_dir(&path(STEAM));
    fs.add_file(
        &path(&format!("{STEAM}/steamapps/appmanifest_10.acf")),
        ACF.len() as u64,
        "-1d",
        Some(ACF.as_bytes()),
    );
    fs.add_file(
        &path(&format!("{STEAM}/userdata/111/config/localconfig.vdf")),
        7,
        "-1d",
        None,
    );
    fs
}

/// Records the launcher ids that collectors see.
struct Probe {
    seen: Mutex<Option<Vec<String>>>,
}

#[async_trait]
impl Collector for Probe {
    fn id(&self) -> &'static str {
        "probe"
    }

    fn display_key(&self) -> &'static str {
        "collector.probe"
    }

    async fn collect(&self, ctx: &CollectContext) -> Result<CollectOutput, CollectorError> {
        let ids = ctx.env.launchers.iter().map(|l| l.id.clone()).collect();
        *self.seen.lock().unwrap() = Some(ids);
        Ok(CollectOutput::default())
    }
}

fn pipeline(fs: MemFs, probe: Arc<Probe>) -> ScanPipeline {
    ScanPipeline::new(Arc::new(Config::default()))
        .with_environment(env())
        .with_scanner(Arc::new(fs))
        .with_rules_registry(Arc::new(MemRegistry::new()))
        .with_games_registry(Arc::new(MemRegistry::new()))
        .with_collector(probe)
}

async fn run(pipeline: &ScanPipeline) -> (ScanReport, Vec<Event>) {
    let (tx, mut rx) = unbounded_channel();
    let report = pipeline
        .run(ScanOptions::default(), tx, CancellationToken::new())
        .await
        .unwrap();
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    (report, events)
}

fn probe() -> Arc<Probe> {
    Arc::new(Probe {
        seen: Mutex::new(None),
    })
}

/// Index of the first event matching `pred`.
fn position(events: &[Event], pred: impl Fn(&Event) -> bool) -> usize {
    events.iter().position(pred).unwrap()
}

#[tokio::test]
async fn launchers_reach_collectors_report_and_rules() {
    let probe = probe();
    let (report, events) = run(&pipeline(steam_profile(), Arc::clone(&probe))).await;

    // Collectors and the report see the launchers found.
    assert_eq!(*probe.seen.lock().unwrap(), Some(vec!["steam".to_owned()]));
    let steam_root = PathTemplate::from_path(Path::new(&path(STEAM)), &env());
    assert_eq!(
        report.environment.launchers,
        [LauncherSnapshot {
            id: "steam".to_owned(),
            root: Some(steam_root),
            game_count: 1,
        }]
    );

    // The detector issue goes to the report and to the events, inside the
    // Environment phase.
    let issue = report
        .issues
        .iter()
        .find(|i| i.message_key == LIBRARYFOLDERS_KEY)
        .unwrap();
    assert_eq!(issue.severity, IssueSeverity::Info);
    assert_eq!(issue.source, "games.steam");
    let started = position(&events, |e| {
        matches!(
            e,
            Event::PhaseStarted {
                phase: ScanPhase::Environment
            }
        )
    });
    let sent = position(
        &events,
        |e| matches!(e, Event::Issue { issue: sent } if sent == issue),
    );
    let finished = position(&events, |e| {
        matches!(
            e,
            Event::PhaseFinished {
                phase: ScanPhase::Environment,
                ..
            }
        )
    });
    assert!(started < sent && sent < finished, "{events:?}");

    // `{STEAM}` and `{STEAM_USERID}` of the built-in rules now resolve.
    let roots: Vec<&str> = report
        .findings
        .iter()
        .filter(|f| {
            f.evidence.iter().any(|e| {
                matches!(&e.source, EvidenceSource::Rule { rule_id } if rule_id == "steam.userdata-config")
            })
        })
        .filter_map(|f| match &f.target {
            Target::FileSet { root, .. } => Some(root.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(roots, [r"{STEAM}\userdata\111\config"]);
}

#[tokio::test]
async fn nothing_installed_gives_no_launchers() {
    let probe = probe();
    let (report, _) = run(&pipeline(MemFs::new(), Arc::clone(&probe))).await;
    assert_eq!(*probe.seen.lock().unwrap(), Some(Vec::new()));
    assert!(report.environment.launchers.is_empty());
    assert!(
        report.issues.iter().all(|i| !i.source.starts_with("games")),
        "{:?}",
        report.issues
    );
}
