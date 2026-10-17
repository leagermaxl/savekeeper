//! H-GIT: the state of one repository read through gix (SPEC-07 §4.4.2).
//!
//! Only reads (NFR-07-05): the repository is opened isolated (only its own
//! configuration, no environment, no global or system configuration), the
//! filter drivers of its configuration (`filter.<name>.*`, e.g. Git LFS) are
//! dropped in memory so that no external program runs, and the stat updates
//! a status finds are never written back to the index.

use std::collections::BTreeSet;
use std::fmt::Display;
use std::path::Path;

use gix::bstr::{BString, ByteSlice};
use gix::dir::entry::{Kind as DiskKind, Status as DirStatus};
use gix::dir::walk::EmissionMode;
use gix::hash::ObjectId;
use gix::remote::Direction;
use gix::status::plumbing::index_as_worktree::EntryStatus;
use gix::status::{index_worktree, Item, Submodule, UntrackedFiles};
use gix::submodule::config::Ignore;
use sk_core::fs::FsScanner;
use time::OffsetDateTime;

/// Untracked files are counted up to this number (SPEC-07 §4.4.2).
pub(crate) const MAX_UNTRACKED: u32 = 10_000;
/// Unpushed commits are counted up to this number (SPEC-07 §4.4.2).
pub(crate) const MAX_UNPUSHED: u32 = 1_000;
/// `.gitattributes` is read up to this size to look for Git LFS.
const MAX_ATTRIBUTES_BYTES: usize = 1024 * 1024;
/// The remote preferred for the host in the evidence.
const ORIGIN: &str = "origin";

/// State of a repository (SPEC-07 §4.4.2).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct GitState {
    /// Short name of the checked-out branch; `None` when `HEAD` is detached.
    pub head_branch: Option<String>,
    /// `HEAD` points at a commit, not at a branch.
    pub detached: bool,
    /// Names of the remotes; URLs are not kept.
    pub remotes: Vec<String>,
    /// Changed tracked files: staged or not, conflicts, intent-to-add.
    pub dirty_files: u32,
    /// Untracked files that are not ignored, counted up to [`MAX_UNTRACKED`].
    pub untracked_files: u32,
    /// Σ over the local branches of the commits ahead of their upstream; a
    /// branch without an upstream counts whole. Commits of a detached `HEAD`
    /// that no branch holds count too. Up to [`MAX_UNPUSHED`].
    pub unpushed_commits: u32,
    /// Local branches without an upstream.
    pub local_only_branches: u32,
    /// Entries of the `refs/stash` reflog.
    pub stash_entries: u32,
    /// The repository uses Git LFS.
    pub has_lfs: bool,
    /// Ignored heavy folders of the work tree: path relative to the
    /// repository with `/`, and size (set by H-GIT from the walk).
    pub ignored_heavy_dirs: Vec<(String, u64)>,
    /// Time of the commit of `HEAD`.
    pub last_commit_time: Option<OffsetDateTime>,
}

impl GitState {
    /// `dirty + untracked + unpushed + stash` (SPEC-07 §4.4.3).
    pub(crate) fn unsaved(&self) -> u64 {
        [
            self.dirty_files,
            self.untracked_files,
            self.unpushed_commits,
            self.stash_entries,
        ]
        .iter()
        .map(|&n| u64::from(n))
        .sum()
    }

    /// Adds the state of a submodule: its changes count as changes of the
    /// repository that holds it; `prefix` is its path in that repository.
    pub(crate) fn add_submodule(&mut self, sub: &GitState, prefix: &str) {
        self.dirty_files = self.dirty_files.saturating_add(sub.dirty_files);
        self.untracked_files = self
            .untracked_files
            .saturating_add(sub.untracked_files)
            .min(MAX_UNTRACKED);
        self.unpushed_commits = self
            .unpushed_commits
            .saturating_add(sub.unpushed_commits)
            .min(MAX_UNPUSHED);
        self.local_only_branches = self
            .local_only_branches
            .saturating_add(sub.local_only_branches);
        self.stash_entries = self.stash_entries.saturating_add(sub.stash_entries);
        self.has_lfs |= sub.has_lfs;
        self.ignored_heavy_dirs.extend(
            sub.ignored_heavy_dirs
                .iter()
                .map(|(dir, size)| (format!("{prefix}/{dir}"), *size)),
        );
    }
}

