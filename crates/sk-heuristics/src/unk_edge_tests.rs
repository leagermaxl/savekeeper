//! Tests of the H-UNK edge cases (SPEC-07 §5): opaque names, huge folders,
//! unreadable candidates and zones.

use std::path::{Path, PathBuf};

use sk_core::env::KnownFolder;
use sk_core::fs::{
    DirEntryInfo, EntryMeta, FsError, FsScanner, Readability, WalkControl, WalkOptions, WalkStats,
};
use sk_core::model::{Category, IssueSeverity, Target};
use sk_core::path::eq_ci;
use sk_core::CancellationToken;
use sk_scan::MemFs;

use super::tests::{abs, env, file, one, Setup, MIB};

#[test]
fn opaque_names_lose_confidence() {
    let mut fs = MemFs::new();
    file(
        &mut fs,
        KnownFolder::AppData,
        "{3F2504E0-4F89-11D3-9A0C-0305E82C3301}/settings.json",
        100,
        "-1d",
    );
    let out = Setup::new(fs).run();
    let f = &out.findings[0];
    assert_eq!(
        f.title,
        "{3F2504E0-4F89-11D3-9A0C-0305E82C3301} — heuristic.title.unrecognized"
    );
    assert!((f.evidence[0].confidence - 0.5).abs() < 1e-6);
    assert_eq!(f.category, Category::Unknown);
    assert_eq!(out.unknown.len(), 1);
}

#[test]
fn huge_candidates_are_tagged() {
    let mut fs = MemFs::new();
    file(
        &mut fs,
        KnownFolder::AppData,
        "Big/disk.bin",
        60 * 1024 * MIB,
        "-1d",
    );
    let out = Setup::new(fs).run();
    assert_eq!(out.findings[0].tags, ["huge"]);
}

/// `MemFs` that refuses access to one folder and stops the walk of another
/// after a number of entries (as the `max_entries` limit of `summarize`).
struct Denied {
    inner: MemFs,
    denied: PathBuf,
    limited: Option<(PathBuf, u64)>,
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
        self.check(path)?;
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
        let mut opts = opts.clone();
        if let Some((_, limit)) = self.limited.as_ref().filter(|(p, _)| eq_ci(p, root)) {
            opts.max_entries = *limit;
        }
        self.inner.walk(root, &opts, visit, cancel)
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
fn unreadable_candidate_is_unknown_with_a_warning() {
    let mut fs = MemFs::new();
    file(&mut fs, KnownFolder::AppData, "Locked/a.json", 100, "-1d");
    let denied = abs(KnownFolder::AppData, "Locked");
    let mut setup = Setup::new(MemFs::new());
    setup.fs = Box::new(Denied {
        inner: fs,
        denied: denied.clone(),
        limited: None,
    });
    let out = setup.run();

    let f = one(&out, &denied);
    assert_eq!(f.category, Category::Unknown);
    assert_eq!(f.evidence[0].message_key, "heuristic.access_denied");
    assert!(f.stats.as_ref().is_some_and(|s| s.truncated));
    assert!(out.unknown.is_empty());
    let [issue] = out.issues.as_slice() else {
        panic!("{:?}", out.issues)
    };
    assert_eq!(issue.severity, IssueSeverity::Warning);
    assert_eq!(issue.message_key, "heuristic.access_denied");
    assert_eq!(issue.source, "heuristics");
    assert_eq!(issue.path.as_deref(), Some(r"{APPDATA}\Locked"));
}

#[test]
fn unreadable_zone_gives_a_warning() {
    let denied = env()
        .known_folder(KnownFolder::SavedGames)
        .unwrap()
        .to_path_buf();
    let mut setup = Setup::new(MemFs::new());
    setup.fs = Box::new(Denied {
        inner: MemFs::new(),
        denied,
        limited: None,
    });
    let out = setup.run();
    let keys: Vec<_> = out.issues.iter().map(|i| i.message_key.as_str()).collect();
    assert_eq!(keys, ["heuristic.access_denied"]);
}

/// A truncated summary does not prove that the claimed parts are all of the
/// candidate: the rest is kept (SPEC-07 §5).
#[test]
fn truncated_partly_claimed_candidate_is_kept() {
    let mut fs = MemFs::new();
    for i in 0..50 {
        let rel = format!("Foo/a/file{i:02}.bin");
        file(&mut fs, KnownFolder::AppData, &rel, 100 * 1024, "-1d");
    }
    file(
        &mut fs,
        KnownFolder::AppData,
        "Foo/saves/slot1.sav",
        100,
        "-1d",
    );
    let foo = abs(KnownFolder::AppData, "Foo");
    let mut setup = Setup::new(MemFs::new());
    setup.claimed.insert(abs(KnownFolder::AppData, "Foo/a"));
    // The walk of `Foo` stops inside the claimed `a`: the rest looks empty.
    setup.fs = Box::new(Denied {
        inner: fs,
        denied: abs(KnownFolder::AppData, "None"),
        limited: Some((foo.clone(), 10)),
    });
    let out = setup.run();

    let f = one(&out, &foo);
    let Target::FileSet { exclude, .. } = &f.target else {
        panic!("{:?}", f.target)
    };
    assert_eq!(exclude, &["a"]);
    assert!(out.claimed.iter().any(|c| eq_ci(c, &foo)));
}
