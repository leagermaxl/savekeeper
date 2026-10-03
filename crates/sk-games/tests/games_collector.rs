//! `GamesCollector` on `fixtures/fs/gamer-profile` with the 20-game manifest
//! excerpt (SPEC-05 §6, T-05-09): launchers detected by `enrich`, the
//! manifest read from the cache of a `ManifestStore`, findings compared with
//! an `insta` snapshot.

// Helpers outside `#[test]` functions are not covered by `allow-unwrap-in-tests`.
#![allow(clippy::unwrap_used)]

use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use sk_core::collector::{CollectContext, CollectOutput, Collector};
use sk_core::config::Config;
use sk_core::env::Environment;
use sk_core::model::{Finding, RegHive, Target};
use sk_core::registry::MemRegistry;
use sk_core::template::PathTemplate;
use sk_core::CancellationToken;
use sk_games::{enrich_with_registry, GamesCollector, ManifestStore};
use sk_scan::MemFs;
use sk_testkit::mem_fixture;

const MINI: &str = include_str!("../../../fixtures/samples/ludusavi/manifest-mini.yaml");

fn root() -> &'static Path {
    Path::new(if cfg!(windows) { r"C:\fake" } else { "/fake" })
}

/// The fixture with its launchers detected and the Epic game added.
fn profile() -> (MemFs, Environment) {
    let (mut fs, mut env) = mem_fixture("gamer-profile", root());
    let program_data = env.known_folders[&sk_core::env::KnownFolder::ProgramData].clone();
    let program_files = env.known_folders[&sk_core::env::KnownFolder::ProgramFiles].clone();
    let install = program_files
        .join("Epic Games")
        .join("SatisfactoryEarlyAccess");
    let item = format!(
        r#"{{"AppName": "CrabEA", "DisplayName": "Satisfactory", "InstallLocation": {}, "InstallSize": 21470562304}}"#,
        serde_json::to_string(&install.to_string_lossy()).unwrap()
    );
    let manifests = ["Epic", "EpicGamesLauncher", "Data", "Manifests", "ABC.item"]
        .iter()
        .fold(program_data, |p, c| p.join(c));
    fs.add_file(
        &manifests.to_string_lossy(),
        item.len() as u64,
        "-1d",
        Some(item.as_bytes()),
    );
    let issues = enrich_with_registry(&mut env, &fs, Arc::new(MemRegistry::new()));
    // Only `libraryfolders.vdf` is missing (main library only).
    assert_eq!(issues.len(), 1, "{issues:?}");
    (fs, env)
}

async fn collect(fs: MemFs, env: Environment, registry: MemRegistry) -> CollectOutput {
    let data = tempfile::tempdir().unwrap();
    let cache = data.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    fs::write(cache.join("ludusavi-manifest.yaml"), MINI).unwrap();
    let mut config = Config::default();
    config.games.auto_update = false;
    let store = ManifestStore::new(&config, data.path());
    let collector = GamesCollector::new(store)
        .with_registry(Arc::new(registry))
        .with_network(false);
    assert_eq!(collector.id(), "games");
    let (events, _rx) = tokio::sync::mpsc::unbounded_channel();
    let ctx = CollectContext {
        env: Arc::new(env),
        config: Arc::new(config),
        scanner: Arc::new(fs),
        events,
        cancel: CancellationToken::new(),
    };
    collector.collect(&ctx).await.unwrap()
}

/// One line per finding, without machine-specific paths.
fn describe(finding: &Finding) -> String {
    let (kind, place, include) = match &finding.target {
        Target::FileSet { root, include, .. } => ("dir", root.to_string(), include.join(",")),
        Target::File { path, .. } => ("file", path.to_string(), String::new()),
        Target::Registry { hive, key, .. } => ("reg", format!("{hive:?}\\{key}"), String::new()),
        Target::SystemExport { exporter_id, .. } => ("sys", exporter_id.clone(), String::new()),
    };
    let app = finding.app.as_ref().map_or_else(String::new, |a| {
        format!("{} installed={:?}", a.id, a.installed)
    });
    let evidence: Vec<String> = finding
        .evidence
        .iter()
        .map(|e| format!("{} {:.2}", e.message_key, e.confidence))
        .collect();
    format!(
        "{:?} {kind} {place} [{include}]\n  title: {}\n  app: {app}\n  tags: {}\n  evidence: {}",
        finding.category,
        finding.title,
        finding.tags.join(","),
        evidence.join("; ")
    )
}

#[tokio::test]
async fn gamer_profile_findings() {
    let (fs, env) = profile();
    let mut registry = MemRegistry::new();
    registry.add_key(RegHive::Hkcu, r"Software\Team Cherry\Hollow Knight");
    let out = collect(fs, env.clone(), registry).await;

    let mut text = String::new();
    for finding in &out.findings {
        writeln!(text, "{}", describe(finding)).unwrap();
    }
    writeln!(text, "claimed:").unwrap();
    for path in &out.claimed_paths {
        writeln!(text, "  {}", PathTemplate::from_path(path, &env)).unwrap();
    }
    writeln!(text, "issues:").unwrap();
    for issue in &out.issues {
        writeln!(text, "  {} {}", issue.source, issue.message_key).unwrap();
    }
    insta::assert_snapshot!("gamer_profile", text);
}

#[tokio::test]
async fn cancelled_collection_is_empty() {
    let (fs, env) = profile();
    let data = tempfile::tempdir().unwrap();
    let mut config = Config::default();
    config.games.auto_update = false;
    let collector = GamesCollector::new(ManifestStore::new(&config, data.path()));
    let (events, _rx) = tokio::sync::mpsc::unbounded_channel();
    let ctx = CollectContext {
        env: Arc::new(env),
        config: Arc::new(config),
        scanner: Arc::new(fs),
        events,
        cancel: CancellationToken::new(),
    };
    ctx.cancel.cancel();
    assert_eq!(
        collector.collect(&ctx).await.unwrap(),
        CollectOutput::default()
    );
}
