//! Cloud-only check of a repository before git runs (SPEC-06 §4.3, `git-bundle`; SPEC-10
//! FR-10-15): git would download every cloud-only file it reads, so the repository is
//! searched for one first, by attributes only, without opening files or following links.

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

use sk_core::fs::CloudState;
use sk_core::CancellationToken;

use super::super::source::SourceFs;
use crate::error::ExportError;

/// The first cloud-only entry at or under `root` (`root` itself included), or `None`.
///
/// A cloud-only folder is reported without being listed. Links and junctions are not
/// followed; the child of `root` named `skip` (`.git` for the work tree) is left out.
/// Entries that cannot be read are skipped (git reports them itself). Cancellation is
/// checked per folder.
pub(crate) fn find_cloud_only(
    source: &dyn SourceFs,
    root: &Path,
    skip: Option<&OsStr>,
    cancel: &CancellationToken,
) -> Result<Option<PathBuf>, ExportError> {
    if is_cloud_only(source, root) {
        return Ok(Some(root.to_path_buf()));
    }
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        if cancel.is_cancelled() {
            return Err(ExportError::Cancelled);
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) => {
                tracing::debug!(error = %err, "folder not checked for cloud-only files");
                continue;
            }
        };
        for entry in entries {
            let Ok(entry) = entry else { continue };
            if dir == root && skip.is_some_and(|skip| entry.file_name() == skip) {
                continue;
            }
            let path = entry.path();
            if is_cloud_only(source, &path) {
                return Ok(Some(path));
            }
            match entry.file_type() {
                // `is_dir` is false for links and junctions: they are not followed.
                Ok(kind) if kind.is_dir() => dirs.push(path),
                Ok(_) => {}
                Err(err) => tracing::debug!(error = %err, "entry type not read"),
            }
        }
    }
    Ok(None)
}

/// Whether `path` is only in the cloud; unreadable attributes count as local.
fn is_cloud_only(source: &dyn SourceFs, path: &Path) -> bool {
    match source.cloud_state(path) {
        Ok(CloudState::CloudOnly) => true,
        Ok(CloudState::Local) => false,
        Err(err) => {
            if err.kind() != io::ErrorKind::NotFound {
                tracing::debug!(error = %err, "cloud state not read");
            }
            false
        }
    }
}
