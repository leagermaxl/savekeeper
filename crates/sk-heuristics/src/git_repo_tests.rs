//! Tests of H-GIT on real repositories (T-07-09, SPEC-07 §6): the state read
//! through gix, the rows of §4.4.3, heavy ignored folders, gitlink work
//! trees, submodules, nested and broken repositories. Every test checks that
//! nothing under its folder changed: no file, size, time or content
//! (NFR-07-05, P1).
//!
//! The repositories are written with gix into a temporary folder under
//! `<workspace>/target/tmp`, not into the system temp folder.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::SystemTime;

use sk_core::model::{Category, Finding, Target};
use sk_scan::RealFs;
use tempfile::TempDir;

use super::test_repo::{write, TestRepo};
use super::tests::{exclude_of, Case};
use super::*;
use crate::git_finding::*;
use crate::git_state::analyze;
use crate::git_state::GitState;

const MIB: u64 = 1024 * 1024;
const REMOTE: &str = "https://user:secret@example.com/team/repo.git";

/// A fresh temporary folder under `<workspace>/target/tmp`.
fn temp_dir() -> TempDir {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
    fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new()
        .prefix("sk-heuristics-git-")
        .tempdir_in(&base)
        .unwrap()
}

/// Kind, size, modification time and (small files) content of every entry.
type Snapshot = BTreeMap<String, (bool, u64, SystemTime, Option<Vec<u8>>)>;

fn snapshot(dir: &Path) -> Snapshot {
    let mut out = Snapshot::new();
    visit(dir, "", &mut out);
    out
}

fn visit(dir: &Path, prefix: &str, out: &mut Snapshot) {
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        // Not `DirEntry::metadata`: on Windows it comes from the listing of
        // the parent, whose copy of the times NTFS updates lazily.
        let meta = fs::symlink_metadata(entry.path()).unwrap();
        let name = format!("{prefix}{}", entry.file_name().to_string_lossy());
        let modified = meta.modified().unwrap();
        if meta.is_dir() {
            out.insert(name.clone(), (true, 0, modified, None));
            visit(&entry.path(), &format!("{name}/"), out);
        } else {
            let content = (meta.len() < MIB).then(|| fs::read(entry.path()).unwrap());
            out.insert(name, (false, meta.len(), modified, content));
        }
    }
}

/// Runs H-GIT over `root` (inside `base`) on the real file system and checks
/// that nothing under `base` changed.
fn run(base: &Path, root: &Path) -> HeuristicOutput {
    run_with_depth(base, root, 32)
}

/// [`run`] with a depth limit of the shared walk.
fn run_with_depth(base: &Path, root: &Path, depth: u32) -> HeuristicOutput {
    let before = snapshot(base);
    let mut case = Case::new(base, vec![root.to_path_buf()]);
    case.depth = depth;
    let fs = RealFs::new(&case.env);
    let out = case.run(&fs);
    assert_unchanged(&before, &snapshot(base));
    out
}

/// Names the entries that appeared, disappeared or changed.
fn assert_unchanged(before: &Snapshot, after: &Snapshot) {
    let changed: Vec<String> = before
        .keys()
        .chain(after.keys())
        .filter(|k| before.get(*k) != after.get(*k))
        .map(|k| {
            let what = |e: Option<&(bool, u64, SystemTime, Option<Vec<u8>>)>| {
                e.map(|(dir, len, time, _)| format!("dir={dir} len={len} time={time:?}"))
            };
            format!("{k}: {:?} -> {:?}", what(before.get(k)), what(after.get(k)))
        })
        .collect();
    assert!(changed.is_empty(), "H-GIT changed files: {changed:#?}");
}

fn state(path: &Path) -> GitState {
    let env = sk_core::env::Environment::fake(path);
    analyze(path, true, &RealFs::new(&env)).unwrap().state
}

fn finding<'a>(out: &'a HeuristicOutput, path: &Path) -> &'a Finding {
    out.findings
        .iter()
        .find(|f| matches!(&f.target, Target::FileSet { resolved, .. } if resolved == path))
        .unwrap_or_else(|| panic!("no finding for {}: {:#?}", path.display(), out.findings))
}

