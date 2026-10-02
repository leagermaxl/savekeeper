//! Parallel traversal of [`RealFs::walk`](super::RealFs) (SPEC-03 §4.2).
//!
//! Folders are listed by tasks on a rayon pool (`std::fs::read_dir`, metadata
//! from the listing). Each task applies the exclusions to its listing and
//! sends what is left over a channel; the calling thread visits the entries
//! and decides which folders to enter, so `SkipDir`, `Stop`, `max_depth` and
//! `max_entries` behave exactly as in `MemFs` (SPEC-03 §4.1 «Поведение
//! walk»). Entries of one folder are visited in name order; the order of
//! folders is not defined.

use std::ffi::OsString;
use std::io;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::Duration;

use globset::GlobSet;
use rayon::ThreadPool;
use sk_core::path::to_extended;

use crate::exclude::name_matches;
use crate::win::{self, RawMeta};
use crate::{
    CancellationToken, DirEntryInfo, EntryKind, Exclusion, FsError, PathFilter, WalkControl,
    WalkOptions, WalkStats,
};

/// How often the calling thread checks cancellation while it waits for
/// listings (FR-03-07: at least every 100 ms).
const POLL: Duration = Duration::from_millis(50);

/// A folder to list.
#[derive(Debug)]
struct DirTask {
    /// Path as the caller gave it (joined with names), for `DirEntryInfo::path`.
    abs: PathBuf,
    /// The same path with the `\\?\` prefix, for file system calls (FR-03-06).
    fs: PathBuf,
    /// Path relative to the walk root.
    rel: PathBuf,
    /// `rel` as a `/`-separated string, for the `include`/`exclude` globs.
    rel_glob: String,
    /// Depth of the folder's children.
    depth: u32,
}

/// An entry that passed the exclusions.
#[derive(Debug)]
struct Child {
    name: OsString,
    raw: RawMeta,
    rel_glob: String,
}

/// What a listing task found.
#[derive(Debug, Default)]
struct Children {
    entries: Vec<Child>,
    skipped: u64,
    errors: u64,
}

/// State shared with the listing tasks.
#[derive(Debug)]
struct Shared {
    excludes: Arc<dyn PathFilter>,
    include: Option<GlobSet>,
    exclude: Option<GlobSet>,
    cancel: CancellationToken,
    /// Set when the walk ends early: queued tasks then list nothing.
    stop: AtomicBool,
    tx: Sender<(DirTask, Children)>,
}

impl Shared {
    fn halted(&self) -> bool {
        self.stop.load(Ordering::Relaxed) || self.cancel.is_cancelled()
    }
}

/// Sets the stop flag when the walk returns, however it returns.
struct StopOnDrop(Arc<Shared>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.stop.store(true, Ordering::Relaxed);
    }
}

