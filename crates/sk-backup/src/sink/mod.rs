//! Writers of the backup container (SPEC-10 §4.3): a `zip` archive ([`ZipSink`])
//! or a plain folder ([`DirSink`]) behind one interface, [`ArchiveSink`].
//!
//! A sink only writes into the container. The content comes from a reader that
//! the file phase opens (FR-10-05) and wraps for hashing (T-10-05); the sink
//! streams it with a 1 MiB buffer (NFR-10-02) and tells a failed read
//! ([`SinkError::Source`], the backup goes on) from a failed write
//! ([`SinkError::Target`], the backup stops). A failed or cancelled entry is
//! removed from the container again.

// Consumed by the write phases (T-10-07..10); until then only the tests use it.
#![cfg_attr(not(test), allow(dead_code, unused_imports))]

mod dir_sink;
mod zip_sink;

use std::io::{self, ErrorKind, Read, Write};

use sk_core::CancellationToken;
use time::OffsetDateTime;

use crate::FileAttribute;

pub(crate) use dir_sink::DirSink;
pub(crate) use zip_sink::ZipSink;

/// Size of the copy buffer (NFR-10-02).
const COPY_BUFFER: usize = 1 << 20;
/// Cancellation is checked after every this many bytes of one file (§4.3, step 4).
const CANCEL_CHECK_BYTES: u64 = 8 << 20;

/// `FILE_ATTRIBUTE_READONLY`.
const ATTR_READONLY: u32 = 0x1;
/// `FILE_ATTRIBUTE_HIDDEN`.
const ATTR_HIDDEN: u32 = 0x2;
/// `FILE_ATTRIBUTE_SYSTEM`.
const ATTR_SYSTEM: u32 = 0x4;
/// `FILE_ATTRIBUTE_DIRECTORY`.
const ATTR_DIRECTORY: u32 = 0x10;
/// `FILE_ATTRIBUTE_ARCHIVE`.
const ATTR_ARCHIVE: u32 = 0x20;

/// Metadata of one entry to write.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SinkEntry<'a> {
    /// Path in the container, separator `/`, as built by `archive_path` (§4.2).
    pub archive_path: &'a str,
    /// Expected size from the metadata of the source; a `zip` entry from 2 GiB
    /// on is written with zip64 headers (the file may still grow while read).
    pub size_hint: u64,
    /// Modification time to keep.
    pub mtime: Option<OffsetDateTime>,
    /// Creation time to keep.
    pub ctime: Option<OffsetDateTime>,
    /// Attributes to keep.
    pub attributes: &'a [FileAttribute],
}

/// Writer of a backup container.
pub(crate) trait ArchiveSink: Send {
    /// Writes a file with the content of `content`; returns the number of bytes
    /// read (and written uncompressed). Checks `cancel` before the first read
    /// and after every 8 MiB. On any error the entry is not in the container.
    fn add_file(
        &mut self,
        entry: &SinkEntry<'_>,
        content: &mut dyn Read,
        cancel: &CancellationToken,
    ) -> Result<u64, SinkError>;

    /// Writes an empty folder (`EntryKind::Dir`).
    fn add_dir(&mut self, entry: &SinkEntry<'_>) -> Result<(), SinkError>;

    /// Completes the container and flushes it to disk; returns its size in
    /// bytes (`totals.archive_bytes`).
    fn finish(self: Box<Self>) -> Result<u64, SinkError>;
}

/// Failure of a sink operation.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SinkError {
    /// Reading the content failed; the entry was not written (`errors[]`).
    #[error("cannot read the source: {0}")]
    Source(#[source] io::Error),
    /// Writing the container failed (disk full, target lost, …).
    #[error("cannot write the backup: {0}")]
    Target(#[source] io::Error),
    /// An entry with this path is already in the container.
    #[error("entry is already in the backup: {0}")]
    Duplicate(String),
    /// The path is not a valid relative `/`-separated path (`name_invalid`).
    #[error("invalid path in the backup: {0:?}")]
    InvalidPath(String),
    /// The operation was cancelled; the entry was not written.
    #[error("cancelled")]
    Cancelled,
}

/// Checks that `path` is relative, `/`-separated and stays inside the container:
/// no empty, `.` or `..` names, no `\`, `:` or control characters.
fn validate_path(path: &str) -> Result<(), SinkError> {
    let bad_name = |name: &str| {
        name.is_empty()
            || name == "."
            || name == ".."
            || name
                .chars()
                .any(|c| c == '\\' || c == ':' || c.is_control())
    };
    if path.split('/').any(bad_name) {
        return Err(SinkError::InvalidPath(path.to_owned()));
    }
    Ok(())
}

/// `FILE_ATTRIBUTE_*` flags of `attributes`.
fn attribute_bits(attributes: &[FileAttribute]) -> u32 {
    attributes.iter().fold(0, |bits, a| {
        bits | match a {
            FileAttribute::Readonly => ATTR_READONLY,
            FileAttribute::Hidden => ATTR_HIDDEN,
            FileAttribute::System => ATTR_SYSTEM,
            FileAttribute::Archive => ATTR_ARCHIVE,
        }
    })
}

/// Saved attributes of `FILE_ATTRIBUTE_*` flags, in the order of [`FileAttribute`].
fn attributes_of(bits: u32) -> Vec<FileAttribute> {
    [
        (ATTR_READONLY, FileAttribute::Readonly),
        (ATTR_HIDDEN, FileAttribute::Hidden),
        (ATTR_SYSTEM, FileAttribute::System),
        (ATTR_ARCHIVE, FileAttribute::Archive),
    ]
    .into_iter()
    .filter(|(bit, _)| bits & bit != 0)
    .map(|(_, a)| a)
    .collect()
}

/// Windows `FILETIME` of `t`: 100 ns intervals since 1601-01-01 UTC; `None`
/// before 1601.
fn file_time(t: OffsetDateTime) -> Option<u64> {
    const UNIX_EPOCH_AS_FILETIME: i128 = 116_444_736_000_000_000;
    let ticks = t.unix_timestamp_nanos() / 100 + UNIX_EPOCH_AS_FILETIME;
    u64::try_from(ticks).ok()
}

/// Copies `content` to `out` through `buf`; errors of `content` are
/// [`SinkError::Source`], errors of `out` [`SinkError::Target`].
fn copy(
    content: &mut dyn Read,
    out: &mut dyn Write,
    buf: &mut [u8],
    cancel: &CancellationToken,
) -> Result<u64, SinkError> {
    if cancel.is_cancelled() {
        return Err(SinkError::Cancelled);
    }
    let mut total = 0u64;
    let mut unchecked = 0u64;
    loop {
        let n = match content.read(buf) {
            Ok(0) => return Ok(total),
            Ok(n) => n,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(SinkError::Source(e)),
        };
        let chunk = buf.get(..n).unwrap_or(buf);
        out.write_all(chunk).map_err(SinkError::Target)?;
        total += chunk.len() as u64;
        unchecked += chunk.len() as u64;
        if unchecked >= CANCEL_CHECK_BYTES {
            unchecked = 0;
            if cancel.is_cancelled() {
                return Err(SinkError::Cancelled);
            }
        }
    }
}

#[cfg(test)]
mod tests;
