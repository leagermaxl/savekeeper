//! Tests of H-GIT without real repositories (T-07-09): which folders are
//! repositories, nesting, the walk of `{HOME}`, the limit, the rows of
//! §4.4.3. On a `MemFs` gix finds nothing on disk, so every repository is
//! one that could not be read.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sk_core::config::HeuristicsConfig;
use sk_core::env::{Environment, KnownFolder};
use sk_core::fs::FsScanner;
use sk_core::model::{Category, IssueSeverity, Target};
use sk_core::path::PathSet;
use sk_core::CancellationToken;
use sk_scan::{ExcludeSet, MemFs};

use super::*;
use crate::git_finding::*;
use crate::git_state::GitState;
use crate::heuristic::{Heuristic, HeuristicContext, HeuristicOutput};
use crate::walk::RootsWalkCache;

fn root() -> &'static Path {
    Path::new(if cfg!(windows) {
        r"C:\fake-git"
    } else {
        "/fake-git"
    })
}

fn s(path: &Path) -> &str {
    path.to_str().unwrap()
}

/// Everything a run needs besides the file system.
pub(super) struct Case {
    pub env: Environment,
    pub cfg: HeuristicsConfig,
    pub claimed: PathSet,
    pub roots: Vec<PathBuf>,
    /// Depth limit of the shared walk.
    pub depth: u32,
}

impl Case {
    pub(super) fn new(base: &Path, roots: Vec<PathBuf>) -> Self {
        let mut env = Environment::fake(base);
        env.drives.clear();
        Self {
            env,
            cfg: HeuristicsConfig::default(),
            claimed: PathSet::new(),
            roots,
            depth: 32,
        }
    }

    pub(super) fn home(&self) -> PathBuf {
        self.env
            .known_folder(KnownFolder::Home)
            .unwrap()
            .to_path_buf()
    }

    pub(super) fn run(&self, fs: &dyn FsScanner) -> HeuristicOutput {
        let excludes = Arc::new(ExcludeSet::builtin(&self.env));
        let cache = RootsWalkCache::new(self.roots.clone(), self.depth);
        let cancel = CancellationToken::new();
        let hctx = HeuristicContext {
            env: &self.env,
            scanner: fs,
            excludes: &excludes,
            claimed: &self.claimed,
            prior: &[],
            cfg: &self.cfg,
            cancel: &cancel,
            progress: &|_, _| {},
            roots_walk: &cache,
        };
        HGit::new().run(&hctx)
    }
}

fn add(fs: &mut MemFs, path: &Path) {
    fs.add_file(s(path), 10, "2026-01-01T00:00:00Z", None);
}

/// A work tree with `.git/HEAD`.
fn worktree(fs: &mut MemFs, path: &Path) {
    add(fs, &path.join(".git").join("HEAD"));
    add(fs, &path.join("README.md"));
}

/// A bare repository: `HEAD`, `objects`, `refs`.
fn bare(fs: &mut MemFs, path: &Path) {
    add(fs, &path.join("HEAD"));
    fs.add_dir(s(&path.join("objects")));
    fs.add_dir(s(&path.join("refs")));
}

pub(super) fn exclude_of(f: &sk_core::model::Finding) -> &[String] {
    match &f.target {
        Target::FileSet { exclude, .. } => exclude,
        other => panic!("{other:?}"),
    }
}

fn root_of(f: &sk_core::model::Finding) -> &Path {
    match &f.target {
        Target::FileSet { resolved, .. } => resolved,
        other => panic!("{other:?}"),
    }
}

#[test]
fn finds_work_trees_bare_and_nested_repositories() {
    let work = root().join("Work");
    let mut fs = MemFs::new();
    worktree(&mut fs, &work.join("app"));
    // A gitlink file: a work tree too.
    add(&mut fs, &work.join("app").join("lib").join(".git"));
    bare(&mut fs, &work.join("backup.git"));
    // Inside a bare repository nothing is looked for.
    bare(&mut fs, &work.join("backup.git").join("refs").join("inner"));
    worktree(&mut fs, &work.join("taken"));
    add(&mut fs, &work.join("plain").join("notes.txt"));
    let mut case = Case::new(root(), vec![work.clone()]);
    case.claimed.insert(work.join("taken"));

    let out = case.run(&fs);
    let roots: Vec<&Path> = out.findings.iter().map(root_of).collect();
    assert_eq!(
        roots,
        [
            work.join("app"),
            work.join("app").join("lib"),
            work.join("backup.git")
        ]
    );
    assert_eq!(out.claimed, roots);
    // Nothing is on disk: each repository could not be read.
    for f in &out.findings {
        assert_eq!(f.category, Category::DevEnvironment);
        assert_eq!(f.evidence[0].confidence, 0.5);
        assert_eq!(f.evidence[0].message_key, REASON_FAILED);
        assert_eq!(
            f.evidence[0].source,
            sk_core::model::EvidenceSource::Heuristic {
                heuristic_id: "h-git".to_owned()
            }
        );
        assert_eq!(f.tags, ["git"]);
        assert!(f.app.is_none());
        let Target::FileSet { include, .. } = &f.target else {
            panic!()
        };
        assert_eq!(include, &["**"]);
    }
    // Not a submodule (the outer one is unreadable): excluded from the outer one.
    assert_eq!(exclude_of(&out.findings[0]), ["lib"]);
    assert!(exclude_of(&out.findings[1]).is_empty());
    assert_eq!(out.issues.len(), 3);
    for issue in &out.issues {
        assert_eq!(issue.severity, IssueSeverity::Warning);
        assert_eq!(issue.source, "heuristics");
        assert_eq!(issue.message_key, ISSUE_FAILED);
        assert!(issue.message_args.contains_key("error"));
    }
}

