//! The system exports of SPEC-06 in `ScanPipeline` (T-06-14): installed
//! programs in the Environment phase and the built-in `system` collector.

// Helpers outside `#[test]` functions are not covered by `allow-unwrap-in-tests`.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use sk_core::collector::{CollectContext, CollectOutput, Collector};
use sk_core::config::Config;
use sk_core::env::{Environment, KnownFolder};
use sk_core::error::CollectorError;
use sk_core::events::Event;
use sk_core::model::{
    Category, CollectorToggles, Evidence, EvidenceSource, Finding, FindingId, IssueSeverity,
    RegHive, ScanReport, Sensitivity, Target,
};
use sk_core::registry::{KeyState, MemRegistry, RegValue, RegistryReader};
use sk_core::CancellationToken;
use sk_engine::{ScanOptions, ScanPipeline};
use sk_scan::MemFs;
use tokio::sync::mpsc::unbounded_channel;

const UNINSTALL: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";

fn env() -> Environment {
    Environment::fake(Path::new(if cfg!(windows) { r"C:\fake" } else { "/fake" }))
}

/// Absolute path of `rel` (`/`-separated) under `{APPDATA}` of the fake env.
fn app_data(rel: &str) -> String {
    let base = env()
        .known_folder(KnownFolder::AppData)
        .unwrap()
        .to_path_buf();
    let path = rel.split('/').fold(base, |p, c| p.join(c));
    path.to_string_lossy().into_owned()
}

/// A user rule `id` for `{APPDATA}\<folder>`, only when a program matching
/// `regex` is installed.
fn installed_rule(id: &str, folder: &str, regex: &str) -> String {
    format!(
        r"  - id: {id}
    app: {{ id: {id}, name: {folder}, kind: application }}
    category: app_config
    title_key: rules.{id}
    conditions:
      - installed: {{ display_name_regex: '{regex}' }}
    targets:
      - path: '{{APPDATA}}\{folder}'
"
    )
}

/// A rules folder with `user.obs` (OBS Studio installed) and `user.krita`
/// (Krita installed, which it is not).
fn rules_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let yaml = format!(
        "schema_version: 1\nrules:\n{}{}",
        installed_rule("user.obs", "SkTestObs", "(?i)^obs studio"),
        installed_rule("user.krita", "SkTestKrita", "(?i)^krita"),
    );
    std::fs::write(dir.path().join("installed.yaml"), yaml).unwrap();
    dir
}

/// A profile with the settings of both rules.
fn profile() -> MemFs {
    let mut fs = MemFs::new();
    fs.add_file(&app_data("SkTestObs/global.ini"), 10, "-1d", None);
    fs.add_file(&app_data("SkTestKrita/kritarc"), 20, "-1d", None);
    fs
}

/// One installed program, OBS Studio, in the 64-bit Uninstall key.
fn registry() -> MemRegistry {
    let key = format!(r"{UNINSTALL}\OBS Studio");
    let mut registry = MemRegistry::new();
    registry
        .set_string(RegHive::Hklm, &key, "DisplayName", "OBS Studio")
        .set_string(RegHive::Hklm, &key, "Publisher", "OBS Project");
    registry
}

/// Records the installed programs that collectors see.
#[derive(Default)]
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
        let names = ctx
            .env
            .installed_programs
            .iter()
            .map(|p| p.name.clone())
            .collect();
        *self.seen.lock().unwrap() = Some(names);
        Ok(CollectOutput::default())
    }
}

