//! File system access interface (SPEC-03 §4.1).
//!
//! The trait and its types live in `sk-core`, because `CollectContext`
//! (SPEC-01 §4.3) refers to them and `sk-core` cannot depend on `sk-scan`.
//! Implementations (`RealFs`, `MemFs`, `ExcludeSet`) and `measure`/`summarize`
//! are in `sk-scan`, which re-exports everything here.
//!
//! Scanning only reads (principle P1): no implementation writes, and none opens
//! cloud-only files or follows reparse points.

use std::ffi::OsStr;
use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use globset::GlobSet;
use time::OffsetDateTime;

use crate::CancellationToken;

/// Read-only access to a file system: the real one or a fake one in tests.
pub trait FsScanner: Send + Sync {
    /// Metadata of one entry, without following reparse points.
    fn metadata(&self, path: &Path) -> Result<EntryMeta, FsError>;

    /// Whether the entry exists; `false` on any problem.
    fn exists(&self, path: &Path) -> bool;

    /// Entries of one directory level.
    fn read_dir(&self, path: &Path) -> Result<Vec<DirEntryInfo>, FsError>;

    /// Parallel recursive traversal; calls `visit` for every entry that is not
    /// excluded. Reparse points are reported but not entered, except cloud
    /// placeholder folders (SPEC-03 §4.2).
    fn walk(
        &self,
        root: &Path,
        opts: &WalkOptions,
        visit: &mut dyn FnMut(&DirEntryInfo) -> WalkControl,
        cancel: &CancellationToken,
    ) -> Result<WalkStats, FsError>;

    /// The first `max` bytes of a file (signatures, VDF/ACF, markers).
    /// Never reads cloud-only files.
    ///
    /// For both reads (SPEC-03 §4.1): the entry itself is checked without
    /// opening it; cloud-only is [`FsError::CloudOnly`] (not opened), a
    /// folder or a reparse point other than a cloud placeholder is
    /// [`FsError::Io`] (links are not followed), a path with `*`/`?` is
    /// [`FsError::NotFound`]; a sharing violation is
    /// [`FsError::SharingViolation`], no access [`FsError::AccessDenied`].
    fn read_head(&self, path: &Path, max: usize) -> Result<Vec<u8>, FsError>;

    /// The whole file if it is at most `max` bytes, else [`FsError::TooLarge`]
    /// (by the size in metadata, or if more than `max` bytes are read; at
    /// most `max + 1` bytes are read). The rules of
    /// [`read_head`](Self::read_head) apply.
    fn read_small(&self, path: &Path, max: usize) -> Result<Vec<u8>, FsError>;

    /// Trial open for reading with full sharing; `Locked` on a sharing violation.
    fn probe_readable(&self, path: &Path) -> Readability;
}

/// Metadata of a file system entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryMeta {
    /// File, directory or reparse point.
    pub kind: EntryKind,
    /// Logical size.
    pub size: u64,
    /// Last modification time.
    pub mtime: Option<OffsetDateTime>,
    /// Creation time.
    pub ctime: Option<OffsetDateTime>,
    /// Raw `FILE_ATTRIBUTE_*` flags; 0 on other platforms.
    pub attrs: u32,
    /// Whether the content is only in the cloud (SPEC-03 §4.2).
    pub cloud: CloudState,
}

/// Kind of an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntryKind {
    /// A regular file.
    File,
    /// A directory.
    Dir,
    /// A reparse point; of these, `walk` enters only cloud placeholder folders
    /// (SPEC-03 §4.2). Cloud placeholders, files and folders alike, are
    /// `Reparse(CloudPlaceholder)`; a folder has `FILE_ATTRIBUTE_DIRECTORY`
    /// in [`EntryMeta::attrs`], a file does not.
    Reparse(ReparseKind),
}

/// Kind of a reparse point, by its tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReparseKind {
    /// `IO_REPARSE_TAG_SYMLINK`.
    Symlink,
    /// `IO_REPARSE_TAG_MOUNT_POINT`.
    Junction,
    /// `IO_REPARSE_TAG_CLOUD*`: OneDrive and similar placeholders.
    CloudPlaceholder,
    /// `IO_REPARSE_TAG_APPEXECLINK`: Store app aliases.
    AppExecLink,
    /// Any other tag.
    Other(u32),
}

/// Where the content of an entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CloudState {
    /// On the local disk.
    Local,
    /// Only in the cloud; opening it would download it.
    CloudOnly,
}

/// An entry found while listing or walking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntryInfo {
    /// Absolute path.
    pub path: PathBuf,
    /// Path relative to the walk root.
    pub rel: PathBuf,
    /// Depth below the walk root; direct children have depth 1.
    pub depth: u32,
    /// Metadata.
    pub meta: EntryMeta,
}

/// What the walk does after visiting an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkControl {
    /// Go on.
    Continue,
    /// Do not enter this directory.
    SkipDir,
    /// End the walk.
    Stop,
}

