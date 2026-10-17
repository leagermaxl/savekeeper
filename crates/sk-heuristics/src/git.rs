//! H-GIT: git repositories and their state (SPEC-07 §4.4).
//!
//! The repositories are found by the shared walk of the roots (§4.3.1, which
//! marks work trees and bare repositories) and, when that walk does not reach
//! `{HOME}`, by a walk of `{HOME}` down to `git.max_depth`. Each one is read
//! through gix ([`analyze`], read-only) and gives one `FileSet` finding of the
//! whole repository without its heavy ignored folders (§4.4.4).
//!
//! A repository inside another one is a submodule when the outer index has a
//! gitlink at its path: its state is added to the outer one and it gives no
//! finding of its own. Otherwise both give findings and the outer one
//! excludes the inner one (§5).

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sk_core::fs::{EntryKind, Exclusion, PathFilter, WalkControl, WalkOptions};
use sk_core::path::{eq_ci, PathSet};

use crate::git_finding::{failed_issue, repo_finding, truncated_issue, Outcome};
use crate::git_state::analyze;
use crate::heuristic::{Heuristic, HeuristicContext, HeuristicOutput};
use crate::unk::{relative_glob, topmost};
use crate::walk::walk_roots;
use crate::walk_tree::RootsWalk;

#[cfg(test)]
#[path = "git_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "git_repo_tests.rs"]
mod repo_tests;

#[cfg(test)]
#[path = "git_test_repo.rs"]
mod test_repo;

/// `Heuristic::id` of H-GIT.
pub(crate) const HEURISTIC_ID: &str = "h-git";
/// An ignored folder heavier than this is excluded (§4.4.2): 50 MiB.
pub(crate) const HEAVY_BYTES: u64 = 50 * 1024 * 1024;
/// Ignored folders that tools rebuild or download again (§4.4.2); other
/// ignored folders may hold the user's data and stay in the finding.
const HEAVY_NAMES: [&str; 22] = [
    "node_modules",
    "bower_components",
    ".pnpm-store",
    "target",
    ".venv",
    "venv",
    "build",
    "dist",
    "out",
    "bin",
    "obj",
    "__pycache__",
    ".gradle",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".parcel-cache",
    ".turbo",
    ".angular",
    ".dart_tool",
    ".tox",
    ".mypy_cache",
];

/// H-GIT: git repositories with their state.
#[derive(Debug, Default)]
pub(crate) struct HGit;

impl HGit {
    /// H-GIT.
    pub(crate) fn new() -> Self {
        Self
    }
}

impl Heuristic for HGit {
    fn id(&self) -> &'static str {
        HEURISTIC_ID
    }

    fn run(&self, hctx: &HeuristicContext<'_>) -> HeuristicOutput {
        let (walk, issues) = hctx.roots_walk.get(hctx);
        let mut out = HeuristicOutput {
            issues,
            ..HeuristicOutput::default()
        };
        let home_walk =
            home_root(hctx, walk).map(|home| walk_roots(hctx, &[home], hctx.cfg.git.max_depth));
        let mut repos = Vec::new();
        find_repos(hctx, walk, &mut repos);
        if let Some((home_walk, issues)) = &home_walk {
            out.issues.extend(issues.iter().cloned());
            // Repositories the shared walk found already (e.g. under an extra
            // root inside `{HOME}`), and folders inside its bare ones.
            let found: HashSet<String> = repos.iter().map(|r| path_key(r.path())).collect();
            let bare: PathSet = repos
                .iter()
                .filter(|r| r.bare)
                .map(|r| r.path().to_path_buf())
                .collect();
            let mut home_repos = Vec::new();
            find_repos(hctx, home_walk, &mut home_repos);
            repos.extend(
                home_repos
                    .into_iter()
                    .filter(|r| !found.contains(&path_key(r.path())) && !bare.covers(r.path())),
            );
        }
        link_parents(&mut repos);
        let max = usize::try_from(hctx.cfg.git.max_repos).unwrap_or(usize::MAX);
        let mut outcomes = Vec::with_capacity(repos.len());
        for (n, repo) in repos.iter().enumerate() {
            if hctx.cancel.is_cancelled() {
                return out;
            }
            outcomes.push(if n < max {
                analyze_repo(hctx, repo)
            } else {
                Outcome::NotAnalyzed
            });
        }
        if repos.len() > max {
            out.issues.push(truncated_issue(repos.len() - max, max));
        }
        let merged = merge_submodules(&repos, &mut outcomes);
        build(hctx, &repos, &outcomes, &merged, &mut out);
        out
    }
}

