//! Tests of H-WEB (SPEC-07 §4.6) on small `MemFs` trees: detection, claims,
//! extensions, credentials, edge cases. Shared helpers of the H-WEB tests.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sk_core::config::HeuristicsConfig;
use sk_core::env::{Environment, KnownFolder};
use sk_core::fs::{
    DirEntryInfo, EntryMeta, FsError, FsScanner, Readability, WalkControl, WalkOptions, WalkStats,
};
use sk_core::model::{Category, Finding, IssueSeverity, Sensitivity, Target};
use sk_core::path::{eq_ci, PathSet};
use sk_core::CancellationToken;
use sk_scan::{ExcludeSet, MemFs};

use super::*;
use crate::heuristic::{Heuristic, HeuristicContext, HeuristicOutput};
use crate::walk::RootsWalkCache;

pub(super) fn env() -> Environment {
    Environment::fake(Path::new(if cfg!(windows) { r"C:\fake" } else { "/fake" }))
}

/// `rel` (with `/`) under a known folder of `env`.
pub(super) fn under(env: &Environment, folder: KnownFolder, rel: &str) -> PathBuf {
    let base = env.known_folder(folder).unwrap().to_path_buf();
    rel.split('/').fold(base, |p, c| p.join(c))
}

fn add(fs: &mut MemFs, rel: &str) {
    let path = under(&env(), KnownFolder::AppData, rel);
    fs.add_file(path.to_str().unwrap(), 100, "-1d", None);
}

/// An Electron app `{APPDATA}\<name>` with data, settings and caches.
fn electron(fs: &mut MemFs, name: &str) {
    for rel in [
        "Local Storage/leveldb/CURRENT",
        "IndexedDB/app_0.indexeddb.leveldb/000005.ldb",
        "Cache/Cache_Data/f_000001",
        "GPUCache/data_0",
        "config.json",
    ] {
        add(fs, &format!("{name}/{rel}"));
    }
}

/// Runs H-WEB on `fs` with `claimed` and `cfg`.
pub(super) fn run_web(
    fs: &dyn FsScanner,
    env: &Environment,
    claimed: &PathSet,
    cfg: &HeuristicsConfig,
    cancel: &CancellationToken,
) -> HeuristicOutput {
    let excludes = Arc::new(ExcludeSet::builtin(env));
    let hctx = HeuristicContext {
        env,
        scanner: fs,
        excludes: &excludes,
        claimed,
        prior: &[],
        cfg,
        cancel,
        progress: &|_, _| {},
        roots_walk: &RootsWalkCache::default(),
    };
    HWeb::new().run(&hctx)
}

fn run_default(fs: &MemFs) -> HeuristicOutput {
    let cfg = HeuristicsConfig::default();
    run_web(fs, &env(), &PathSet::new(), &cfg, &CancellationToken::new())
}

pub(super) fn root_of(f: &Finding) -> &Path {
    match &f.target {
        Target::FileSet { resolved, .. } => resolved,
        other => panic!("unexpected target {other:?}"),
    }
}

pub(super) fn globs(f: &Finding) -> (&[String], &[String]) {
    match &f.target {
        Target::FileSet {
            include, exclude, ..
        } => (include, exclude),
        other => panic!("unexpected target {other:?}"),
    }
}

/// [`CACHE_EXCLUDE`] followed by `extra`.
pub(super) fn cache_plus(extra: &[&str]) -> Vec<String> {
    CACHE_EXCLUDE
        .iter()
        .chain(extra)
        .map(|s| (*s).to_owned())
        .collect()
}

