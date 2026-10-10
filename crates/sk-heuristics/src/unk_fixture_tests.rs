//! FR-07-02 on the fixture `fixtures/fs/heuristics/unk-profile.yaml`: every
//! top-level folder of the search zones is claimed or gives exactly one
//! finding, and the summaries of the `Unknown` ones leave the collector.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sk_core::collector::{CollectContext, PostCollector, PriorResults};
use sk_core::config::{Config, HeuristicsConfig};
use sk_core::env::{Environment, KnownFolder};
use sk_core::fs::{EntryKind, FsScanner};
use sk_core::model::{Category, Finding, Target};
use sk_core::path::{eq_ci, starts_with_ci, PathSet};
use sk_core::CancellationToken;
use sk_scan::MemFs;
use sk_testkit::mem_fixture;
use tokio::sync::mpsc::unbounded_channel;

use super::HUnk;
use crate::collector::{run, HeuristicsCollector};
use crate::data;
use crate::heuristic::Heuristic;

fn root() -> &'static Path {
    Path::new(if cfg!(windows) { r"C:\fake" } else { "/fake" })
}

fn fixture() -> (MemFs, Environment) {
    mem_fixture("heuristics/unk-profile", root())
}

fn context(fs: Arc<MemFs>, env: Environment) -> CollectContext {
    let (events, _rx) = unbounded_channel();
    CollectContext {
        env: Arc::new(env),
        config: Arc::new(Config::default()),
        scanner: fs,
        events,
        cancel: CancellationToken::new(),
    }
}

fn under(env: &Environment, folder: KnownFolder, rel: &str) -> PathBuf {
    let base = env.known_folder(folder).unwrap().to_path_buf();
    rel.split('/').fold(base, |p, c| p.join(c))
}

/// What the rules would have claimed.
fn prior_claims(env: &Environment) -> PathSet {
    [
        under(env, KnownFolder::AppData, "Code"),
        under(env, KnownFolder::AppData, "Tool/config"),
    ]
    .into_iter()
    .collect()
}

fn heuristics() -> Vec<Box<dyn Heuristic>> {
    vec![Box::new(HUnk::new())]
}

fn root_of(f: &Finding) -> &Path {
    match &f.target {
        Target::FileSet { resolved, .. } | Target::File { resolved, .. } => resolved,
        other => panic!("unexpected target {other:?}"),
    }
}

/// Folders (by `MemFs` listing) of `dir`, without links.
fn dirs(fs: &MemFs, dir: &Path) -> Vec<PathBuf> {
    fs.read_dir(dir)
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e.meta.kind == EntryKind::Dir)
        .map(|e| e.path)
        .collect()
}

fn name(path: &Path) -> String {
    path.file_name().unwrap().to_string_lossy().into_owned()
}

/// The top-level entries of the zones of SPEC-07 §4.2.1, listed independently
/// of H-UNK: vendor containers and `Packages` are replaced by their children,
/// `{LOCALLOW}` gives `Company\Product`, the profile root its folders that hold
/// no known folder and its dot files.
fn zone_entries(fs: &MemFs, env: &Environment) -> Vec<PathBuf> {
    let vendors = &data::builtin().unwrap().vendors;
    let known: Vec<&Path> = env.known_folders.values().map(PathBuf::as_path).collect();
    let mut out = Vec::new();
    for folder in [
        KnownFolder::AppData,
        KnownFolder::LocalAppData,
        KnownFolder::LocalLow,
        KnownFolder::ProgramData,
        KnownFolder::Documents,
        KnownFolder::SavedGames,
        KnownFolder::Home,
    ] {
        let zone = env.known_folder(folder).unwrap();
        for dir in dirs(fs, zone) {
            if known.iter().any(|k| starts_with_ci(k, &dir)) {
                continue;
            }
            let vendored = matches!(folder, KnownFolder::AppData | KnownFolder::LocalAppData)
                && vendors.contains(&name(&dir));
            if vendored || folder == KnownFolder::LocalLow {
                out.extend(dirs(fs, &dir));
            } else {
                out.push(dir);
            }
        }
        if folder == KnownFolder::Home {
            let dot_files = fs
                .read_dir(zone)
                .unwrap()
                .into_iter()
                .filter(|e| e.meta.kind == EntryKind::File && name(&e.path).starts_with('.'));
            out.extend(dot_files.map(|e| e.path));
        }
    }
    out
}

/// Zone entries that by SPEC-07 give neither a finding nor a claim.
const SKIPPED: [&str; 6] = [
    r"{LOCALAPPDATA}\Temp",
    r"{LOCALAPPDATA}\CrashDumps",
    r"{PACKAGE:Microsoft.Windows.Photos}",
    r"{PACKAGE:MicrosoftWindows.Client.CBS}",
    r"{PROGRAMDATA}\Stale",
    r"{PROGRAMDATA}\Agent",
];

