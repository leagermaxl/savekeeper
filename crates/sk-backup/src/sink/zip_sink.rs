//! `zip` container (SPEC-10 §4.3, NFR-10-01, NFR-10-03, NFR-10-04).
//!
//! - Method: `Stored` for already compressed formats and at level 0, else
//!   `Deflated` at `compression_level`.
//! - zip64 headers for entries from 2 GiB on; the archive itself switches to
//!   zip64 records when needed.
//! - Names are UTF-8; non-ASCII names get the EFS flag (bit 11). An ASCII name
//!   reads the same in UTF-8 and in CP437, so it needs none.
//! - Times: the DOS time in local time (as Explorer and Info-ZIP expect), the
//!   extended timestamp (0x5455, Unix seconds) and the NTFS field (0x000a,
//!   `FILETIME`, 100 ns). Attributes: MS-DOS external attributes.

use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Read, Seek, Write};
use std::path::Path;

use sk_core::path::to_extended;
use sk_core::CancellationToken;
use time::{OffsetDateTime, UtcOffset};
use zip::result::ZipError;
use zip::write::{FullFileOptions, SimpleFileOptions};
use zip::{CompressionMethod, DateTime, System, ZipWriter};

use super::{
    attribute_bits, copy, file_time, validate_path, ArchiveSink, SinkEntry, SinkError,
    ATTR_DIRECTORY, COPY_BUFFER,
};

/// Extensions of already compressed formats, written `Stored` (NFR-10-01).
const STORED_EXTENSIONS: [&str; 11] = [
    "zip", "7z", "rar", "jpg", "png", "mp4", "mkv", "mp3", "gz", "zst", "pak",
];

/// Entries of at least this expected size get zip64 headers. Below 4 GiB, so
/// that a file growing while it is read still fits.
const LARGE_FILE_BYTES: u64 = 2 << 30;

/// Writes a `zip` archive.
pub(crate) struct ZipSink<W: Write + Seek = BufWriter<File>> {
    writer: ZipWriter<W>,
    compression_level: u8,
    buf: Vec<u8>,
}

impl ZipSink {
    /// Creates a new archive file at `path` (never overwrites one) with the
    /// `compression_level` of `BackupOptions` (0..=9).
    pub fn create(path: &Path, compression_level: u8) -> Result<Self, SinkError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(to_extended(path))
            .map_err(SinkError::Target)?;
        Ok(Self::new(
            BufWriter::with_capacity(COPY_BUFFER, file),
            compression_level,
        ))
    }
}

impl<W: Write + Seek> ZipSink<W> {
    /// A sink over any seekable writer.
    pub fn new(inner: W, compression_level: u8) -> Self {
        Self {
            writer: ZipWriter::new(inner),
            compression_level: compression_level.min(9),
            buf: vec![0; COPY_BUFFER],
        }
    }

    /// Completes the archive and returns the writer.
    pub fn finish_into_inner(self) -> Result<W, SinkError> {
        self.writer.finish().map_err(target)
    }

    /// Options of an entry: method, times, attributes, zip64.
    fn options(&self, entry: &SinkEntry<'_>, dir: bool) -> FullFileOptions<'static, 'static> {
        let mut attrs = attribute_bits(entry.attributes);
        let method = if dir {
            attrs |= ATTR_DIRECTORY;
            CompressionMethod::Stored
        } else {
            method_for(entry.archive_path, self.compression_level)
        };
        let level =
            (method == CompressionMethod::Deflated).then_some(i64::from(self.compression_level));
        let mut options = SimpleFileOptions::default()
            .system(System::Dos)
            .external_attributes(attrs)
            .compression_method(method)
            .compression_level(level)
            .last_modified_time(entry.mtime.map_or(DateTime::DEFAULT, dos_time))
            .large_file(!dir && entry.size_hint >= LARGE_FILE_BYTES)
            .into_full_options();
        let Some(mtime) = entry.mtime else {
            return options;
        };
        // A failure (extra data too long) only loses the precise times.
        let unix = |t: OffsetDateTime| u32::try_from(t.unix_timestamp()).ok();
        if let Some(m) = unix(mtime) {
            let ctime = entry.ctime.and_then(unix);
            if let Ok(o) = options
                .clone()
                .with_extended_timestamp(Some(m), None, ctime)
            {
                options = o;
            }
        }
        if let Some(m) = file_time(mtime) {
            // NTFS field 0x000a; the access time is the modification time.
            let c = entry.ctime.and_then(file_time).unwrap_or(m);
            if let Ok(o) = options.clone().with_ntfs_times(m, m, c) {
                options = o;
            }
        }
        options
    }
}