fn arg<'a>(f: &'a Finding, key: &str) -> &'a str {
    f.evidence[0].message_args[key].as_str()
}

/// `README.md` with `n` lines.
fn readme(n: usize) -> String {
    "line\n".repeat(n)
}

/// A repository with `commits` commits, pushed to `origin` up to `pushed`.
fn repo(path: &Path, commits: usize, pushed: Option<usize>) -> TestRepo {
    let mut repo = TestRepo::init(path);
    for n in 1..=commits {
        repo.commit(&[("README.md", &readme(n))]);
    }
    if let Some(pushed) = pushed {
        let id = pushed.checked_sub(1).map(|i| repo.commits[i]);
        repo.remote(REMOTE, id);
    }
    repo
}

#[test]
fn state_of_repositories() {
    let dir = temp_dir();
    let root = dir.path().join("root");

    let path = root.join("no-remote");
    repo(&path, 2, None);
    let s = state(&path);
    assert!(s.remotes.is_empty());
    assert_eq!((s.unpushed_commits, s.local_only_branches), (2, 1));
    assert_eq!(s.head_branch.as_deref(), Some("main"));
    assert!(!s.detached);
    let time = s.last_commit_time.unwrap();
    assert_eq!(time.unix_timestamp(), 1_700_000_000 + 3600);

    let path = root.join("clean");
    repo(&path, 2, Some(2));
    let s = state(&path);
    assert_eq!(s.remotes, ["origin"]);
    assert_eq!(s.unsaved(), 0);
    assert_eq!(s.local_only_branches, 0);
    assert!(!s.has_lfs);

    let path = root.join("dirty");
    let r = repo(&path, 1, Some(1));
    r.write_file("README.md", "changed\n");
    assert_eq!(state(&path).dirty_files, 1);

    let path = root.join("staged");
    let mut r = repo(&path, 1, Some(1));
    r.stage("README.md", "staged\n");
    assert_eq!(state(&path).dirty_files, 1);

    let path = root.join("untracked");
    let mut r = repo(&path, 1, None);
    r.commit(&[
        (".gitignore", "*.log\n"),
        (".gitattributes", "*.psd filter=lfs\n"),
    ]);
    r.write_file("new.txt", "x");
    r.write_file("docs/more.txt", "x");
    r.write_file("debug.log", "x");
    let s = state(&path);
    assert_eq!((s.untracked_files, s.dirty_files), (2, 0));
    assert!(s.has_lfs);

    let path = root.join("unpushed");
    repo(&path, 3, Some(1));
    let s = state(&path);
    assert_eq!((s.unpushed_commits, s.local_only_branches), (2, 0));

    let path = root.join("branch");
    let r = repo(&path, 2, Some(2));
    r.branch("feature", r.commits[0]);
    let s = state(&path);
    assert_eq!((s.unpushed_commits, s.local_only_branches), (1, 1));

    let path = root.join("stash");
    let r = repo(&path, 1, Some(1));
    r.stash(2);
    assert_eq!(state(&path).stash_entries, 2);

    let path = root.join("detached");
    let r = repo(&path, 2, Some(1));
    r.branch("main", r.commits[0]);
    r.detach(r.commits[1]);
    let s = state(&path);
    assert!(s.detached);
    assert_eq!(s.head_branch, None);
    assert_eq!(s.unpushed_commits, 1);

    let out = run(dir.path(), &root);
    assert!(out.issues.is_empty(), "{:?}", out.issues);
    assert_eq!(out.findings.len(), 9);
    let clean = finding(&out, &root.join("clean"));
    assert_eq!(clean.category, Category::Reinstallable);
    assert_eq!(clean.evidence[0].confidence, 0.85);
    assert_eq!(clean.tags, ["git", "git-clean"]);
    assert_eq!(arg(clean, "remote"), "example.com");
    let no_remote = finding(&out, &root.join("no-remote"));
    assert_eq!(no_remote.category, Category::DevEnvironment);
    assert_eq!(no_remote.tags, ["git", "git-no-remote", "git-unsaved"]);
    assert_eq!(arg(no_remote, "remote"), "");
    for (name, key, value) in [
        ("dirty", "dirty", "1"),
        ("staged", "dirty", "1"),
        ("unpushed", "unpushed", "2"),
        ("branch", "branches", "1"),
        ("stash", "stash", "2"),
        ("detached", "unpushed", "1"),
    ] {
        let f = finding(&out, &root.join(name));
        assert_eq!(f.tags, ["git", "git-unsaved"], "{name}");
        assert_eq!(f.evidence[0].message_key, REASON_UNSAVED, "{name}");
        assert_eq!(f.evidence[0].confidence, 0.95, "{name}");
        assert_eq!(arg(f, key), value, "{name}");
    }
    let untracked = finding(&out, &root.join("untracked"));
    assert_eq!(arg(untracked, "untracked"), "2");
}