/// A found repository.
struct Repo<'w> {
    /// The walk that found it.
    walk: &'w RootsWalk,
    /// Its folder in [`RootsWalk::dirs`].
    node: usize,
    /// A bare repository (`HEAD` + `objects` + `refs`), not a work tree.
    bare: bool,
    /// The repository it lies in, as an index of the found repositories.
    parent: Option<usize>,
}

impl Repo<'_> {
    fn path(&self) -> &Path {
        &self.walk.dirs[self.node].path
    }

    /// Size of a walked folder of the repository.
    fn walked_bytes(&self, path: &Path) -> Option<u64> {
        let end = self.walk.dirs[self.node].end;
        self.walk.dirs[self.node + 1..end]
            .iter()
            .find(|d| eq_ci(&d.path, path))
            .map(|d| d.subtree.bytes)
    }
}

/// `{HOME}` when the shared walk did not walk it (§4.4.1): it is outside the
/// roots, or a skipped or too deep folder.
fn home_root(hctx: &HeuristicContext<'_>, walk: &RootsWalk) -> Option<PathBuf> {
    let home = hctx.env.known_folder(sk_core::env::KnownFolder::Home)?;
    let walked = walk.dirs.iter().any(|d| eq_ci(&d.path, home));
    (!walked && !walk.cancelled).then(|| home.to_path_buf())
}

/// Appends the repositories of `walk` in walk order: folders marked as work
/// trees or bare repositories that are not claimed. A bare repository is
/// not looked into.
fn find_repos<'w>(hctx: &HeuristicContext<'_>, walk: &'w RootsWalk, repos: &mut Vec<Repo<'w>>) {
    // Ends of the folder ranges of the enclosing bare repositories.
    let mut bare_ends: Vec<usize> = Vec::new();
    for (i, dir) in walk.dirs.iter().enumerate() {
        while bare_ends.last().is_some_and(|&end| i >= end) {
            bare_ends.pop();
        }
        let worktree = dir.is_git_worktree();
        if !worktree && !dir.is_bare_repo() {
            continue;
        }
        if !bare_ends.is_empty() || hctx.claimed.covers(&dir.path) {
            continue;
        }
        repos.push(Repo {
            walk,
            node: i,
            bare: !worktree,
            parent: None,
        });
        if !worktree {
            bare_ends.push(dir.end);
        }
    }
}

/// Case-insensitive key of a path, by components.
fn path_key(path: &Path) -> String {
    let parts: Vec<String> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect();
    parts.join("/")
}

/// Sets each repository's `parent`: the nearest repository it lies in,
/// whichever walk found either of them.
fn link_parents(repos: &mut [Repo<'_>]) {
    let index: HashMap<String, usize> = repos
        .iter()
        .enumerate()
        .map(|(i, r)| (path_key(r.path()), i))
        .collect();
    for repo in repos.iter_mut() {
        repo.parent = repo
            .path()
            .ancestors()
            .skip(1)
            .find_map(|a| index.get(&path_key(a)).copied());
    }
}

/// Reads one repository and sizes its heavy ignored folders. A work tree
/// with files that are only in the cloud is not looked at: reading them
/// would download them.
fn analyze_repo(hctx: &HeuristicContext<'_>, repo: &Repo<'_>) -> Outcome {
    let cloud = repo.walk.dirs[repo.node].subtree.cloud_only_files > 0;
    match analyze(repo.path(), !repo.bare && !cloud, hctx.scanner) {
        Ok(mut analysis) => {
            analysis.state.ignored_heavy_dirs = analysis
                .ignored_dirs
                .iter()
                .filter(|rel| is_heavy_name(rel))
                .filter_map(|rel| {
                    let path = join_rel(repo.path(), rel);
                    let size = repo
                        .walked_bytes(&path)
                        .unwrap_or_else(|| size_above(hctx, &path, HEAVY_BYTES));
                    (size > HEAVY_BYTES).then(|| (rel.clone(), size))
                })
                .collect();
            Outcome::Analyzed(analysis)
        }
        Err(err) => Outcome::Failed(err),
    }
}

/// The last segment of a relative path is one of [`HEAVY_NAMES`] or `.venv*`.
fn is_heavy_name(rel: &str) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel).to_lowercase();
    name.starts_with(".venv") || HEAVY_NAMES.contains(&name.as_str())
}

/// `rel` (with `/`) below `root`.
fn join_rel(root: &Path, rel: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    path.extend(rel.split('/').filter(|s| !s.is_empty()));
    path
}

