//! Default scanner until `RealFs` exists (SPEC-03 T-03-04).

use std::io;
use std::path::Path;

use sk_core::fs::{
    DirEntryInfo, EntryMeta, FsError, FsScanner, Readability, WalkControl, WalkOptions, WalkStats,
};
use sk_core::CancellationToken;

/// Fails every call: the real scanner is not implemented yet.
pub(crate) struct UnavailableFs;

fn unavailable() -> FsError {
    FsError::Io(io::Error::other(
        "file system scanner is not implemented yet (SPEC-03 T-03-04)",
    ))
}

impl FsScanner for UnavailableFs {
    fn metadata(&self, _: &Path) -> Result<EntryMeta, FsError> {
        Err(unavailable())
    }

    fn exists(&self, _: &Path) -> bool {
        false
    }

    fn read_dir(&self, _: &Path) -> Result<Vec<DirEntryInfo>, FsError> {
        Err(unavailable())
    }

    fn walk(
        &self,
        _: &Path,
        _: &WalkOptions,
        _: &mut dyn FnMut(&DirEntryInfo) -> WalkControl,
        _: &CancellationToken,
    ) -> Result<WalkStats, FsError> {
        Err(unavailable())
    }

    fn read_head(&self, _: &Path, _: usize) -> Result<Vec<u8>, FsError> {
        Err(unavailable())
    }

    fn read_small(&self, _: &Path, _: usize) -> Result<Vec<u8>, FsError> {
        Err(unavailable())
    }

    fn probe_readable(&self, _: &Path) -> Readability {
        Readability::Missing
    }
}