#[test]
fn heavy_ignored_folders_are_excluded() {
    let dir = temp_dir();
    let root = dir.path().join("root");
    let path = root.join("web");
    let mut r = TestRepo::init(&path);
    let id = r.commit(&[(".gitignore", "node_modules/\ndata/\nbuild/\ndist/\n")]);
    r.remote(REMOTE, Some(id));
    let big = |rel: &str| {
        write(&path, rel, "");
        let file = fs::OpenOptions::new()
            .write(true)
            .open(path.join(rel))
            .unwrap();
        file.set_len(51 * MIB).unwrap();
    };
    // A global exclusion of the walk: measured on its own.
    big("node_modules/left-pad/big.bin");
    // Walked: its size comes from the walk.
    big("dist/app.bin");
    // Ignored, heavy, but maybe the user's own data: kept.
    big("data/set.bin");
    // A build folder, but small: kept.
    r.write_file("build/out.txt", "x");

    let out = run(dir.path(), &root);
    assert!(out.issues.is_empty(), "{:?}", out.issues);
    let f = finding(&out, &path);
    assert_eq!(exclude_of(f), ["dist", "node_modules"]);
    assert_eq!(f.tags, ["git", "git-clean"]);
    assert_eq!(arg(f, "untracked"), "0");
}

#[test]
fn gitlink_work_tree_with_its_repository_outside_the_walk() {
    let dir = temp_dir();
    let root = dir.path().join("root");
    let path = root.join("wt");
    let mut r = TestRepo::init_separate(&path, &dir.path().join("elsewhere").join("wt.git"));
    r.commit(&[("README.md", "x\n")]);
    r.write_file("new.txt", "x");

    let out = run(dir.path(), &root);
    assert!(out.issues.is_empty(), "{:?}", out.issues);
    let [f] = out.findings.as_slice() else {
        panic!("{:#?}", out.findings)
    };
    assert_eq!(f.evidence[0].message_key, REASON_NO_REMOTE);
    assert_eq!(arg(f, "untracked"), "1");
    assert_eq!(arg(f, "unpushed"), "1");
}

#[test]
fn broken_head_gives_an_issue_and_a_finding_without_state() {
    let dir = temp_dir();
    let root = dir.path().join("root");
    let path = root.join("broken");
    let r = repo(&path, 1, None);
    write(&r.git_dir, "HEAD", "this is not a reference\n");

    let out = run(dir.path(), &root);
    let [f] = out.findings.as_slice() else {
        panic!("{:#?}", out.findings)
    };
    assert_eq!(f.evidence[0].message_key, REASON_FAILED);
    assert_eq!(f.evidence[0].confidence, 0.5);
    assert_eq!(f.category, Category::DevEnvironment);
    let [issue] = out.issues.as_slice() else {
        panic!("{:?}", out.issues)
    };
    assert_eq!(issue.message_key, ISSUE_FAILED);
    assert!(!issue.message_args["error"].is_empty());
}

#[test]
fn bare_repository() {
    let dir = temp_dir();
    let root = dir.path().join("root");
    let path = root.join("backup.git");
    let mut r = TestRepo::init_bare(&path);
    r.commit(&[("README.md", "x\n")]);

    let out = run(dir.path(), &root);
    assert!(out.issues.is_empty(), "{:?}", out.issues);
    let [f] = out.findings.as_slice() else {
        panic!("{:#?}", out.findings)
    };
    assert_eq!(f.evidence[0].message_key, REASON_NO_REMOTE);
    assert_eq!(arg(f, "unpushed"), "1");
    assert!(exclude_of(f).is_empty());
}

