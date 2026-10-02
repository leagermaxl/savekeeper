//! `MemFs` loaded from `fixtures/fs/electron-app.yaml` through
//! `sk_testkit::mem_fixture` (SPEC-03 T-03-02).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sk_core::env::KnownFolder;
use sk_scan::{
    CancellationToken, EntryKind, Exclusion, FsScanner, PathFilter, Readability, WalkControl,
    WalkOptions,
};

#[derive(Debug)]
struct NoExcludes;

impl PathFilter for NoExcludes {
    fn check(&self, _: &Path, _: &OsStr, _: bool) -> Exclusion {
        Exclusion::Keep
    }
}

fn opts() -> WalkOptions {
    WalkOptions {
        max_depth: 32,
        max_entries: 2_000_000,
        follow_links: false,
        excludes: Arc::new(NoExcludes),
        include: None,
        exclude: None,
        threads: 1,
    }
}

fn root() -> PathBuf {
    PathBuf::from(if cfg!(windows) { r"C:\fake" } else { "/fake" })
}

#[test]
fn walk_over_electron_app_fixture() {
    let (fs, env) = sk_testkit::mem_fixture("electron-app", &root());
    let app = env
        .known_folder(KnownFolder::AppData)
        .unwrap()
        .join("Obsidian");

    let mut dirs = Vec::new();
    let mut files = 0;
    let mut bytes = 0;
    let stats = fs
        .walk(
            &app,
            &opts(),
            &mut |e| {
                assert!(e.path.starts_with(&app));
                match e.meta.kind {
                    EntryKind::Dir if e.depth == 1 => dirs.push(e.rel.clone()),
                    EntryKind::Dir => {}
                    _ => {
                        files += 1;
                        bytes += e.meta.size;
                    }
                }
                WalkControl::Continue
            },
            &CancellationToken::new(),
        )
        .unwrap();

    assert_eq!(stats.entries, 45);
    assert!(!stats.truncated);
    assert_eq!(stats.errors, 0);
    assert_eq!(files, 32);
    assert!(bytes > 600 * 1024, "{bytes}");
    for electron in [
        "Local Storage",
        "IndexedDB",
        "Cache",
        "GPUCache",
        "Code Cache",
        "Session Storage",
        "blob_storage",
        "Service Worker",
    ] {
        assert!(dirs.contains(&PathBuf::from(electron)), "{electron}");
    }
}

#[test]
fn fixture_flags_and_content() {
    let (fs, env) = sk_testkit::mem_fixture("electron-app", &root());
    let app = env
        .known_folder(KnownFolder::AppData)
        .unwrap()
        .join("Obsidian");
    let lock = app.join("Local Storage").join("leveldb").join("LOCK");
    assert_eq!(fs.probe_readable(&lock), Readability::Locked);
    let current = app.join("Local Storage/leveldb/CURRENT");
    assert_eq!(fs.read_small(&current, 64).unwrap(), b"MANIFEST-000001\n");
    let config = fs.read_small(&app.join("obsidian.json"), 1024).unwrap();
    assert!(config.starts_with(br#"{"vaults""#));
    assert!(fs.read_dir(&app.join("blob_storage")).unwrap().is_empty());
    assert_eq!(fs.calls().read_head, 0);
}
