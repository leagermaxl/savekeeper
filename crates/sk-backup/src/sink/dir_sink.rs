//! `dir` container: a plain folder (SPEC-10 §4.3).
//!
//! Files are created new (never overwritten) under extended paths, so long
//! paths work (§5). After the content, the modification and (on Windows)
//! creation times are set through the open handle (`SetFileTime`), then the
//! attributes `READONLY|HIDDEN|SYSTEM|ARCHIVE` are set exactly
//! (`sk_core::fs::set_attributes`; none → `NORMAL`, which also clears the
//! `ARCHIVE` flag Windows sets on new files).

use std::fs::{self, File, FileTimes, OpenOptions};
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use sk_core::fs::set_attributes;
use sk_core::path::to_extended;
use sk_core::CancellationToken;

use super::{attribute_bits, copy, validate_path, ArchiveSink, SinkEntry, SinkError, COPY_BUFFER};

/// Writes a backup as a folder.
pub(crate) struct DirSink {
    /// The folder, extended path.
    root: PathBuf,
    /// Bytes of the files written.
    bytes: u64,
    buf: Vec<u8>,
}

impl DirSink {
    /// Creates the new folder `root` (never writes into an existing one); its
    /// parent must exist.
    pub fn create(root: &Path) -> Result<Self, SinkError> {
        let root = to_extended(root);
        fs::create_dir(&root).map_err(SinkError::Target)?;
        Ok(Self {
            root,
            bytes: 0,
            buf: vec![0; COPY_BUFFER],
        })
    }

    /// Path of an entry on disk.
    fn dest(&self, archive_path: &str) -> Result<PathBuf, SinkError> {
        validate_path(archive_path)?;
        let mut path = self.root.clone();
        path.extend(archive_path.split('/'));
        Ok(path)
    }
}

impl ArchiveSink for DirSink {
    fn add_file(
        &mut self,
        entry: &SinkEntry<'_>,
        content: &mut dyn Read,
        cancel: &CancellationToken,
    ) -> Result<u64, SinkError> {
        let dest = self.dest(entry.archive_path)?;
        if cancel.is_cancelled() {
            return Err(SinkError::Cancelled);
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(SinkError::Target)?;
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&dest)
            .map_err(|e| match e.kind() {
                ErrorKind::AlreadyExists => SinkError::Duplicate(entry.archive_path.to_owned()),
                _ => SinkError::Target(e),
            })?;
        let written = match copy(content, &mut file, &mut self.buf, cancel) {
            Ok(n) => n,
            Err(e) => {
                drop(file);
                // Our own incomplete output inside the backup folder.
                let _ = fs::remove_file(&dest);
                return Err(e);
            }
        };
        file.set_times(file_times(entry))
            .map_err(SinkError::Target)?;
        drop(file);
        set_attributes(&dest, attribute_bits(entry.attributes)).map_err(SinkError::Target)?;
        self.bytes += written;
        Ok(written)
    }

    fn add_dir(&mut self, entry: &SinkEntry<'_>) -> Result<(), SinkError> {
        let dest = self.dest(entry.archive_path)?;
        fs::create_dir_all(&dest).map_err(SinkError::Target)?;
        open_dir_for_times(&dest)
            .and_then(|dir| dir.set_times(file_times(entry)))
            .map_err(SinkError::Target)?;
        let mut bits = attribute_bits(entry.attributes);
        if cfg!(not(windows)) {
            // A read-only folder could not be written into outside Windows.
            bits &= !super::ATTR_READONLY;
        }
        set_attributes(&dest, bits).map_err(SinkError::Target)
    }

    fn finish(self: Box<Self>) -> Result<u64, SinkError> {
        Ok(self.bytes)
    }
}

/// Times to set: modification and, on Windows, creation.
fn file_times(entry: &SinkEntry<'_>) -> FileTimes {
    let mut times = FileTimes::new();
    if let Some(mtime) = entry.mtime {
        times = times.set_modified(SystemTime::from(mtime));
    }
    #[cfg(windows)]
    if let Some(ctime) = entry.ctime {
        use std::os::windows::fs::FileTimesExt;
        times = times.set_created(SystemTime::from(ctime));
    }
    times
}

/// Opens a folder to set its times: on Windows with `FILE_WRITE_ATTRIBUTES`
/// and `FILE_FLAG_BACKUP_SEMANTICS`, which a folder handle needs.
fn open_dir_for_times(path: &Path) -> std::io::Result<File> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_WRITE_ATTRIBUTES: u32 = 0x100;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        OpenOptions::new()
            .access_mode(FILE_WRITE_ATTRIBUTES)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)
    }
    #[cfg(not(windows))]
    {
        File::open(path)
    }
}