#[test]
fn claimed_parts_of_a_repository_are_excluded() {
    let work = root().join("Work");
    let mut fs = MemFs::new();
    worktree(&mut fs, &work.join("app"));
    add(&mut fs, &work.join("app").join("conf").join("a.json"));
    let mut case = Case::new(root(), vec![work.clone()]);
    case.claimed.insert(work.join("app").join("conf"));
    let out = case.run(&fs);
    assert_eq!(out.findings.len(), 1);
    assert_eq!(exclude_of(&out.findings[0]), ["conf"]);
}

#[test]
fn home_is_walked_when_the_roots_do_not_reach_it() {
    let work = root().join("Work");
    let mut case = Case::new(root(), vec![work.clone()]);
    let home = case.home();
    let mut fs = MemFs::new();
    worktree(&mut fs, &work.join("app"));
    worktree(&mut fs, &home.join("Projects").join("tool"));
    // `{HOME}\AppData` is H-UNK's.
    worktree(
        &mut fs,
        &home.join("AppData").join("Local").join("cache-repo"),
    );
    // Deeper than `git.max_depth`.
    let mut deep = home.clone();
    deep.extend(["a", "b", "c", "d", "e", "f", "g"]);
    worktree(&mut fs, &deep);

    let out = case.run(&fs);
    let roots: Vec<&Path> = out.findings.iter().map(root_of).collect();
    assert_eq!(
        roots,
        [work.join("app"), home.join("Projects").join("tool")]
    );

    // When the roots hold `{HOME}`, it is not walked again.
    case.roots = vec![root().to_path_buf()];
    let out = case.run(&fs);
    let roots: Vec<&Path> = out.findings.iter().map(root_of).collect();
    assert_eq!(
        roots,
        [deep, home.join("Projects").join("tool"), work.join("app")]
    );
}

#[test]
fn home_under_a_root_the_walk_does_not_reach_is_walked() {
    let mut case = Case::new(root(), vec![root().to_path_buf()]);
    let home = case.home();
    let mut fs = MemFs::new();
    worktree(&mut fs, &home.join("Projects").join("tool"));
    // The shared walk stops above `{HOME}`.
    case.depth = 1;
    assert!(home.components().count() > root().components().count() + 1);

    let out = case.run(&fs);
    let roots: Vec<&Path> = out.findings.iter().map(root_of).collect();
    assert_eq!(roots, [home.join("Projects").join("tool")]);
}

#[test]
fn repository_at_the_depth_limit_of_the_shared_walk_is_found_once() {
    let mut case = Case::new(root(), vec![]);
    let projects = case.home().join("Projects");
    case.roots = vec![root().to_path_buf(), projects.clone()];
    // The shared walk lists `Projects	ool` but does not enter it.
    case.depth = 1;
    let mut fs = MemFs::new();
    worktree(&mut fs, &projects.join("tool"));

    let out = case.run(&fs);
    let roots: Vec<&Path> = out.findings.iter().map(root_of).collect();
    assert_eq!(roots, [projects.join("tool")]);
}

#[test]
fn repository_found_only_by_the_home_walk_is_excluded_from_the_outer_one() {
    let mut case = Case::new(root(), vec![]);
    let projects = case.home().join("Projects");
    case.roots = vec![projects.clone()];
    // The shared walk finds `outer` but does not enter `outer\inner`.
    case.depth = 2;
    let mut fs = MemFs::new();
    worktree(&mut fs, &projects.join("outer"));
    worktree(&mut fs, &projects.join("outer").join("inner"));

    let out = case.run(&fs);
    let roots: Vec<&Path> = out.findings.iter().map(root_of).collect();
    assert_eq!(
        roots,
        [projects.join("outer"), projects.join("outer").join("inner")]
    );
    assert_eq!(exclude_of(&out.findings[0]), ["inner"]);
    assert!(exclude_of(&out.findings[1]).is_empty());
}

