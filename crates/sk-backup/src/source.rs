//! Reading of source files for the file phase (SPEC-10 §4.3, FR-10-05, FR-10-15,
//! NFR-10-05).
//!
//! A source file is opened read-only with full sharing, so a program that holds
//! it open for writing (a game writing its save) is not disturbed and does not
//! block the read. A sharing or lock violation is retried by the
//! [`RetryPolicy`]; after the last retry the file is `locked` (FR-10-06). A
//! cloud-only file is never opened (FR-10-15), and a link is not followed.
//!
//! The content is hashed while it is copied ([`HashingReader`]), and after the
//! copy [`SourceFile::changed_during_read`] tells whether the file changed
//! meanwhile (flag `changed_during_read`).

// Consumed by the file phase (T-10-07); until then only the tests use it.
#![cfg_attr(not(test), allow(dead_code))]

use std::fs::{File, Metadata, OpenOptions};
use std::io::{self, ErrorKind, Read};
use std::path::Path;
use std::time::Duration;

use sk_core::fs::CloudState;
use sk_core::CancellationToken;
use time::OffsetDateTime;

use crate::{ErrorCode, FileAttribute, RetryPolicy};

/// `ERROR_SHARING_VIOLATION`.
const ERROR_SHARING_VIOLATION: i32 = 32;
/// `ERROR_LOCK_VIOLATION`.
const ERROR_LOCK_VIOLATION: i32 = 33;
/// `ERROR_FILENAME_EXCED_RANGE`.
const ERROR_FILENAME_EXCED_RANGE: i32 = 206;
/// Cancellation is checked at least this often while waiting for a retry
/// (FR-10-08: cancel within 1 s).
const CANCEL_POLL: Duration = Duration::from_millis(50);

/// Metadata of a source file, taken from its open handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceMeta {
    /// Size in bytes.
    pub size: u64,
    /// Modification time.
    pub mtime: Option<OffsetDateTime>,
    /// Creation time.
    pub ctime: Option<OffsetDateTime>,
    /// Raw `FILE_ATTRIBUTE_*` flags; outside Windows only `READONLY`.
    pub attrs: u32,
}

impl SourceMeta {
    fn of(meta: &Metadata) -> Self {
        Self {
            size: meta.len(),
            mtime: meta.modified().ok().map(OffsetDateTime::from),
            ctime: meta.created().ok().map(OffsetDateTime::from),
            attrs: raw_attributes(meta),
        }
    }

    /// The attributes kept in the manifest and the container.
    pub fn attributes(&self) -> Vec<FileAttribute> {
        crate::sink::attributes_of(self.attrs)
    }
}

/// A source file open for reading.
#[derive(Debug)]
pub(crate) struct SourceFile {
    file: File,
    meta: SourceMeta,
}

impl SourceFile {
    /// Metadata at the moment the file was opened.
    pub fn meta(&self) -> &SourceMeta {
        &self.meta
    }

    /// Whether the file changed while it was read (§4.3): its size or mtime
    /// differ from those at opening, or `bytes_read` (all bytes read until
    /// the end) differs from the size at opening. If the metadata cannot be
    /// queried again, the change cannot be ruled out and the answer is `true`
    /// (the flag is only a warning).
    pub fn changed_during_read(&self, bytes_read: u64) -> bool {
        if bytes_read != self.meta.size {
            return true;
        }
        match self.file.metadata() {
            Ok(now) => {
                let now = SourceMeta::of(&now);
                now.size != self.meta.size || now.mtime != self.meta.mtime
            }
            Err(_) => true,
        }
    }
}

impl Read for SourceFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.file.read(buf)
    }
}

