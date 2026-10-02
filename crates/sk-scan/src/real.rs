//! `RealFs`: the scanner of the real file system (SPEC-03 §4.1, §4.2).
//!
//! Everything goes through `sk_core::path::to_extended` (FR-03-06) and only
//! reads (P1). Metadata comes from directory enumeration, so walking never
//! opens a file; reparse points other than cloud placeholder folders are
//! reported but not entered (FR-03-02). Reads (`read_head`, `read_small`)
//! check the entry itself first and never open a cloud-only file or follow a
//! link (SPEC-03 §4.1).

mod walk;

use std::collections::HashMap;
use std::ffi::OsString;
use std::io;
use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::{Arc, Mutex, PoisonError};

use rayon::{ThreadPool, ThreadPoolBuilder};
use sk_core::env::{DriveKind, DriveMedia, Environment};
use sk_core::path::to_extended;

use crate::win;
use crate::{
    CancellationToken, DirEntryInfo, EntryMeta, FsError, FsScanner, Readability, WalkControl,
    WalkOptions, WalkStats,
};

/// Threads for a root on an SSD: at most this many, and not more than CPUs (NFR-03-03).
const SSD_THREADS: usize = 8;
/// Threads for a root on an HDD or a network drive (NFR-03-03).
const SLOW_THREADS: usize = 2;

/// The scanner of the real file system (SPEC-03 §4.1).
///
/// Keeps the kinds of the drives from the [`Environment`] to choose the number
/// of walk threads, and one thread pool per thread count.
#[derive(Debug)]
pub struct RealFs {
    /// Drive letter (uppercase), kind and media.
    drives: Vec<(char, DriveKind, DriveMedia)>,
    /// Walk thread pools by their number of threads.
    pools: Mutex<HashMap<usize, Arc<ThreadPool>>>,
}

impl RealFs {
    /// A scanner for the drives of `env`.
    pub fn new(env: &Environment) -> Self {
        Self {
            drives: env
                .drives
                .iter()
                .map(|d| (d.letter.to_ascii_uppercase(), d.kind, d.media))
                .collect(),
            pools: Mutex::new(HashMap::new()),
        }
    }

    /// Number of walk threads for `root` (SPEC-03 §4.2).
    fn threads(&self, root: &Path, requested: usize) -> usize {
        let drive = drive_letter(root).and_then(|letter| {
            self.drives
                .iter()
                .find(|(l, _, _)| *l == letter)
                .map(|(_, kind, media)| (*kind, *media))
        });
        let cpus = std::thread::available_parallelism().map_or(1, usize::from);
        walk_threads(requested, drive, cpus)
    }

    /// The pool with `threads` threads, created on first use.
    fn pool(&self, threads: usize) -> Result<Arc<ThreadPool>, FsError> {
        let mut pools = self.pools.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(pool) = pools.get(&threads) {
            return Ok(Arc::clone(pool));
        }
        let pool = ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|i| format!("sk-scan-walk-{i}"))
            .build()
            .map_err(|e| FsError::Io(io::Error::other(e)))?;
        let pool = Arc::new(pool);
        pools.insert(threads, Arc::clone(&pool));
        Ok(pool)
    }
}

/// Number of walk threads (SPEC-03 §4.2, NFR-03-03): `requested`, but not
/// more than the limit of the root's drive — 2 for a network drive or an
/// HDD, otherwise `min(cpus, 8)`. `requested == 0` means the drive limit;
/// an unknown drive (`None`) counts as an SSD. Never less than 1.
pub(crate) fn walk_threads(
    requested: usize,
    drive: Option<(DriveKind, DriveMedia)>,
    cpus: usize,
) -> usize {
    let limit = match drive {
        Some((DriveKind::Network, _) | (_, DriveMedia::Hdd)) => SLOW_THREADS,
        _ => cpus.min(SSD_THREADS),
    };
    let threads = if requested == 0 {
        limit
    } else {
        requested.min(limit)
    };
    threads.max(1)
}

/// Uppercase drive letter of a `C:\...` or `\\?\C:\...` path.
fn drive_letter(path: &Path) -> Option<char> {
    match path.components().next()? {
        Component::Prefix(prefix) => match prefix.kind() {
            Prefix::Disk(d) | Prefix::VerbatimDisk(d) => Some(char::from(d).to_ascii_uppercase()),
            _ => None,
        },
        _ => None,
    }
}

impl FsScanner for RealFs {
    fn metadata(&self, path: &Path) -> Result<EntryMeta, FsError> {
        win::find_meta(path).map(|raw| raw.entry_meta())
    }

    fn exists(&self, path: &Path) -> bool {
        win::find_meta(path).is_ok()
    }

    fn read_dir(&self, path: &Path) -> Result<Vec<DirEntryInfo>, FsError> {
        let fs_path = to_extended(path);
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(&fs_path)? {
            // An entry that vanished or cannot be described is left out.
            let Ok(entry) = entry else { continue };
            let Ok(md) = entry.metadata() else { continue };
            let name: OsString = entry.file_name();
            let raw = win::listing_meta(&md, &fs_path.join(&name));
            entries.push(DirEntryInfo {
                path: path.join(&name),
                rel: PathBuf::from(name),
                depth: 1,
                meta: raw.entry_meta(),
            });
        }
        entries.sort_by(|a, b| a.rel.cmp(&b.rel));
        Ok(entries)
    }

    fn walk(
        &self,
        root: &Path,
        opts: &WalkOptions,
        visit: &mut dyn FnMut(&DirEntryInfo) -> WalkControl,
        cancel: &CancellationToken,
    ) -> Result<WalkStats, FsError> {
        walk::walk(
            || self.pool(self.threads(root, opts.threads)),
            root,
            opts,
            visit,
            cancel,
        )
    }

    fn read_head(&self, path: &Path, max: usize) -> Result<Vec<u8>, FsError> {
        win::read_file(path, max, false)
    }

    fn read_small(&self, path: &Path, max: usize) -> Result<Vec<u8>, FsError> {
        win::read_file(path, max, true)
    }

    fn probe_readable(&self, path: &Path) -> Readability {
        win::probe_readable(path)
    }
}

#[cfg(test)]
#[path = "real_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "real_read_tests.rs"]
mod read_tests;