#[test]
fn every_zone_folder_is_claimed_or_found_once() {
    let (fs, env) = fixture();
    let fs = Arc::new(fs);
    let prior = prior_claims(&env);
    let ctx = context(Arc::clone(&fs), env.clone());
    let cfg = HeuristicsConfig::default();
    let out = run(&heuristics(), &cfg, &ctx, Vec::new(), prior.clone());
    assert!(out.issues.is_empty(), "{:?}", out.issues);

    let mut claimed = prior;
    claimed.extend(out.claimed_paths.iter().cloned());
    let entries = zone_entries(&fs, &env);
    assert!(entries.len() > 25, "{entries:?}");
    for entry in &entries {
        let template = sk_core::template::PathTemplate::from_path(entry, &env);
        let found = out
            .findings
            .iter()
            .filter(|f| eq_ci(root_of(f), entry))
            .count();
        if SKIPPED.contains(&template.as_str()) {
            assert_eq!(found, 0, "{template}");
            assert!(!claimed.covers(entry), "{template}");
            continue;
        }
        assert!(found <= 1, "{template}: {found} findings");
        assert!(
            claimed.covers(entry),
            "{template} is neither found nor claimed"
        );
    }
    // No finding outside the zone entries.
    for f in &out.findings {
        assert!(
            entries.iter().any(|e| eq_ci(e, root_of(f))),
            "unexpected finding {}",
            f.title
        );
    }
    // Junctions are not candidates.
    let junction = under(&env, KnownFolder::Home, "Application Data");
    assert!(!claimed.covers(&junction));
}

#[test]
fn fixture_findings_are_classified() {
    let (fs, env) = fixture();
    let ctx = context(Arc::new(fs), env.clone());
    let out = run(
        &heuristics(),
        &HeuristicsConfig::default(),
        &ctx,
        Vec::new(),
        prior_claims(&env),
    );
    let mut found: Vec<(String, Category)> = out
        .findings
        .iter()
        .map(|f| match &f.target {
            Target::FileSet { root, .. } => (root.as_str().to_owned(), f.category),
            Target::File { path, .. } => (path.as_str().to_owned(), f.category),
            other => panic!("{other:?}"),
        })
        .collect();
    found.sort();
    let expected = [
        (r"{APPDATA}\Adobe\Common", Category::Cache),
        (r"{APPDATA}\Adobe\Lightroom", Category::AppConfig),
        (r"{APPDATA}\Adobe\Logs", Category::Cache),
        (r"{APPDATA}\Mystery", Category::Unknown),
        (r"{APPDATA}\Notepad4", Category::AppConfig),
        (r"{APPDATA}\Tool", Category::Unknown),
        (r"{DOCUMENTS}\Klei", Category::AppConfig),
        (r"{DOCUMENTS}\Rockstar Games", Category::Unknown),
        (r"{HOME}\.cargo", Category::DevEnvironment),
        (r"{HOME}\.gitconfig", Category::DevEnvironment),
        (r"{HOME}\Zomboid", Category::Unknown),
        (r"{HOME}\source", Category::DevEnvironment),
        (r"{LOCALAPPDATA}\Discord", Category::Reinstallable),
        (r"{LOCALAPPDATA}\Microsoft\Edge", Category::Unknown),
        (r"{LOCALAPPDATA}\Microsoft\Windows", Category::Unknown),
        (r"{LOCALAPPDATA}\Programs\Obsidian", Category::Reinstallable),
        (r"{LOCALAPPDATA}\SquirrelTemp", Category::Cache),
        (r"{LOCALLOW}\Mojang\Minecraft Dungeons", Category::GameSave),
        (r"{LOCALLOW}\Studio\Tool", Category::Unknown),
        (r"{PACKAGE:Microsoft.SunriseBaseGame}", Category::GameSave),
        (r"{PACKAGE:SpotifyAB.SpotifyMusic}", Category::Unknown),
        (r"{PROGRAMDATA}\Recent", Category::AppConfig),
        (r"{SAVED_GAMES}\Frontier Developments", Category::GameSave),
    ];
    let expected: Vec<(String, Category)> = expected
        .iter()
        .map(|(t, c)| ((*t).to_owned(), *c))
        .collect();
    assert_eq!(found, expected);
    let tool = out.findings.iter().find(|f| f.title == "Tool").unwrap();
    let Target::FileSet { exclude, .. } = &tool.target else {
        panic!()
    };
    assert_eq!(exclude, &["config"]);
}

#[tokio::test]
async fn collector_passes_unknown_summaries_on() {
    let (fs, env) = fixture();
    let ctx = context(Arc::new(fs), env.clone());
    let claimed = prior_claims(&env);
    let collector = HeuristicsCollector::new(HeuristicsConfig::default());
    let prior = PriorResults {
        findings: &[],
        claimed: &claimed,
    };
    let out = collector.collect(&ctx, &prior).await.unwrap();
    let unknown: Vec<_> = out
        .findings
        .iter()
        .filter(|f| f.category == Category::Unknown)
        .map(|f| match &f.target {
            Target::FileSet { root, .. } => root.as_str().to_owned(),
            other => panic!("{other:?}"),
        })
        .collect();
    let summaries: Vec<_> = out
        .unknown_summaries
        .iter()
        .map(|s| s.path.as_str().to_owned())
        .collect();
    assert_eq!(summaries, unknown);
    assert!(summaries.len() >= 5, "{summaries:?}");

    // The same file system gives the same output.
    let again = collector.collect(&ctx, &prior).await.unwrap();
    assert_eq!(again, out);
}
