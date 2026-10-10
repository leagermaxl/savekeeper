//! Tests of the shared walk of the roots (T-07-07): unreadable folders, the
//! cache, cancellation, progress, git marks and independence of the order in
//! which the walk gives the entries.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use sk_core::fs::{
    DirEntryInfo, EntryMeta, FsError, FsScanner, Readability, WalkControl, WalkOptions, WalkStats,
};
use sk_core::model::IssueSeverity;
use sk_core::path::starts_with_ci;
use sk_core::CancellationToken;
use sk_scan::MemFs;

use super::tests::{add, env, no_drives, rel, root, s, Setup};
use super::*;

/// A `MemFs` whose walk gives the entries in another order (by depth, names
/// descending), can fail, and reports unreadable folders.
pub(super) struct Shuffled {
    pub inner: MemFs,
    pub errors: u64,
    pub fail: bool,
}

impl FsScanner for Shuffled {
    fn metadata(&self, path: &Path) -> Result<EntryMeta, FsError> {
        self.inner.metadata(path)
    }
    fn exists(&self, path: &Path) -> bool {
        self.inner.exists(path)
    }
    fn read_dir(&self, path: &Path) -> Result<Vec<DirEntryInfo>, FsError> {
        self.inner.read_dir(path)
    }
    fn walk(
        &self,
        root: &Path,
        opts: &WalkOptions,
        visit: &mut dyn FnMut(&DirEntryInfo) -> WalkControl,
        cancel: &CancellationToken,
    ) -> Result<WalkStats, FsError> {
        if self.fail {
            return Err(FsError::AccessDenied);
        }
        let mut all = Vec::new();
        let mut stats = self.inner.walk(
            root,
            opts,
            &mut |e| {
                all.push(e.clone());
                WalkControl::Continue
            },
            cancel,
        )?;
        all.sort_by(|a, b| a.depth.cmp(&b.depth).then_with(|| b.path.cmp(&a.path)));
        let mut skipped: Vec<PathBuf> = Vec::new();
        for e in &all {
            if skipped.iter().any(|p| starts_with_ci(&e.path, p)) {
                continue;
            }
            match visit(e) {
                WalkControl::Continue => {}
                WalkControl::SkipDir => skipped.push(e.path.clone()),
                WalkControl::Stop => break,
            }
        }
        stats.errors += self.errors;
        Ok(stats)
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

fn small_tree() -> MemFs {
    let mut fs = MemFs::new();
    let work = root().join("Work");
    add(&mut fs, &work.join("a").join("x.docx"), 10);
    add(&mut fs, &work.join("b").join("y.jpg"), 20);
    fs
}

#[test]
fn unreadable_folders_and_roots_are_reported() {
    let setup = Setup::new(env());
    let work = root().join("Work");
    let errors = Shuffled {
        inner: small_tree(),
        errors: 3,
        fail: false,
    };
    let (walk, issues) = setup.walk(&errors, std::slice::from_ref(&work));
    assert_eq!(walk.roots[0].errors, 3);
    let [issue] = issues.as_slice() else {
        panic!("{issues:?}")
    };
    assert_eq!(issue.severity, IssueSeverity::Warning);
    assert_eq!(issue.message_key, "issue.scan.dirs_unreadable");
    assert_eq!(
        issue.message_args.get("count").map(String::as_str),
        Some("3")
    );

    let failing = Shuffled {
        inner: small_tree(),
        errors: 0,
        fail: true,
    };
    let (walk, issues) = setup.walk(&failing, &[work]);
    assert!(walk.dirs.is_empty());
    let [issue] = issues.as_slice() else {
        panic!("{issues:?}")
    };
    assert_eq!(issue.message_key, "heuristic.access_denied");
}

#[test]
fn the_cache_walks_once_and_reports_issues_once() {
    let mut fs = small_tree();
    fs.add_file(s(&root().join("Extra")), 1, "2026-01-01T00:00:00Z", None);
    let setup = Setup::new(env());
    let cache = RootsWalkCache::new(vec![root().join("Work"), root().join("Extra")], 32);

    let listings = setup.with(&fs, &cache, &|_, _| {}, |hctx| {
        let (first, issues) = cache.get(hctx);
        assert_eq!(first.dirs.len(), 3);
        assert_eq!(issues.len(), 1, "Extra is a file");
        let listings = fs.calls().read_dir;
        let (second, issues) = cache.get(hctx);
        assert!(std::ptr::eq(first, second));
        assert!(issues.is_empty(), "issues come once");
        listings
    });
    assert_eq!(fs.calls().read_dir, listings, "walked once");
    assert!(format!("{cache:?}").contains("walked: true"));
}

#[test]
fn a_cancelled_walk_is_marked() {
    let fs = small_tree();
    let setup = Setup::new(env());
    setup.cancel.cancel();
    let (walk, issues) = setup.walk(&fs, &[root().join("Work")]);
    assert!(walk.cancelled);
    assert!(walk.dirs.is_empty() && issues.is_empty());
}

#[test]
fn progress_names_the_root() {
    let mut fs = MemFs::new();
    for i in 0..1100 {
        add(&mut fs, &root().join("Many").join(format!("f{i}.txt")), 1);
    }
    let setup = Setup::new(env());
    let seen = RefCell::new(Vec::new());
    let progress = |done: u64, current: Option<&str>| {
        seen.borrow_mut().push((done, current.map(str::to_owned)));
    };
    let cache = RootsWalkCache::new(vec![root().join("Many")], 32);
    setup.with(&fs, &cache, &progress, |hctx| cache.get(hctx).0.dirs.len());
    let seen = seen.into_inner();
    let [(done, Some(current))] = seen.as_slice() else {
        panic!("{seen:?}")
    };
    assert_eq!(*done, 1024);
    assert!(current.ends_with("Many"), "{current}");
}

#[test]
fn git_working_copies_and_bare_repositories_are_marked() {
    let mut fs = MemFs::new();
    let base = root().join("src");
    add(&mut fs, &base.join("app").join("main.rs"), 10);
    add(&mut fs, &base.join("app").join(".git").join("HEAD"), 100);
    add(
        &mut fs,
        &base.join("app").join(".git").join("objects").join("x"),
        100,
    );
    add(&mut fs, &base.join("wt").join(".GIT"), 5);
    add(&mut fs, &base.join("bare").join("HEAD"), 1);
    add(&mut fs, &base.join("bare").join("objects").join("p"), 1);
    add(
        &mut fs,
        &base.join("bare").join("refs").join("heads").join("main"),
        1,
    );
    add(&mut fs, &base.join("half").join("HEAD"), 1);
    add(&mut fs, &base.join("half").join("refs").join("r"), 1);
    let setup = Setup::new(env());

    let (walk, _) = setup.walk(&fs, std::slice::from_ref(&base));
    let marks: Vec<(String, bool, bool)> = walk
        .dirs
        .iter()
        .map(|d| (rel(&d.path, &base), d.is_git_worktree(), d.is_bare_repo()))
        .collect();
    let mark = |p: &str, w, b| (p.to_owned(), w, b);
    assert_eq!(
        marks,
        [
            mark("", false, false),
            mark("app", true, false),
            mark("bare", false, true),
            mark("bare/objects", false, false),
            mark("bare/refs", false, false),
            mark("bare/refs/heads", false, false),
            mark("half", false, false),
            mark("half/refs", false, false),
            mark("wt", true, false),
        ]
    );
    let app = &walk.dirs[1];
    assert_eq!(
        (app.subtree.files, app.subtree.bytes),
        (1, 10),
        ".git is not walked"
    );
    assert_eq!(walk.dirs[8].own.files, 1, "a gitlink is a file");
}

#[test]
fn aggregates_do_not_depend_on_the_walk_order() {
    let (fs, env) = sk_testkit::mem_fixture("heuristics/usr-walk", root());
    let setup = Setup::new(no_drives(env));
    let roots = [root().to_path_buf()];
    let (sorted, _) = setup.walk(&fs, &roots);
    let shuffled = Shuffled {
        inner: fs,
        errors: 0,
        fail: false,
    };
    let (other, _) = setup.walk(&shuffled, &roots);
    assert_eq!(sorted, other);
    assert!(sorted.dirs.len() > 20);
}