/// `path` relative to `root`, segments joined with `/`, not escaped.
fn rel_path(path: &Path, root: &Path) -> String {
    let parts: Vec<String> = path
        .components()
        .skip(root.components().count())
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts.join("/")
}

/// Every walk filter decision is `Keep`: an ignored folder is measured
/// whole, global exclusions too (they are what is measured).
#[derive(Debug)]
struct KeepAll;

impl PathFilter for KeepAll {
    fn check(&self, _: &Path, _: &OsStr, _: bool) -> Exclusion {
        Exclusion::Keep
    }
}

/// Size of the files under `path`, counted until it exceeds `limit` (the
/// shared walk does not enter global exclusions such as `node_modules`).
fn size_above(hctx: &HeuristicContext<'_>, path: &Path, limit: u64) -> u64 {
    let opts = WalkOptions {
        max_depth: u32::MAX,
        max_entries: u64::MAX,
        follow_links: false,
        excludes: Arc::new(KeepAll),
        include: None,
        exclude: None,
        threads: 0,
    };
    let mut bytes = 0u64;
    let walked = hctx.scanner.walk(
        path,
        &opts,
        &mut |e| {
            if e.meta.kind == EntryKind::File {
                bytes = bytes.saturating_add(e.meta.size);
            }
            if bytes > limit {
                WalkControl::Stop
            } else {
                WalkControl::Continue
            }
        },
        hctx.cancel,
    );
    if walked.is_ok() || bytes > limit {
        bytes
    } else {
        0
    }
}

/// Folds the submodules into the repositories that hold them, deepest
/// first. Returns which repositories were folded (they give no finding).
fn merge_submodules(repos: &[Repo<'_>], outcomes: &mut [Outcome]) -> Vec<bool> {
    let mut merged = vec![false; repos.len()];
    let mut order: Vec<usize> = (0..repos.len()).collect();
    order.sort_by_key(|&j| std::cmp::Reverse(repos[j].path().components().count()));
    for j in order {
        let Some(p) = repos[j].parent else {
            continue;
        };
        let rel = rel_path(repos[j].path(), repos[p].path());
        let is_submodule = matches!(&outcomes[p], Outcome::Analyzed(a)
            if a.gitlinks.iter().any(|g| g.eq_ignore_ascii_case(&rel)));
        if !is_submodule {
            continue;
        }
        merged[j] = true;
        let sub = match &outcomes[j] {
            Outcome::Analyzed(sub) => sub.state.clone(),
            _ => continue,
        };
        if let Outcome::Analyzed(outer) = &mut outcomes[p] {
            outer.state.add_submodule(&sub, &rel);
        }
    }
    merged
}

/// The nearest repository holding repository `k` that gives a finding: a
/// submodule folded into its outer repository is passed through.
fn finding_parent(repos: &[Repo<'_>], merged: &[bool], k: usize) -> Option<usize> {
    let mut parent = repos[k].parent;
    while let Some(p) = parent.filter(|&p| merged[p]) {
        parent = repos[p].parent;
    }
    parent
}

/// The findings, claimed paths and issues of the repositories.
fn build(
    hctx: &HeuristicContext<'_>,
    repos: &[Repo<'_>],
    outcomes: &[Outcome],
    merged: &[bool],
    out: &mut HeuristicOutput,
) {
    for (j, (repo, outcome)) in repos.iter().zip(outcomes).enumerate() {
        if let Outcome::Failed(err) = outcome {
            out.issues.push(failed_issue(hctx, repo.path(), err));
        }
        if merged[j] {
            continue;
        }
        let path = repo.path();
        let mut exclude: Vec<String> = Vec::new();
        if let Outcome::Analyzed(a) = outcome {
            exclude.extend(
                a.state
                    .ignored_heavy_dirs
                    .iter()
                    .map(|(rel, _)| relative_glob(&join_rel(path, rel), path)),
            );
        }
        let nested: Vec<&Path> = repos
            .iter()
            .enumerate()
            .filter(|&(k, _)| !merged[k] && finding_parent(repos, merged, k) == Some(j))
            .map(|(_, r)| r.path())
            .collect();
        let mut inner: Vec<&Path> = nested;
        inner.extend(hctx.claimed.descendants(path));
        // Outer folders first, so that `topmost` keeps them only.
        inner.sort_by_key(|p| p.components().count());
        for inner in topmost(inner) {
            let glob = relative_glob(&inner, path);
            if !exclude.contains(&glob) {
                exclude.push(glob);
            }
        }
        let node = &repo.walk.dirs[repo.node];
        out.findings
            .push(repo_finding(hctx, node, repo.bare, outcome, exclude));
        out.claimed.push(path.to_path_buf());
    }
}
