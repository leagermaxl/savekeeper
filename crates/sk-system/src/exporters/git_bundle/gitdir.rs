//! The git folders of a repository and the cloud-only check before git runs (SPEC-06
//! §4.3, `git-bundle`; SPEC-10 FR-10-15).
//!
//! A work tree whose `.git` is a file (linked worktree, submodule gitlink) keeps its
//! git data in the folder of the file's `gitdir:` line, and shares objects and refs
//! with that folder's `commondir`. Git reads all of them, so all of them are checked.

use std::ffi::OsStr;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use sk_core::CancellationToken;

use super::super::source::{self, SourceFs};
use super::cloud::find_cloud_only;
use crate::error::ExportError;

/// Longest `.git` or `commondir` file read (a path and a newline).
const POINTER_LIMIT: u64 = 64 * 1024;

/// What the check before git found.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Precheck {
    /// Git may run.
    Run {
        /// The local git folders: `.git` (or the bare repository), the `gitdir:`
        /// folder and its `commondir`.
        git_dirs: Vec<PathBuf>,
        /// The first cloud-only entry of the work tree (`git status` must not run).
        worktree_cloud_only: Option<PathBuf>,
    },
    /// A cloud-only entry in the git data: git must not run.
    CloudOnly(PathBuf),
    /// The `gitdir:` or `commondir` folder is on a network share: git must not run.
    RemoteGitDir,
}

/// Checks the repository at `repo` before git runs: its git folders for cloud-only
/// entries (attributes only, links not followed), then the work tree (`worktree`).
pub(crate) fn precheck(
    source: &dyn SourceFs,
    repo: &Path,
    worktree: bool,
    cancel: &CancellationToken,
) -> Result<Precheck, ExportError> {
    let git_path = if worktree {
        repo.join(".git")
    } else {
        repo.to_path_buf()
    };
    if let Some(path) = find_cloud_only(source, &git_path, None, cancel)? {
        return Ok(Precheck::CloudOnly(path));
    }
    let mut git_dirs = Vec::new();
    if git_path.is_dir() {
        git_dirs.push(git_path);
    } else if let Some(gitdir) = pointer(source, &git_path, "gitdir:", repo) {
        for dir in [
            Some(gitdir.clone()),
            pointer(source, &gitdir.join("commondir"), "", &gitdir),
        ]
        .into_iter()
        .flatten()
        {
            if is_remote(&dir) {
                return Ok(Precheck::RemoteGitDir);
            }
            if git_dirs.contains(&dir) || !dir.is_dir() {
                continue;
            }
            if let Some(path) = find_cloud_only(source, &dir, None, cancel)? {
                return Ok(Precheck::CloudOnly(path));
            }
            git_dirs.push(dir);
        }
    }
    let worktree_cloud_only = if worktree {
        find_cloud_only(source, repo, Some(OsStr::new(".git")), cancel)?
    } else {
        None
    };
    Ok(Precheck::Run {
        git_dirs,
        worktree_cloud_only,
    })
}

/// Whether one of `git_dirs` has a `sharedindex.*` file (split index), found by name
/// without opening it: reading a split index updates its modification time.
pub(crate) fn has_shared_index(git_dirs: &[PathBuf]) -> bool {
    git_dirs.iter().any(|dir| {
        std::fs::read_dir(dir).is_ok_and(|entries| {
            entries.flatten().any(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with("sharedindex."))
            })
        })
    })
}

/// The path in the small text file `file` after `prefix` (`gitdir: <path>`, or the
/// whole first line of `commondir`), resolved against `base` when relative. `None`
/// when the file is missing, cloud-only (already reported by the check of its folder),
/// unreadable or has no such line.
fn pointer(source: &dyn SourceFs, file: &Path, prefix: &str, base: &Path) -> Option<PathBuf> {
    let mut text = String::new();
    match source::open_local(source, file) {
        Ok(Some(handle)) => {
            if let Err(err) = handle.take(POINTER_LIMIT).read_to_string(&mut text) {
                tracing::debug!(error = %err, "git pointer file not read");
                return None;
            }
        }
        Ok(None) => return None,
        Err(err) => {
            if err.kind() != io::ErrorKind::NotFound {
                tracing::debug!(error = %err, "git pointer file not read");
            }
            return None;
        }
    }
    let line = text.lines().next()?.trim_start_matches('\u{feff}');
    let path = line.strip_prefix(prefix)?.trim();
    if path.is_empty() {
        return None;
    }
    let path = Path::new(path);
    if is_remote(path) {
        return Some(path.to_path_buf());
    }
    // `..` resolved lexically, links are not followed.
    let path = base.join(path);
    Some(std::path::absolute(&path).unwrap_or(path))
}

/// Whether `path` is on a network share: `\\server\share`, `//server/share` or
/// `\\?\UNC\…` (a `\\?\C:\` path is local).
pub(crate) fn is_remote(path: &Path) -> bool {
    let text = path.to_string_lossy().replace('/', "\\");
    match text
        .strip_prefix(r"\\?\")
        .or_else(|| text.strip_prefix(r"\\.\"))
    {
        Some(rest) => rest
            .get(..4)
            .is_some_and(|head| head.eq_ignore_ascii_case(r"UNC\")),
        None => text.starts_with(r"\\"),
    }
}