#[test]
fn repositories_over_the_limit_are_not_read() {
    let work = root().join("Work");
    let mut fs = MemFs::new();
    for name in ["a", "b", "c"] {
        worktree(&mut fs, &work.join(name));
    }
    let mut case = Case::new(root(), vec![work.clone()]);
    case.cfg.git.max_repos = 1;
    let out = case.run(&fs);
    assert_eq!(out.findings.len(), 3);
    assert_eq!(out.findings[0].evidence[0].message_key, REASON_FAILED);
    for f in &out.findings[1..] {
        assert_eq!(f.evidence[0].message_key, REASON_NOT_ANALYZED);
        assert_eq!(f.evidence[0].confidence, 0.5);
        assert_eq!(f.tags, ["git", "truncated"]);
    }
    let truncated: Vec<_> = out
        .issues
        .iter()
        .filter(|i| i.message_key == ISSUE_TRUNCATED)
        .collect();
    let [issue] = truncated.as_slice() else {
        panic!("{:?}", out.issues)
    };
    assert_eq!(issue.message_args["count"], "2");
    assert_eq!(issue.message_args["limit"], "1");
}

#[test]
fn rows_of_the_classification() {
    let remote = || vec!["origin".to_owned()];
    let clean = GitState {
        remotes: remote(),
        ..GitState::default()
    };
    let (v, tags) = classify(&clean, false);
    assert_eq!(
        (v.category, v.confidence, v.reason_key),
        (Category::Reinstallable, 0.85, REASON_CLEAN)
    );
    assert_eq!(tags, [TAG_CLEAN]);

    let (v, tags) = classify(&GitState::default(), false);
    assert_eq!(
        (v.category, v.confidence, v.reason_key),
        (Category::DevEnvironment, 0.95, REASON_NO_REMOTE)
    );
    assert_eq!(tags, [TAG_NO_REMOTE, TAG_UNSAVED]);

    let unsaved = [
        GitState {
            dirty_files: 1,
            ..clean.clone()
        },
        GitState {
            untracked_files: 1,
            ..clean.clone()
        },
        GitState {
            unpushed_commits: 1,
            ..clean.clone()
        },
        GitState {
            stash_entries: 1,
            ..clean.clone()
        },
    ];
    for state in &unsaved {
        let (v, tags) = classify(state, false);
        assert_eq!(
            (v.category, v.confidence, v.reason_key),
            (Category::DevEnvironment, 0.95, REASON_UNSAVED)
        );
        assert_eq!(tags, [TAG_UNSAVED]);
    }
    // A work tree that was not looked at is not taken for clean.
    let (v, _) = classify(&clean, true);
    assert_eq!(v.reason_key, REASON_UNSAVED);
}

#[test]
fn evidence_arguments() {
    let state = GitState {
        dirty_files: 1,
        untracked_files: 2,
        unpushed_commits: 3,
        stash_entries: 4,
        local_only_branches: 5,
        ..GitState::default()
    };
    let args = state_args(&state, "example.com");
    let pairs: Vec<(&str, &str)> = args.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    assert_eq!(
        pairs,
        [
            ("branches", "5"),
            ("dirty", "1"),
            ("remote", "example.com"),
            ("stash", "4"),
            ("unpushed", "3"),
            ("untracked", "2")
        ]
    );
}

#[test]
fn heavy_folder_names() {
    for rel in [
        "node_modules",
        "web/node_modules",
        "target",
        ".venv",
        ".venv-3.12",
        "Build",
        "dist",
    ] {
        assert!(is_heavy_name(rel), "{rel}");
    }
    for rel in ["data", "node_modules/data", "secrets", ".env", "notes"] {
        assert!(!is_heavy_name(rel), "{rel}");
    }
}

#[test]
fn submodule_state_is_added_to_the_outer_one() {
    let mut outer = GitState {
        untracked_files: 9_999,
        unpushed_commits: 999,
        ..GitState::default()
    };
    let sub = GitState {
        dirty_files: 2,
        untracked_files: 5,
        unpushed_commits: 5,
        stash_entries: 1,
        has_lfs: true,
        ignored_heavy_dirs: vec![("node_modules".to_owned(), 1)],
        ..GitState::default()
    };
    outer.add_submodule(&sub, "libs/ui");
    assert_eq!(outer.dirty_files, 2);
    assert_eq!(outer.untracked_files, 10_000);
    assert_eq!(outer.unpushed_commits, 1_000);
    assert_eq!(outer.stash_entries, 1);
    assert!(outer.has_lfs);
    assert_eq!(
        outer.ignored_heavy_dirs,
        [("libs/ui/node_modules".to_owned(), 1)]
    );
}