#[test]
fn electron_app_is_one_app_data_finding() {
    let mut fs = MemFs::new();
    electron(&mut fs, "Chatter");
    let out = run_default(&fs);

    let [f] = out.findings.as_slice() else {
        panic!("{:?}", out.findings)
    };
    let root = under(&env(), KnownFolder::AppData, "Chatter");
    assert!(eq_ci(root_of(f), &root));
    assert_eq!(f.category, Category::AppData);
    assert_eq!(f.title, "Chatter — heuristic.title.app_data");
    assert_eq!(f.app.as_ref().unwrap().id, "chatter");
    let (include, exclude) = globs(f);
    assert!(include.is_empty());
    assert_eq!(exclude, cache_plus(&[]));
    let ev = &f.evidence[0];
    assert_eq!(ev.message_key, REASON_ELECTRON);
    assert_eq!(ev.message_args["app"], "Chatter");
    assert!((ev.confidence - 0.8).abs() < 1e-6);
    assert_eq!(
        ev.source,
        sk_core::model::EvidenceSource::Heuristic {
            heuristic_id: "h-web".to_owned()
        }
    );
    assert_eq!(f.sensitivity, Sensitivity::None);
    assert_eq!(f.notes_key, None);
    assert!(f.tags.is_empty());
    assert_eq!(out.claimed, [root]);
    assert!(out.issues.is_empty());
    assert!(out.unknown.is_empty());
}

#[test]
fn folders_without_markers_are_left_to_h_unk() {
    let mut fs = MemFs::new();
    add(&mut fs, "Notepad4/settings.ini");
    // Two Electron names only: no marker.
    add(&mut fs, "Half/Local Storage/leveldb/CURRENT");
    add(&mut fs, "Half/Cache/data_0");
    // Only Chromium caches: a cache, not a profile.
    add(&mut fs, "Updater/Cache/data_0");
    add(&mut fs, "Updater/GPUCache/data_0");
    add(&mut fs, "Updater/Code Cache/js/index");
    let out = run_default(&fs);
    assert_eq!(out, HeuristicOutput::default());
}

#[test]
fn claimed_profiles_and_their_parts() {
    let mut fs = MemFs::new();
    electron(&mut fs, "Covered");
    electron(&mut fs, "Obsidian");
    add(&mut fs, "Obsidian/obsidian.json");
    add(&mut fs, "Obsidian/plugins/a/data.json");
    add(&mut fs, "Obsidian/plugins/b/data.json");
    let env = env();
    let claimed: PathSet = [
        under(&env, KnownFolder::AppData, "Covered"),
        under(&env, KnownFolder::AppData, "Obsidian/obsidian.json"),
        under(&env, KnownFolder::AppData, "Obsidian/plugins"),
        under(&env, KnownFolder::AppData, "Obsidian/plugins/a"),
    ]
    .into_iter()
    .collect();
    let cfg = HeuristicsConfig::default();
    let out = run_web(&fs, &env, &claimed, &cfg, &CancellationToken::new());

    let [f] = out.findings.as_slice() else {
        panic!("{:?}", out.findings)
    };
    assert!(eq_ci(
        root_of(f),
        &under(&env, KnownFolder::AppData, "Obsidian")
    ));
    assert_eq!(globs(f).1, cache_plus(&["obsidian.json", "plugins"]));
}

#[test]
fn synchronized_extensions_are_excluded() {
    let mut fs = MemFs::new();
    electron(&mut fs, "Synced");
    add(&mut fs, "Synced/Extensions/abc/manifest.json");
    add(&mut fs, "Synced/Sync Data/LevelDB/CURRENT");
    electron(&mut fs, "Unsynced");
    add(&mut fs, "Unsynced/extensions/abc/manifest.json");
    let out = run_default(&fs);

    let [synced, unsynced] = out.findings.as_slice() else {
        panic!("{:?}", out.findings)
    };
    assert_eq!(globs(synced).1, cache_plus(&["Extensions"]));
    assert_eq!(globs(unsynced).1, cache_plus(&[]));
}

#[test]
fn credentials_make_the_profile_sensitive() {
    for creds in ["Cookies", "Login Data", "Web Data", "Network/Cookies"] {
        let mut fs = MemFs::new();
        electron(&mut fs, "App");
        add(&mut fs, &format!("App/{creds}"));
        let out = run_default(&fs);
        let f = &out.findings[0];
        assert_eq!(f.sensitivity, Sensitivity::High, "{creds}");
        assert_eq!(f.tags, ["browser-credentials"], "{creds}");
        assert_eq!(f.notes_key.as_deref(), Some("heuristic.dpapi_bound"));
        assert_eq!(f.category, Category::AppData);
    }
    // A `Network` folder without `Cookies` holds no credentials.
    let mut fs = MemFs::new();
    electron(&mut fs, "App");
    add(&mut fs, "App/Network/Network Persistent State");
    let f = &run_default(&fs).findings[0];
    assert_eq!(f.sensitivity, Sensitivity::None);
    assert!(f.tags.is_empty());
}

