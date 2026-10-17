//! Output of `git status --porcelain=v1 -z` (SPEC-06 §4.3, `git-bundle`): the paths of
//! the changed and untracked files of a working copy.

use std::path::{Component, Path, PathBuf};

/// Paths of the entries of `git status --porcelain=v1 -z` output, relative to the
/// repository root, in the order git lists them.
///
/// An entry is `XY <path>` followed by NUL; a rename or copy (`R`/`C` in `XY`) is
/// followed by one more NUL-terminated entry with the original path, which is
/// skipped (the original no longer holds the changed content). A last entry without
/// its NUL (output cut at the size limit) and malformed entries are dropped.
pub(crate) fn changed_paths(stdout: &str) -> Vec<String> {
    let mut entries: Vec<&str> = stdout.split('\0').collect();
    // The text after the last NUL is empty or an incomplete entry.
    entries.pop();
    let mut paths = Vec::new();
    let mut entries = entries.into_iter();
    while let Some(entry) = entries.next() {
        let Some((status, path)) = entry.split_at_checked(3) else {
            tracing::debug!("malformed git status entry");
            continue;
        };
        if !status.ends_with(' ') || path.is_empty() {
            tracing::debug!("malformed git status entry");
            continue;
        }
        if status[..2].contains(['R', 'C']) {
            entries.next();
        }
        paths.push(path.to_owned());
    }
    paths
}

/// The relative path of a `git status` path (`/`-separated, relative to the repository
/// root): `None` when it is empty, absolute or leaves the repository (`..`), so that a
/// copy never reads outside the repository or writes outside the target folder.
pub(crate) fn safe_relative(path: &str) -> Option<PathBuf> {
    let path = path.trim_end_matches('/');
    let relative = Path::new(path);
    let mut out = PathBuf::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}
