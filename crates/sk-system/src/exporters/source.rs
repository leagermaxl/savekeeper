//! Read access to the user's files copied by the exporters `fonts`, `hosts` and
//! `powershell-modules` (SPEC-06 T-06-17, SPEC-10 FR-10-15).
//!
//! Before a source file is opened, its cloud state is checked without opening it
//! ([`sk_core::fs::cloud_state`]): a cloud-only file (OneDrive Files On-Demand and the
//! like) is never opened, as that would download it.

use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::Arc;

use sk_core::fs::CloudState;

/// How the copying exporters reach their source files: the real file system, or a fake
/// in tests that marks files as cloud-only and counts opens.
pub(crate) trait SourceFs: Send + Sync {
    /// Whether `path` is local or only in the cloud, without opening it
    /// ([`sk_core::fs::cloud_state`]); a missing path is
    /// [`io::ErrorKind::NotFound`].
    fn cloud_state(&self, path: &Path) -> io::Result<CloudState>;

    /// Opens `path` for reading only (principle P1).
    fn open(&self, path: &Path) -> io::Result<File>;
}

/// The file system of this machine.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RealSourceFs;

impl SourceFs for RealSourceFs {
    fn cloud_state(&self, path: &Path) -> io::Result<CloudState> {
        sk_core::fs::cloud_state(path)
    }

    fn open(&self, path: &Path) -> io::Result<File> {
        File::open(path)
    }
}

/// [`RealSourceFs`] as the shared source of an exporter.
pub(crate) fn real() -> Arc<dyn SourceFs> {
    Arc::new(RealSourceFs)
}

/// Opens `path` for reading unless it is only in the cloud: `Ok(None)` for a cloud-only
/// file, which is not opened. A missing file is [`io::ErrorKind::NotFound`].
pub(crate) fn open_local(source: &dyn SourceFs, path: &Path) -> io::Result<Option<File>> {
    match source.cloud_state(path)? {
        CloudState::CloudOnly => Ok(None),
        CloudState::Local => source.open(path).map(Some),
    }
}