fn pipeline(
    rules_dir: &Path,
    registry: Arc<dyn RegistryReader>,
    probe: &Arc<Probe>,
) -> ScanPipeline {
    ScanPipeline::new(Arc::new(Config::default()))
        .with_environment(env())
        .with_scanner(Arc::new(profile()))
        .with_rules_dir(rules_dir.to_path_buf())
        .with_rules_registry(Arc::new(MemRegistry::new()))
        .with_games_registry(Arc::new(MemRegistry::new()))
        .with_system_registry(registry)
        .with_collector(Arc::clone(probe) as Arc<dyn Collector>)
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

fn system_off() -> ScanOptions {
    ScanOptions {
        collectors: CollectorToggles {
            system: false,
            ..CollectorToggles::default()
        },
        ..ScanOptions::default()
    }
}

/// Ids of the rules behind the findings, sorted.
fn rule_ids(report: &ScanReport) -> Vec<String> {
    let mut ids: Vec<String> = report
        .findings
        .iter()
        .flat_map(|f: &Finding| &f.evidence)
        .filter_map(|e| match &e.source {
            EvidenceSource::Rule { rule_id } => Some(rule_id.clone()),
            _ => None,
        })
        .collect();
    ids.sort();
    ids
}

/// Exporter ids of the `SystemExport` findings, sorted.
fn exporter_ids(report: &ScanReport) -> Vec<String> {
    let mut ids: Vec<String> = report
        .findings
        .iter()
        .filter_map(|f| match &f.target {
            Target::SystemExport { exporter_id, .. } => Some(exporter_id.clone()),
            _ => None,
        })
        .collect();
    ids.sort();
    ids
}

fn seen(probe: &Probe) -> Option<Vec<String>> {
    probe.seen.lock().unwrap().clone()
}

#[tokio::test]
async fn installed_programs_reach_collectors_and_installed_rules() {
    let dir = rules_dir();
    let probe = Arc::new(Probe::default());
    let pipeline = pipeline(dir.path(), Arc::new(registry()), &probe);
    let (report, _) = run(&pipeline, ScanOptions::default()).await;

    assert_eq!(seen(&probe), Some(vec!["OBS Studio".to_owned()]));
    assert_eq!(rule_ids(&report), ["user.obs"]);
    assert!(
        report.issues.iter().all(|i| i.source != "system"),
        "{:?}",
        report.issues
    );
    // The built-in `system` collector (`SystemCollector::new()`): on Windows
    // at least the checklist is always available, elsewhere nothing is.
    let exporters = exporter_ids(&report);
    assert_eq!(
        exporters.iter().any(|id| id == "checklist"),
        cfg!(windows),
        "{exporters:?}"
    );
    for finding in report
        .findings
        .iter()
        .filter(|f| matches!(f.target, Target::SystemExport { .. }))
    {
        assert!(finding
            .evidence
            .iter()
            .any(|e| matches!(e.source, EvidenceSource::System { .. })));
    }
}

/// The installed programs do not depend on the `system` toggle.
#[tokio::test]
async fn system_toggle_off_drops_exports_but_keeps_installed_programs() {
    let dir = rules_dir();
    let probe = Arc::new(Probe::default());
    let pipeline = pipeline(dir.path(), Arc::new(registry()), &probe);
    let (report, _) = run(&pipeline, system_off()).await;

    assert!(exporter_ids(&report).is_empty(), "{:?}", report.findings);
    assert!(report.findings.iter().all(|f| f
        .evidence
        .iter()
        .all(|e| !matches!(e.source, EvidenceSource::System { .. }))));
    assert_eq!(seen(&probe), Some(vec!["OBS Studio".to_owned()]));
    assert_eq!(rule_ids(&report), ["user.obs"]);
}

/// A fake system collector with one export `fake`.
struct Replacement;

#[async_trait]
impl Collector for Replacement {
    fn id(&self) -> &'static str {
        "system"
    }

    fn display_key(&self) -> &'static str {
        "collector.system"
    }

    async fn collect(&self, _: &CollectContext) -> Result<CollectOutput, CollectorError> {
        Ok(CollectOutput {
            findings: vec![fake_export()],
            ..CollectOutput::default()
        })
    }
}

fn fake_export() -> Finding {
    let target = Target::SystemExport {
        exporter_id: "fake".to_owned(),
        params: serde_json::Value::Null,
    };
    Finding {
        id: FindingId::for_target(&target),
        target,
        category: Category::SystemSettings,
        app: None,
        title: "fake".to_owned(),
        evidence: vec![Evidence {
            source: EvidenceSource::System {
                exporter_id: "fake".to_owned(),
            },
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

/// `with_collector` with the id `system` replaces the built-in one; the
/// installed programs are still filled, and the toggle applies to it.
#[tokio::test]
async fn a_system_collector_replaces_the_built_in_one() {
    let dir = rules_dir();
    let probe = Arc::new(Probe::default());
    let pipeline =
        pipeline(dir.path(), Arc::new(registry()), &probe).with_collector(Arc::new(Replacement));

    let (report, _) = run(&pipeline, ScanOptions::default()).await;
    assert_eq!(exporter_ids(&report), ["fake"]);
    assert_eq!(seen(&probe), Some(vec!["OBS Studio".to_owned()]));

    let (report, _) = run(&pipeline, system_off()).await;
    assert!(exporter_ids(&report).is_empty());
}

/// A registry whose every read panics.
struct PanickingRegistry;

impl RegistryReader for PanickingRegistry {
    fn key_state(&self, _: RegHive, _: &str) -> KeyState {
        panic!("registry exploded")
    }
    fn string_value(&self, _: RegHive, _: &str, _: &str) -> Option<String> {
        panic!("registry exploded")
    }
    fn dword_value(&self, _: RegHive, _: &str, _: &str) -> Option<u32> {
        panic!("registry exploded")
    }
    fn subkeys(&self, _: RegHive, _: &str) -> Vec<String> {
        panic!("registry exploded")
    }
    fn values(&self, _: RegHive, _: &str) -> Option<Vec<RegValue>> {
        panic!("registry exploded")
    }
}

/// A panicking enricher leaves no installed programs and gives an issue,
/// the scan goes on.
#[tokio::test]
async fn a_panicking_enricher_gives_an_issue_and_no_programs() {
    let dir = rules_dir();
    let probe = Arc::new(Probe::default());
    let pipeline = pipeline(dir.path(), Arc::new(PanickingRegistry), &probe);
    let (report, events) = run(&pipeline, system_off()).await;

    assert_eq!(seen(&probe), Some(Vec::new()));
    assert!(rule_ids(&report).is_empty());
    let issues: Vec<_> = report
        .issues
        .iter()
        .filter(|i| i.source == "system")
        .collect();
    assert_eq!(issues.len(), 1, "{:?}", report.issues);
    let issue = issues[0];
    assert_eq!(issue.severity, IssueSeverity::Error);
    assert_eq!(issue.message_key, "collector.panicked");
    assert!(issue.message_args.contains_key("error"));
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::Issue { issue: sent } if sent == issue)));
}