/// Why a source file was not opened.
#[derive(Debug, thiserror::Error)]
pub(crate) enum OpenError {
    /// The content is only in the cloud; the file was not opened (FR-10-15).
    #[error("the file is only in the cloud")]
    CloudOnly,
    /// A sharing or lock violation persisted through all retries (FR-10-05).
    #[error("the file is locked by another process: {0}")]
    Locked(#[source] io::Error),
    /// The file does not exist (any more).
    #[error("the file does not exist: {0}")]
    NotFound(#[source] io::Error),
    /// Access is denied.
    #[error("access to the file is denied: {0}")]
    AccessDenied(#[source] io::Error),
    /// The path is too long for the file system.
    #[error("the path is too long: {0}")]
    PathTooLong(#[source] io::Error),
    /// The path is a folder or a link, not a regular file.
    #[error("not a regular file")]
    NotAFile,
    /// Another I/O error.
    #[error("cannot open the file: {0}")]
    Io(#[source] io::Error),
    /// Cancelled while waiting for a retry.
    #[error("cancelled")]
    Cancelled,
}

impl OpenError {
    /// The manifest error code (§4.4); `Locked` goes to `locked[]` instead of
    /// `errors[]`, its code is used only where an error code is needed.
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::CloudOnly => ErrorCode::CloudOnly,
            Self::Locked(_) => ErrorCode::Locked,
            Self::NotFound(_) => ErrorCode::NotFound,
            Self::AccessDenied(_) => ErrorCode::AccessDenied,
            Self::PathTooLong(_) => ErrorCode::PathTooLong,
            Self::NotAFile | Self::Io(_) => ErrorCode::Io,
            Self::Cancelled => ErrorCode::Cancelled,
        }
    }

    fn from_io(e: io::Error) -> Self {
        if is_lock_error(&e) {
            return Self::Locked(e);
        }
        if e.raw_os_error() == Some(ERROR_FILENAME_EXCED_RANGE) {
            return Self::PathTooLong(e);
        }
        match e.kind() {
            ErrorKind::NotFound => Self::NotFound(e),
            ErrorKind::PermissionDenied => Self::AccessDenied(e),
            _ => Self::Io(e),
        }
    }
}

/// Whether `e` is a sharing or lock violation (`ERROR_SHARING_VIOLATION`,
/// `ERROR_LOCK_VIOLATION`), which FR-10-05 retries. A lock violation can also
/// come from a read (a byte range locked by `LockFileEx`); the file phase then
/// retries the whole entry. Always `false` outside Windows.
pub(crate) fn is_lock_error(e: &io::Error) -> bool {
    cfg!(windows)
        && matches!(
            e.raw_os_error(),
            Some(ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION)
        )
}

/// Opens the source file at `path` for reading (FR-10-05).
///
/// Order: cancellation, then the cloud state from the attributes of the entry
/// itself, without opening it (`sk_core::fs::cloud_state`; cloud-only is
/// [`OpenError::CloudOnly`]), then the open with retries by `policy` on a
/// sharing or lock violation, waiting in steps so that `cancel` is honoured
/// within 50 ms ([`OpenError::Cancelled`]).
///
/// On Windows the extended path (`\\?\`) is opened with
/// `GENERIC_READ | FILE_READ_ATTRIBUTES`, sharing `READ | WRITE | DELETE`,
/// `FILE_FLAG_BACKUP_SEMANTICS` (the privilege is not requested),
/// `FILE_FLAG_OPEN_NO_RECALL` (a file that became cloud-only meanwhile is not
/// downloaded) and `FILE_FLAG_OPEN_REPARSE_POINT` (a link is not followed).
/// The source is never opened for writing (NFR-10-05). A folder or a link
/// (symlink, junction) is [`OpenError::NotAFile`]; a cloud placeholder whose
/// content is local is a regular file.
pub(crate) fn open_source(
    path: &Path,
    policy: &RetryPolicy,
    cancel: &CancellationToken,
) -> Result<SourceFile, OpenError> {
    if cancel.is_cancelled() {
        return Err(OpenError::Cancelled);
    }
    match sk_core::fs::cloud_state(path) {
        Ok(CloudState::CloudOnly) => return Err(OpenError::CloudOnly),
        Ok(CloudState::Local) => {}
        Err(e) => return Err(OpenError::from_io(e)),
    }
    // Outside Windows there is no flag to open a link itself: the entry is
    // checked first, so a link is not followed.
    #[cfg(not(windows))]
    if std::fs::symlink_metadata(path)
        .map_err(OpenError::from_io)?
        .file_type()
        .is_symlink()
    {
        return Err(OpenError::NotAFile);
    }
    let file = with_retries(policy, &mut |d| wait(d, cancel), &mut || {
        open_read_only(path).map_err(OpenError::from_io)
    })?;
    let meta = file.metadata().map_err(OpenError::from_io)?;
    if !meta.is_file() {
        return Err(OpenError::NotAFile);
    }
    Ok(SourceFile {
        meta: SourceMeta::of(&meta),
        file,
    })
}

/// Runs `op`; on [`OpenError::Locked`] retries it up to `policy.attempts`
/// times, calling `wait` with the delay before each retry (the last delay
/// repeats; no delays means no wait). `wait` returns `false` when cancelled.
/// Other errors are returned at once; after the last retry the last
/// `Locked` error is returned.
fn with_retries<T>(
    policy: &RetryPolicy,
    wait: &mut dyn FnMut(Duration) -> bool,
    op: &mut dyn FnMut() -> Result<T, OpenError>,
) -> Result<T, OpenError> {
    let mut retry = 0u32;
    loop {
        match op() {
            Err(OpenError::Locked(e)) if retry < policy.attempts => {
                let index = usize::try_from(retry).unwrap_or(usize::MAX);
                let delay_ms = policy
                    .delays_ms
                    .get(index)
                    .or(policy.delays_ms.last())
                    .copied()
                    .unwrap_or(0);
                drop(e);
                if !wait(Duration::from_millis(delay_ms)) {
                    return Err(OpenError::Cancelled);
                }
                retry += 1;
            }
            result => return result,
        }
    }
}

/// Sleeps for `delay` in steps of at most [`CANCEL_POLL`]; `false` if `cancel`
/// was triggered before or during the wait.
fn wait(delay: Duration, cancel: &CancellationToken) -> bool {
    let mut left = delay;
    loop {
        if cancel.is_cancelled() {
            return false;
        }
        if left.is_zero() {
            return true;
        }
        let step = left.min(CANCEL_POLL);
        std::thread::sleep(step);
        left -= step;
    }
}

/// Opens `path` read-only as described in [`open_source`].
fn open_read_only(path: &Path) -> io::Result<File> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const GENERIC_READ: u32 = 0x8000_0000;
        const FILE_READ_ATTRIBUTES: u32 = 0x80;
        const FILE_SHARE_ALL: u32 = 0x1 | 0x2 | 0x4;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_FLAG_OPEN_NO_RECALL: u32 = 0x0010_0000;
        OpenOptions::new()
            .access_mode(GENERIC_READ | FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_ALL)
            .custom_flags(
                FILE_FLAG_BACKUP_SEMANTICS
                    | FILE_FLAG_OPEN_REPARSE_POINT
                    | FILE_FLAG_OPEN_NO_RECALL,
            )
            .open(sk_core::path::to_extended(path))
    }
    #[cfg(not(windows))]
    {
        OpenOptions::new().read(true).open(path)
    }
}

/// `FILE_ATTRIBUTE_*` flags of `meta`.
fn raw_attributes(meta: &Metadata) -> u32 {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes()
    }
    #[cfg(not(windows))]
    {
        u32::from(meta.permissions().readonly())
    }
}