#[test]
fn submodule_is_part_of_the_outer_repository() {
    let dir = temp_dir();
    let root = dir.path().join("root");
    let outer_path = root.join("outer");
    let mut outer = repo(&outer_path, 1, Some(1));
    let sub = repo(&outer_path.join("sub"), 1, Some(1));
    sub.write_file("scratch.txt", "x");
    let id = outer.add_gitlink("sub", sub.commits[0]);
    outer.remote(REMOTE, Some(id));

    let out = run(dir.path(), &root);
    assert!(out.issues.is_empty(), "{:?}", out.issues);
    let [f] = out.findings.as_slice() else {
        panic!("{:#?}", out.findings)
    };
    assert!(exclude_of(f).is_empty());
    assert_eq!(arg(f, "untracked"), "1");
    assert_eq!(arg(f, "dirty"), "0");
    assert_eq!(f.tags, ["git", "git-unsaved"]);
    assert_eq!(out.claimed, [outer_path]);
}

#[test]
fn nested_repository_is_excluded_from_the_outer_one() {
    let dir = temp_dir();
    let root = dir.path().join("root");
    let outer_path = root.join("outer");
    repo(&outer_path, 1, Some(1));
    let inner = repo(&outer_path.join("inner"), 1, None);
    inner.write_file("notes.txt", "x");

    let out = run(dir.path(), &root);
    assert!(out.issues.is_empty(), "{:?}", out.issues);
    let outer = finding(&out, &outer_path);
    assert_eq!(exclude_of(outer), ["inner"]);
    assert_eq!(arg(outer, "untracked"), "0");
    assert_eq!(outer.tags, ["git", "git-clean"]);
    let inner = finding(&out, &outer_path.join("inner"));
    assert_eq!(arg(inner, "untracked"), "1");
    assert_eq!(inner.tags, ["git", "git-no-remote", "git-unsaved"]);
}

#[test]
fn submodule_found_only_by_the_home_walk_is_merged() {
    let dir = temp_dir();
    let home = Case::new(dir.path(), vec![]).home();
    let projects = home.join("Projects");
    let outer_path = projects.join("outer");
    let mut outer = repo(&outer_path, 1, Some(1));
    let sub = repo(&outer_path.join("sub"), 1, Some(1));
    sub.write_file("scratch.txt", "x");
    let id = outer.add_gitlink("sub", sub.commits[0]);
    outer.remote(REMOTE, Some(id));

    // The shared walk finds `outer` but does not enter `outer/sub`; the
    // walk of `{HOME}` finds the submodule.
    let out = run_with_depth(dir.path(), &projects, 2);
    assert!(out.issues.is_empty(), "{:?}", out.issues);
    let [f] = out.findings.as_slice() else {
        panic!("{:#?}", out.findings)
    };
    assert!(exclude_of(f).is_empty());
    assert_eq!(arg(f, "untracked"), "1");
    assert_eq!(out.claimed, [outer_path]);
}

#[test]
fn repository_inside_a_submodule_is_excluded_from_the_outer_one() {
    let dir = temp_dir();
    let root = dir.path().join("root");
    let outer_path = root.join("outer");
    let mut outer = repo(&outer_path, 1, Some(1));
    let sub = repo(&outer_path.join("sub"), 1, Some(1));
    // Not a submodule of `sub`: its index has no gitlink for it.
    let inner_path = outer_path.join("sub").join("inner");
    repo(&inner_path, 1, None);
    let id = outer.add_gitlink("sub", sub.commits[0]);
    outer.remote(REMOTE, Some(id));

    let out = run(dir.path(), &root);
    assert!(out.issues.is_empty(), "{:?}", out.issues);
    let roots: Vec<&Path> = out
        .findings
        .iter()
        .map(|f| match &f.target {
            Target::FileSet { resolved, .. } => resolved.as_path(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(roots, [outer_path.as_path(), inner_path.as_path()]);
    assert_eq!(exclude_of(finding(&out, &outer_path)), ["sub/inner"]);
    assert_eq!(out.claimed, [outer_path, inner_path]);
}