impl<W: Write + Seek + Send> ArchiveSink for ZipSink<W> {
    fn add_file(
        &mut self,
        entry: &SinkEntry<'_>,
        content: &mut dyn Read,
        cancel: &CancellationToken,
    ) -> Result<u64, SinkError> {
        validate_path(entry.archive_path)?;
        if cancel.is_cancelled() {
            return Err(SinkError::Cancelled);
        }
        let options = self.options(entry, false);
        self.writer
            .start_file(entry.archive_path, options)
            .map_err(|e| start_error(e, entry.archive_path))?;
        let result = copy(content, &mut self.writer, &mut self.buf, cancel);
        if result.is_err() {
            self.writer.abort_file().map_err(target)?;
        }
        result
    }

    fn add_dir(&mut self, entry: &SinkEntry<'_>) -> Result<(), SinkError> {
        validate_path(entry.archive_path)?;
        let options = self.options(entry, true);
        self.writer
            .add_directory(entry.archive_path, options)
            .map_err(|e| start_error(e, entry.archive_path))
    }

    fn finish(self: Box<Self>) -> Result<u64, SinkError> {
        let mut inner = self.finish_into_inner()?;
        inner.flush().map_err(SinkError::Target)?;
        inner.stream_position().map_err(SinkError::Target)
    }
}

/// Method for a file: `Stored` for compressed formats and at level 0 (NFR-10-01).
pub(super) fn method_for(archive_path: &str, compression_level: u8) -> CompressionMethod {
    let name = archive_path.rsplit('/').next().unwrap_or(archive_path);
    let compressed = name.rsplit_once('.').is_some_and(|(_, ext)| {
        STORED_EXTENSIONS
            .iter()
            .any(|e| ext.eq_ignore_ascii_case(e))
    });
    if compression_level == 0 || compressed {
        CompressionMethod::Stored
    } else {
        CompressionMethod::Deflated
    }
}

/// DOS date and time of `t` in local time; 1980-01-01 outside 1980..=2107.
pub(super) fn dos_time(t: OffsetDateTime) -> DateTime {
    let offset = UtcOffset::local_offset_at(t).unwrap_or(UtcOffset::UTC);
    let local = t.to_offset(offset);
    u16::try_from(local.year())
        .ok()
        .and_then(|year| {
            DateTime::from_date_and_time(
                year,
                u8::from(local.month()),
                local.day(),
                local.hour(),
                local.minute(),
                local.second(),
            )
            .ok()
        })
        .unwrap_or(DateTime::DEFAULT)
}

/// Errors of `start_file`/`add_directory`: a duplicate name or a write error.
fn start_error(e: ZipError, path: &str) -> SinkError {
    match e {
        ZipError::Io(e) => SinkError::Target(e),
        ZipError::InvalidArchive(msg) if msg.contains("Duplicate") => {
            SinkError::Duplicate(path.to_owned())
        }
        other => SinkError::Target(io::Error::other(other)),
    }
}

/// Any other zip error: the archive cannot be written.
fn target(e: ZipError) -> SinkError {
    match e {
        ZipError::Io(e) => SinkError::Target(e),
        other => SinkError::Target(io::Error::other(other)),
    }
}