#[test]
fn opaque_names_lose_confidence() {
    let mut fs = MemFs::new();
    electron(&mut fs, "{3F2504E0-4F89-11D3-9A0C-0305E82C3301}");
    let out = run_default(&fs);
    let f = &out.findings[0];
    assert_eq!(
        f.title,
        "{3F2504E0-4F89-11D3-9A0C-0305E82C3301} — heuristic.title.unrecognized — heuristic.title.app_data"
    );
    assert!((f.evidence[0].confidence - 0.7).abs() < 1e-6);
    assert_eq!(f.category, Category::AppData);
}

#[test]
fn cancelled_run_finds_nothing() {
    let mut fs = MemFs::new();
    electron(&mut fs, "Chatter");
    let cancel = CancellationToken::new();
    cancel.cancel();
    let cfg = HeuristicsConfig::default();
    let out = run_web(&fs, &env(), &PathSet::new(), &cfg, &cancel);
    assert_eq!(out, HeuristicOutput::default());
}

/// `MemFs` that refuses access to one folder.
struct Denied {
    inner: MemFs,
    denied: PathBuf,
}

impl Denied {
    fn check(&self, path: &Path) -> Result<(), FsError> {
        if eq_ci(path, &self.denied) {
            Err(FsError::AccessDenied)
        } else {
            Ok(())
        }
    }
}

impl FsScanner for Denied {
    fn metadata(&self, path: &Path) -> Result<EntryMeta, FsError> {
        self.inner.metadata(path)
    }
    fn exists(&self, path: &Path) -> bool {
        self.inner.exists(path)
    }
    fn read_dir(&self, path: &Path) -> Result<Vec<DirEntryInfo>, FsError> {
        self.check(path)?;
        self.inner.read_dir(path)
    }
    fn walk(
        &self,
        root: &Path,
        opts: &WalkOptions,
        visit: &mut dyn FnMut(&DirEntryInfo) -> WalkControl,
        cancel: &CancellationToken,
    ) -> Result<WalkStats, FsError> {
        self.check(root)?;
        self.inner.walk(root, opts, visit, cancel)
    }
    fn read_head(&self, path: &Path, max: usize) -> Result<Vec<u8>, FsError> {
        self.inner.read_head(path, max)
    }
    fn read_small(&self, path: &Path, max: usize) -> Result<Vec<u8>, FsError> {
        self.inner.read_small(path, max)
    }
    fn probe_readable(&self, path: &Path) -> Readability {
        self.inner.probe_readable(path)
    }
}

#[test]
fn read_problems_are_reported_only_without_h_unk() {
    let mut inner = MemFs::new();
    electron(&mut inner, "Chatter");
    add(&mut inner, "Locked/a.json");
    let env = env();
    let fs = Denied {
        inner,
        denied: under(&env, KnownFolder::AppData, "Locked"),
    };
    let claimed = PathSet::new();
    let cancel = CancellationToken::new();

    // H-UNK reads the same folder and reports it.
    let out = run_web(&fs, &env, &claimed, &HeuristicsConfig::default(), &cancel);
    assert_eq!(out.findings.len(), 1);
    assert!(out.issues.is_empty(), "{:?}", out.issues);

    let mut cfg = HeuristicsConfig::default();
    cfg.enabled.unk = false;
    let out = run_web(&fs, &env, &claimed, &cfg, &cancel);
    assert_eq!(out.findings.len(), 1);
    let [issue] = out.issues.as_slice() else {
        panic!("{:?}", out.issues)
    };
    assert_eq!(issue.severity, IssueSeverity::Warning);
    assert_eq!(issue.message_key, "heuristic.access_denied");
    assert_eq!(issue.source, "heuristics");
}