/// What the analysis of a repository gives besides [`GitState`].
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Analysis {
    /// The state; `ignored_heavy_dirs` is left empty.
    pub state: GitState,
    /// Host of the `origin` remote (or of the first one), without user name
    /// or password; empty for a local path or without remotes.
    pub remote_host: String,
    /// Ignored folders of the work tree, relative with `/`.
    pub ignored_dirs: Vec<String>,
    /// Submodules: paths of the gitlinks of the index, relative with `/`.
    pub gitlinks: Vec<String>,
    /// A submodule (or one of its own) could not be read: gix failed or the
    /// limit of repositories was reached. Set by H-GIT; the repository is
    /// then not taken for clean.
    pub unread_submodules: bool,
}

/// Reads the state of the repository at `path`: a work tree (holding `.git`)
/// or a bare repository. With `worktree = false` the work tree is not looked
/// at (no status, no untracked files), e.g. when it holds files that are only
/// in the cloud and reading them would download them.
pub(crate) fn analyze(
    path: &Path,
    worktree: bool,
    scanner: &dyn FsScanner,
) -> Result<Analysis, String> {
    let mut repo = gix::open_opts(path, gix::open::Options::isolated()).map_err(text)?;
    without_filter_drivers(&mut repo)?;
    let mut out = Analysis::default();
    let state = &mut out.state;
    state.head_branch = repo
        .head_name()
        .map_err(text)?
        .map(|name| name.shorten().to_string());
    let head = repo.head().map_err(text)?;
    state.detached = head.is_detached();
    let head_id = head.id().map(|id| id.detach());
    if let Some(id) = head_id {
        let commit = repo.find_commit(id).map_err(text)?;
        let time = commit.time().map_err(text)?;
        state.last_commit_time = OffsetDateTime::from_unix_timestamp(time.seconds).ok();
    }
    let names = repo.remote_names();
    state.remotes = names.iter().map(|n| n.to_string()).collect();
    out.remote_host = remote_host(&repo, &state.remotes);
    branches(&repo, state, head_id.filter(|_| state.detached))?;
    state.stash_entries = stash_entries(&repo)?;
    state.has_lfs = repo.common_dir().join("lfs").is_dir();
    if let Some(work) = repo.workdir().map(Path::to_path_buf) {
        state.has_lfs |= uses_lfs(&work, scanner);
        if worktree {
            status(&repo, &mut out)?;
        }
        let index = repo.index_or_empty().map_err(text)?;
        out.gitlinks = index
            .entries()
            .iter()
            .filter(|e| e.mode == gix::index::entry::Mode::COMMIT)
            .map(|e| e.path(&index).to_string())
            .collect();
    }
    Ok(out)
}

fn text(err: impl Display) -> String {
    err.to_string()
}

/// Drops the `filter` sections of the configuration, in memory only: a
/// status would otherwise run their programs (`git-lfs` stores the files it
/// cleans in `.git/lfs`).
fn without_filter_drivers(repo: &mut gix::Repository) -> Result<(), String> {
    let mut config = repo.config_snapshot_mut();
    let ids: Vec<_> = config
        .sections_and_ids_by_name("filter")
        .into_iter()
        .flatten()
        .map(|(_, id)| id)
        .collect();
    for id in ids {
        config.remove_section_by_id(id);
    }
    config.commit().map_err(text)?;
    Ok(())
}

/// Host of the `origin` remote or of the first one.
fn remote_host(repo: &gix::Repository, names: &[String]) -> String {
    let name = names
        .iter()
        .find(|n| n.as_str() == ORIGIN)
        .or_else(|| names.first());
    name.and_then(|n| repo.find_remote(n.as_str()).ok())
        .and_then(|r| {
            r.url(Direction::Fetch)
                .and_then(|u| u.host().map(str::to_owned))
        })
        .unwrap_or_default()
}

/// Unpushed commits and local-only branches; `detached` is the commit of a
/// detached `HEAD`.
fn branches(
    repo: &gix::Repository,
    state: &mut GitState,
    detached: Option<ObjectId>,
) -> Result<(), String> {
    let refs = repo.references().map_err(text)?;
    let mut locals = Vec::new();
    for r in refs.local_branches().map_err(text)? {
        let mut r = r.map_err(text)?;
        let id = r.peel_to_id().map_err(text)?.detach();
        locals.push((r.name().to_owned(), id));
    }
    let mut tips: Vec<ObjectId> = locals.iter().map(|(_, id)| *id).collect();
    for r in refs.remote_branches().map_err(text)? {
        let mut r = r.map_err(text)?;
        tips.push(r.peel_to_id().map_err(text)?.detach());
    }
    let mut budget = MAX_UNPUSHED;
    for (name, tip) in &locals {
        let upstream = match repo.branch_remote_tracking_ref_name(name.as_ref(), Direction::Fetch) {
            Some(Ok(tracking)) => match repo.try_find_reference(tracking.as_ref()) {
                Ok(Some(mut r)) => Some(r.peel_to_id().map_err(text)?.detach()),
                Ok(None) => None,
                Err(err) => return Err(text(err)),
            },
            _ => None,
        };
        if upstream.is_none() {
            state.local_only_branches += 1;
        }
        let ahead = count_commits(repo, *tip, upstream, budget)?;
        budget -= ahead;
        state.unpushed_commits += ahead;
    }
    if let Some(head) = detached {
        let walk = repo.rev_walk([head]).with_hidden(tips);
        let mut ahead = 0;
        for info in walk.all().map_err(text)? {
            info.map_err(text)?;
            if ahead >= budget {
                break;
            }
            ahead += 1;
        }
        state.unpushed_commits += ahead;
    }
    Ok(())
}