/// Reader that hashes (blake3) and counts everything read through it, so the
/// content is hashed in the same pass that copies it (§4.3).
#[derive(Debug)]
pub(crate) struct HashingReader<R> {
    inner: R,
    hasher: blake3::Hasher,
    bytes: u64,
}

/// Hash and size of the content read through a [`HashingReader`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Hashed {
    /// blake3 of the content, 64 lowercase hex digits (`ManifestEntry.blake3`).
    pub blake3: String,
    /// Number of bytes read.
    pub bytes: u64,
}

impl<R: Read> HashingReader<R> {
    /// Wraps `inner`.
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            hasher: blake3::Hasher::new(),
            bytes: 0,
        }
    }

    /// Number of bytes read so far.
    pub fn bytes_read(&self) -> u64 {
        self.bytes
    }

    /// Hash and size of everything read so far.
    pub fn finish(&self) -> Hashed {
        Hashed {
            blake3: self.hasher.finalize().to_hex().to_string(),
            bytes: self.bytes,
        }
    }
}

impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        let chunk = buf.get(..n).unwrap_or(buf);
        self.hasher.update(chunk);
        self.bytes += chunk.len() as u64;
        Ok(n)
    }
}

#[cfg(test)]
#[path = "source_tests.rs"]
mod tests;

#[cfg(all(test, windows))]
#[path = "source_win_tests.rs"]
mod win_tests;