/// Walks `root` (SPEC-03 §4.2). `pool` is called once, after the root checks.
pub(super) fn walk(
    pool: impl FnOnce() -> Result<Arc<ThreadPool>, FsError>,
    root: &Path,
    opts: &WalkOptions,
    visit: &mut dyn FnMut(&DirEntryInfo) -> WalkControl,
    cancel: &CancellationToken,
) -> Result<WalkStats, FsError> {
    let fs_root = to_extended(root);
    let raw = win::find_meta(&fs_root)?;
    if cancel.is_cancelled() {
        return Err(FsError::Cancelled);
    }
    if !raw.is_walked() {
        if matches!(raw.kind(), EntryKind::Reparse(_)) {
            // A reparse root is not entered (step 1); the caller reports it.
            return Ok(WalkStats::default());
        }
        return Err(FsError::Io(io::Error::new(
            io::ErrorKind::NotADirectory,
            "not a folder",
        )));
    }
    let mut stats = WalkStats::default();
    if opts.max_depth == 0 {
        return Ok(stats);
    }

    let pool = pool()?;
    let (tx, rx) = mpsc::channel();
    let shared = Arc::new(Shared {
        excludes: Arc::clone(&opts.excludes),
        include: opts.include.clone(),
        exclude: opts.exclude.clone(),
        cancel: cancel.clone(),
        stop: AtomicBool::new(false),
        tx,
    });
    let _stop = StopOnDrop(Arc::clone(&shared));
    let spawn =
        |task: DirTask| {
            let shared = Arc::clone(&shared);
            pool.spawn(move || {
                // A panicking filter must not lose the message the walk waits for.
                let children = catch_unwind(AssertUnwindSafe(|| list(&shared, &task)))
                    .unwrap_or_else(|_| Children {
                        errors: 1,
                        ..Children::default()
                    });
                // The walk may have ended already; then nobody listens.
                let _ = shared.tx.send((task, children));
            });
        };

    spawn(DirTask {
        abs: root.to_path_buf(),
        fs: fs_root,
        rel: PathBuf::new(),
        rel_glob: String::new(),
        depth: 1,
    });
    let mut pending: usize = 1;
    while pending > 0 {
        if cancel.is_cancelled() {
            return Err(FsError::Cancelled);
        }
        let (task, children) = match rx.recv_timeout(POLL) {
            Ok(message) => message,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        pending -= 1;
        stats.skipped_excluded += children.skipped;
        stats.errors += children.errors;
        for child in children.entries {
            if cancel.is_cancelled() {
                return Err(FsError::Cancelled);
            }
            if stats.entries >= opts.max_entries {
                stats.truncated = true;
                return Ok(stats);
            }
            stats.entries += 1;
            let info = DirEntryInfo {
                path: task.abs.join(&child.name),
                rel: task.rel.join(&child.name),
                depth: task.depth,
                meta: child.raw.entry_meta(),
            };
            match visit(&info) {
                WalkControl::Stop => return Ok(stats),
                WalkControl::SkipDir => continue,
                WalkControl::Continue => {}
            }
            if child.raw.is_walked() && task.depth < opts.max_depth {
                spawn(DirTask {
                    fs: task.fs.join(&child.name),
                    abs: info.path,
                    rel: info.rel,
                    rel_glob: child.rel_glob,
                    depth: task.depth + 1,
                });
                pending += 1;
            }
        }
    }
    // A listing cut short by cancellation looks empty: report the cancellation.
    if cancel.is_cancelled() {
        return Err(FsError::Cancelled);
    }
    Ok(stats)
}

/// Lists the folder of `task` and applies the exclusions (step 3). Nothing is
/// listed once the walk is stopped or cancelled.
fn list(shared: &Shared, task: &DirTask) -> Children {
    let mut out = Children::default();
    if shared.halted() {
        return out;
    }
    let read = match std::fs::read_dir(&task.fs) {
        Ok(read) => read,
        Err(_) => {
            // No right to list, or vanished: the descent is skipped (SPEC-03 §5).
            out.errors = 1;
            return out;
        }
    };
    let mut listing: Vec<(OsString, RawMeta)> = Vec::new();
    for entry in read {
        if shared.halted() {
            return Children::default();
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                out.errors += 1;
                continue;
            }
        };
        let name = entry.file_name();
        match entry.metadata() {
            Ok(md) => {
                let raw = win::listing_meta(&md, &task.fs.join(&name));
                listing.push((name, raw));
            }
            Err(_) => out.errors += 1,
        }
    }
    listing.sort_by(|a, b| a.0.cmp(&b.0));

    for (i, (name, raw)) in listing.iter().enumerate() {
        let rel_glob = if task.rel_glob.is_empty() {
            name.to_string_lossy().into_owned()
        } else {
            format!("{}/{}", task.rel_glob, name.to_string_lossy())
        };
        let excluded = is_excluded(shared, task, i, &listing)
            || shared
                .exclude
                .as_ref()
                .is_some_and(|g| g.is_match(&rel_glob));
        if excluded {
            out.skipped += 1;
            continue;
        }
        // `include` selects files; folders are always entered.
        let included = raw.is_dir
            || shared
                .include
                .as_ref()
                .is_none_or(|g| g.is_match(&rel_glob));
        if included {
            out.entries.push(Child {
                name: name.clone(),
                raw: *raw,
                rel_glob,
            });
        }
    }
    out
}

/// Asks the global filter about entry `i` of `listing` and resolves its
/// conditions (SPEC-03 §4.2 step 3): siblings from the same listing, a child
/// file by one metadata lookup.
fn is_excluded(shared: &Shared, task: &DirTask, i: usize, listing: &[(OsString, RawMeta)]) -> bool {
    let (name, raw) = &listing[i];
    match shared
        .excludes
        .check(&task.abs.join(name), name, raw.is_dir)
    {
        Exclusion::Keep => false,
        Exclusion::Exclude => true,
        Exclusion::ExcludeIfSibling(glob) => listing
            .iter()
            .enumerate()
            .any(|(j, (n, _))| j != i && name_matches(glob, &n.to_string_lossy())),
        Exclusion::ExcludeIfChild(file) => {
            raw.is_dir && win::find_meta(&task.fs.join(name).join(file)).is_ok_and(|m| !m.is_dir)
        }
    }
}