/// Commits of `tip` that `hidden` does not hold, up to `max`.
fn count_commits(
    repo: &gix::Repository,
    tip: ObjectId,
    hidden: Option<ObjectId>,
    max: u32,
) -> Result<u32, String> {
    let mut n = 0;
    for info in repo
        .rev_walk([tip])
        .with_hidden(hidden)
        .all()
        .map_err(text)?
    {
        info.map_err(text)?;
        if n >= max {
            break;
        }
        n += 1;
    }
    Ok(n)
}

/// Entries of the `refs/stash` reflog; 1 for a stash without a reflog.
fn stash_entries(repo: &gix::Repository) -> Result<u32, String> {
    let Some(stash) = repo.try_find_reference("refs/stash").map_err(text)? else {
        return Ok(0);
    };
    let mut log = stash.log_iter();
    let count = match log.all().map_err(text)? {
        Some(lines) => lines.filter(Result::is_ok).count(),
        None => 1,
    };
    Ok(u32::try_from(count).unwrap_or(u32::MAX))
}

/// `.gitattributes` of the work tree root sends files to the `lfs` filter.
fn uses_lfs(work: &Path, scanner: &dyn FsScanner) -> bool {
    scanner
        .read_small(&work.join(".gitattributes"), MAX_ATTRIBUTES_BYTES)
        .is_ok_and(|text| text.find(b"filter=lfs").is_some())
}

/// Changed tracked files, untracked files and ignored folders of the work tree.
fn status(repo: &gix::Repository, out: &mut Analysis) -> Result<(), String> {
    let items = repo
        .status(gix::progress::Discard)
        .map_err(text)?
        .untracked_files(UntrackedFiles::Files)
        .index_worktree_rewrites(None)
        // The commit of a submodule is compared; its work tree is analyzed
        // on its own (with the filter drivers dropped) and added by H-GIT.
        .index_worktree_submodules(Submodule::Given {
            ignore: Ignore::Dirty,
            check_dirty: false,
        })
        .dirwalk_options(|o| {
            o.emit_ignored(Some(EmissionMode::Matching))
                .recurse_repositories(false)
                .emit_empty_directories(false)
        })
        .into_iter(Vec::<BString>::new())
        .map_err(text)?;
    let mut dirty: BTreeSet<BString> = BTreeSet::new();
    let mut untracked = 0u32;
    for item in items {
        match item.map_err(text)? {
            Item::TreeIndex(change) => {
                dirty.insert(change.location().to_owned());
            }
            Item::IndexWorktree(index_worktree::Item::Modification {
                rela_path, status, ..
            }) => {
                if !matches!(status, EntryStatus::NeedsUpdate(_)) {
                    dirty.insert(rela_path);
                }
            }
            Item::IndexWorktree(index_worktree::Item::DirectoryContents { entry, .. }) => {
                match (entry.status, entry.disk_kind) {
                    (DirStatus::Untracked, Some(DiskKind::File | DiskKind::Symlink)) => {
                        untracked = (untracked + 1).min(MAX_UNTRACKED);
                    }
                    (DirStatus::Ignored(_), Some(DiskKind::Directory)) => {
                        out.ignored_dirs.push(entry.rela_path.to_string());
                    }
                    _ => {}
                }
            }
            Item::IndexWorktree(index_worktree::Item::Rewrite { dirwalk_entry, .. }) => {
                dirty.insert(dirwalk_entry.rela_path);
            }
        }
    }
    out.state.dirty_files = u32::try_from(dirty.len()).unwrap_or(u32::MAX);
    out.state.untracked_files = untracked;
    out.ignored_dirs.sort();
    Ok(())
}
