//! H-USR on `fixtures/fs/heuristics/usr-walk.yaml` (T-07-08): alone, and
//! after H-UNK in the collector, where it only adds what H-UNK left.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sk_core::collector::CollectContext;
use sk_core::config::{Config, HeuristicsConfig};
use sk_core::env::{Environment, InstalledGame, InstalledProgram, KnownFolder, ProgramSource};
use sk_core::model::{Category, Finding};
use sk_core::path::{starts_with_ci, PathSet};
use sk_core::CancellationToken;
use sk_scan::MemFs;
use tokio::sync::mpsc::unbounded_channel;

use super::tests::{exclude_of, found, rel, root, Case};
use super::*;
use crate::collector::run;
use crate::unk::HUnk;

/// The fixture without drives, with a Steam game in `{HOME}\SteamLibrary`
/// and a program installed in `{HOME}\Tools\Paint` (as in the walk tests).
fn fixture() -> (MemFs, Environment) {
    let (fs, mut env) = sk_testkit::mem_fixture("heuristics/usr-walk", root());
    env.drives.clear();
    let home = env
        .known_folder(KnownFolder::Home)
        .map(Path::to_path_buf)
        .unwrap_or_default();
    if let Some(steam) = env.launchers.iter_mut().find(|l| l.id == "steam") {
        steam.games.push(InstalledGame {
            store_game_id: "1145360".to_owned(),
            name: "Hades".to_owned(),
            install_dir: home.join("SteamLibrary/steamapps/common/Hades"),
            size_bytes: None,
            manifest_key: None,
        });
    }
    env.installed_programs.push(InstalledProgram {
        name: "Paint".to_owned(),
        publisher: None,
        version: None,
        install_location: Some(home.join("Tools/Paint")),
        install_date: None,
        estimated_size_kb: None,
        source: ProgramSource::Hkcu,
        uninstall_key: "Paint".to_owned(),
    });
    (fs, env)
}

fn entry(path: &str, category: Category, reason: &str) -> (String, Category, String) {
    (
        format!("fake/Users/user/{path}"),
        category,
        reason.to_owned(),
    )
}

#[test]
fn fixture_gives_the_expected_findings() {
    let (fs, env) = fixture();
    let mut case = Case::new();
    case.env = env;
    let out = case.run(&fs);
    assert!(out.issues.is_empty(), "{:?}", out.issues);
    let valuable = "heuristic.usr.valuable_file";
    // Work: ClientA dominates (the RAW photo weighs 50 of 54.4), then raw.
    // The PST file lies in Documents, which is one finding.
    assert_eq!(
        found(&out),
        [
            entry("Documents", Category::UserFiles, REASON_USER_FOLDER),
            entry("Work/ClientA/raw", Category::UserFiles, REASON_CLUSTER),
            entry("Secrets/id_rsa.pem", Category::Credentials, valuable),
            entry("Secrets/vault.kdbx", Category::Credentials, valuable),
        ]
    );
    let documents = &out.findings[0];
    assert_eq!(documents.tags, ["cloud-synced"]);
    let args = &documents.evidence[0].message_args;
    assert_eq!(args["group1"], "ebook_mail");
    assert_eq!(args["group2"], "document");
    assert_eq!(args["cloud_only_files"], "1");
    assert_eq!(
        out.findings[1].evidence[0].message_args["group1"],
        "photo_raw"
    );
}

#[test]
fn after_h_unk_only_the_rest_is_added() {
    let (fs, env) = fixture();
    let mut config = Config::default();
    config.scan.extra_roots = vec![root().to_path_buf()];
    let (events, _rx) = unbounded_channel();
    let ctx = CollectContext {
        env: Arc::new(env),
        config: Arc::new(config),
        scanner: Arc::new(fs),
        events,
        cancel: CancellationToken::new(),
    };
    let heuristics: Vec<Box<dyn Heuristic>> = vec![Box::new(HUnk::new()), Box::new(HUsr::new())];
    let out = run(
        &heuristics,
        &HeuristicsConfig::default(),
        &ctx,
        Vec::new(),
        PathSet::new(),
    );
    let usr: Vec<&Finding> = out
        .findings
        .iter()
        .filter(|f| f.evidence[0].source == heuristic_source())
        .collect();
    // H-UNK took the folders of `{HOME}` and `{DOCUMENTS}\Outlook`; the
    // loose file of Documents is left.
    let paths: Vec<String> = usr.iter().map(|f| rel(f)).collect();
    assert_eq!(paths, ["fake/Users/user/Documents"]);
    assert_eq!(exclude_of(usr[0]), ["Outlook"]);
    assert_eq!(usr[0].evidence[0].message_args["files"], "1");

    // No H-USR finding lies inside another finding.
    let roots: Vec<PathBuf> = out.findings.iter().map(abs).collect();
    for finding in &usr {
        let path = abs(finding);
        let inside = roots
            .iter()
            .filter(|other| starts_with_ci(&path, other))
            .count();
        assert_eq!(inside, 1, "{path:?}");
    }
}

fn heuristic_source() -> sk_core::model::EvidenceSource {
    sk_core::model::EvidenceSource::Heuristic {
        heuristic_id: HEURISTIC_ID.to_owned(),
    }
}

fn abs(f: &Finding) -> PathBuf {
    match &f.target {
        sk_core::model::Target::FileSet { resolved, .. }
        | sk_core::model::Target::File { resolved, .. } => resolved.clone(),
        other => panic!("{other:?}"),
    }
}