/// Result of a trial open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readability {
    /// Can be read.
    Ok,
    /// Opened by another process without sharing.
    Locked,
    /// No permission.
    Denied,
    /// Only in the cloud; not opened.
    CloudOnly,
    /// Does not exist.
    Missing,
}

/// Decision of a [`PathFilter`] about one entry.
///
/// The conditions are checked by the walker (SPEC-03 §4.2); the filter itself
/// never touches the file system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Exclusion {
    /// The entry is walked.
    Keep,
    /// The entry is skipped.
    Exclude,
    /// Skip the entry if another entry of the same folder has a name matching
    /// this glob (`*` and `?`, case-insensitive), e.g. `target` next to `Cargo.toml`.
    ExcludeIfSibling(&'static str),
    /// Skip the folder if it directly contains a file with this name,
    /// e.g. `venv` with `pyvenv.cfg`.
    ExcludeIfChild(&'static str),
}

/// Decides which entries a walk skips; implemented by `sk_scan::ExcludeSet` (SPEC-03 §4.5).
pub trait PathFilter: Send + Sync + Debug {
    /// Decision for the entry at the absolute path `abs` named `name`.
    fn check(&self, abs: &Path, name: &OsStr, is_dir: bool) -> Exclusion;
}

/// Parameters of [`FsScanner::walk`].
#[derive(Debug, Clone)]
pub struct WalkOptions {
    /// Maximum depth (`config.scan.max_depth`, 32).
    pub max_depth: u32,
    /// Limit of entries per root (2 000 000 by default); then `truncated`.
    pub max_entries: u64,
    /// Always false in the MVP; reserved for SPEC-17.
    pub follow_links: bool,
    /// Global exclusions.
    pub excludes: Arc<dyn PathFilter>,
    /// Only entries matching these globs, relative to the root.
    pub include: Option<GlobSet>,
    /// Entries matching these globs are skipped, relative to the root.
    pub exclude: Option<GlobSet>,
    /// Number of worker threads.
    pub threads: usize,
}

/// Counters of a finished walk.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WalkStats {
    /// Entries visited.
    pub entries: u64,
    /// Entries skipped by exclusions.
    pub skipped_excluded: u64,
    /// Entries that could not be read.
    pub errors: u64,
    /// Stopped by `max_entries`.
    pub truncated: bool,
}

/// File system error.
#[derive(Debug, thiserror::Error)]
pub enum FsError {
    /// The path does not exist.
    #[error("not found")]
    NotFound,
    /// No permission.
    #[error("access denied")]
    AccessDenied,
    /// Opened by another process without sharing.
    #[error("sharing violation")]
    SharingViolation,
    /// Larger than the allowed size.
    #[error("file too large")]
    TooLarge,
    /// Only in the cloud; not opened.
    #[error("file is only in the cloud")]
    CloudOnly,
    /// The operation was cancelled.
    #[error("cancelled")]
    Cancelled,
    /// Any other I/O error.
    #[error(transparent)]
    Io(std::io::Error),
}

impl From<std::io::Error> for FsError {
    fn from(e: std::io::Error) -> Self {
        // ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION.
        if cfg!(windows) && matches!(e.raw_os_error(), Some(32 | 33)) {
            return FsError::SharingViolation;
        }
        match e.kind() {
            std::io::ErrorKind::NotFound => FsError::NotFound,
            std::io::ErrorKind::PermissionDenied => FsError::AccessDenied,
            _ => FsError::Io(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Error, ErrorKind};

    use super::*;

    #[test]
    fn io_errors_map_to_kinds() {
        assert!(matches!(
            FsError::from(Error::from(ErrorKind::NotFound)),
            FsError::NotFound
        ));
        assert!(matches!(
            FsError::from(Error::from(ErrorKind::PermissionDenied)),
            FsError::AccessDenied
        ));
        assert!(matches!(FsError::from(Error::other("x")), FsError::Io(_)));
    }

    #[cfg(windows)]
    #[test]
    fn sharing_violation_is_recognized() {
        assert!(matches!(
            FsError::from(Error::from_raw_os_error(32)),
            FsError::SharingViolation
        ));
        assert!(matches!(
            FsError::from(Error::from_raw_os_error(5)),
            FsError::AccessDenied
        ));
    }

    #[derive(Debug)]
    struct NoExcludes;

    impl PathFilter for NoExcludes {
        fn check(&self, _: &Path, _: &OsStr, _: bool) -> Exclusion {
            Exclusion::Keep
        }
    }

    /// The trait is object safe and `WalkOptions` can hold any filter.
    #[test]
    fn trait_objects() {
        fn _takes(_: &dyn FsScanner) {}
        let opts = WalkOptions {
            max_depth: 32,
            max_entries: 2_000_000,
            follow_links: false,
            excludes: Arc::new(NoExcludes),
            include: None,
            exclude: None,
            threads: 1,
        };
        assert_eq!(
            opts.excludes.check(Path::new("a"), OsStr::new("a"), false),
            Exclusion::Keep
        );
        assert!(format!("{opts:?}").contains("NoExcludes"));
    }
}
