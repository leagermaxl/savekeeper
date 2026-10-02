//! Stubs outside Windows: no reparse tags and no cloud attributes, trial
//! opens through `std::fs`.

use std::path::Path;

use sk_core::fs::{FsError, Readability};

/// Always `None` for an existing entry: other platforms have no reparse tags.
pub(crate) fn reparse_tag(path: &Path) -> Result<Option<u32>, FsError> {
    std::fs::symlink_metadata(path)?;
    Ok(None)
}

/// Trial open for reading (a listing for directories); nothing is read.
///
/// Links are not followed (FR-03-02): a symlink is checked as an entry of its
/// own and is `Ok` even if its target is missing or unreadable.
pub(crate) fn probe_readable(path: &Path) -> Readability {
    let opened = match std::fs::symlink_metadata(path) {
        Err(e) => Err(e),
        Ok(meta) if meta.file_type().is_symlink() => Ok(()),
        Ok(meta) if meta.is_dir() => std::fs::read_dir(path).map(drop),
        Ok(_) => std::fs::File::open(path).map(drop),
    };
    match opened.map_err(FsError::from) {
        Ok(()) => Readability::Ok,
        Err(FsError::NotFound) => Readability::Missing,
        Err(FsError::SharingViolation) => Readability::Locked,
        Err(_) => Readability::Denied,
    }
}
