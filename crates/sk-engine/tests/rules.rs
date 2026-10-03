//! The built-in `rules` collector of `ScanPipeline` (SPEC-04 T-04-13).

// Helpers outside `#[test]` functions are not covered by `allow-unwrap-in-tests`.
#![allow(clippy::unwrap_used)]

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use sk_core::collector::{CollectContext, CollectOutput, Collector};
use sk_core::config::Config;
use sk_core::env::{Environment, KnownFolder};
use sk_core::error::CollectorError;
use sk_core::events::Event;
use sk_core::model::{
    CollectorToggles, EvidenceSource, Finding, IssueSeverity, ScanIssue, ScanReport, Target,
};
use sk_core::registry::MemRegistry;
use sk_core::CancellationToken;
use sk_engine::{ScanOptions, ScanPipeline};
use sk_scan::MemFs;
use tokio::sync::mpsc::unbounded_channel;

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

/// A user rule file with one rule `id` for the folder `{APPDATA}\<folder>`.
fn rule_file(id: &str, folder: &str) -> String {
    format!(
        r"schema_version: 1
rules:
  - id: {id}
    app: {{ id: {id}, name: {folder}, kind: application }}
    category: app_config
    title_key: rules.{id}
    targets:
      - path: '{{APPDATA}}\{folder}'
"
    )
}

/// A profile with `{APPDATA}\Alpha` and `{APPDATA}\Beta`.
fn profile() -> MemFs {
    let mut fs = MemFs::new();
    fs.add_file(&app_data("Alpha/settings.json"), 10, "-1d", None);
    fs.add_file(&app_data("Beta/beta.ini"), 20, "-2d", None);
    fs
}

fn pipeline(rules_dir: &Path) -> ScanPipeline {
    ScanPipeline::new(Arc::new(Config::default()))
        .with_environment(env())
        .with_scanner(Arc::new(profile()))
        .with_rules_dir(rules_dir.to_path_buf())
        .with_rules_registry(Arc::new(MemRegistry::new()))
        .with_games_registry(Arc::new(MemRegistry::new()))
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

fn invalid_file_issues(issues: &[ScanIssue]) -> Vec<&ScanIssue> {
    issues
        .iter()
        .filter(|i| i.message_key == "issue.rules.invalid_file")
        .collect()
}

#[tokio::test]
async fn user_rules_give_findings_and_a_broken_file_an_issue() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("alpha.yaml"),
        rule_file("user.alpha", "Alpha"),
    )
    .unwrap();
    std::fs::write(dir.path().join("broken.yaml"), "rules: [ {").unwrap();

    let (report, events) = run(&pipeline(dir.path()), ScanOptions::default()).await;

    assert_eq!(rule_ids(&report), ["user.alpha"]);
    let finding = &report.findings[0];
    match &finding.target {
        Target::FileSet { root, .. } => assert_eq!(root.as_str(), r"{APPDATA}\Alpha"),
        other => panic!("unexpected target {other:?}"),
    }
    assert_eq!(finding.stats.as_ref().unwrap().total_bytes, 10, "measured");

    let issues = invalid_file_issues(&report.issues);
    assert_eq!(issues.len(), 1, "{:?}", report.issues);
    let issue = issues[0];
    assert_eq!(issue.source, "rules");
    assert_eq!(issue.severity, IssueSeverity::Warning);
    assert_eq!(issue.message_args["file"], "broken.yaml");
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::Issue { issue: sent } if sent == issue)));
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::FindingsAdded { .. })));
}

/// The rules are loaded for each run: a new user file needs no restart.
#[tokio::test]
async fn user_rules_are_reloaded_for_each_run() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("alpha.yaml"),
        rule_file("user.alpha", "Alpha"),
    )
    .unwrap();
    let pipeline = pipeline(dir.path());

    let (first, _) = run(&pipeline, ScanOptions::default()).await;
    assert_eq!(rule_ids(&first), ["user.alpha"]);
    assert!(first.issues.is_empty(), "{:?}", first.issues);

    std::fs::write(dir.path().join("beta.yaml"), rule_file("user.beta", "Beta")).unwrap();
    let (second, _) = run(&pipeline, ScanOptions::default()).await;
    assert_eq!(rule_ids(&second), ["user.alpha", "user.beta"]);
}

/// A missing `rules.d` is not a problem.
#[tokio::test]
async fn missing_rules_dir_gives_no_issue() {
    let dir = tempfile::tempdir().unwrap();
    let (report, _) = run(
        &pipeline(&dir.path().join("rules.d")),
        ScanOptions::default(),
    )
    .await;
    assert!(rule_ids(&report).is_empty());
    assert!(report.issues.is_empty(), "{:?}", report.issues);
}

/// With the `rules` toggle off the rules are not even loaded.
#[tokio::test]
async fn toggle_off_skips_loading() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("alpha.yaml"),
        rule_file("user.alpha", "Alpha"),
    )
    .unwrap();
    std::fs::write(dir.path().join("broken.yaml"), "rules: [ {").unwrap();
    let opts = ScanOptions {
        collectors: CollectorToggles {
            rules: false,
            ..CollectorToggles::default()
        },
        ..ScanOptions::default()
    };
    let (report, _) = run(&pipeline(dir.path()), opts).await;
    assert!(report.findings.is_empty());
    assert!(report.issues.is_empty(), "{:?}", report.issues);
}

struct Replacement;

#[async_trait]
impl Collector for Replacement {
    fn id(&self) -> &'static str {
        "rules"
    }
    fn display_key(&self) -> &'static str {
        "collector.rules"
    }
    async fn collect(&self, _: &CollectContext) -> Result<CollectOutput, CollectorError> {
        Ok(CollectOutput::default())
    }
}

/// `with_collector` with the id `rules` replaces the built-in rules collector.
#[tokio::test]
async fn a_rules_collector_replaces_the_built_in_one() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("alpha.yaml"),
        rule_file("user.alpha", "Alpha"),
    )
    .unwrap();
    std::fs::write(dir.path().join("broken.yaml"), "rules: [ {").unwrap();
    let pipeline = pipeline(dir.path()).with_collector(Arc::new(Replacement));
    let (report, _) = run(&pipeline, ScanOptions::default()).await;
    assert!(report.findings.is_empty());
    assert!(report.issues.is_empty(), "{:?}", report.issues);
}
