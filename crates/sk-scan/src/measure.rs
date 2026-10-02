//! `measure`: sizes and counts of findings (SPEC-03 §4.3).
//!
//! A `FileSet` is walked once with its `include`/`exclude` globs and the global
//! exclusions of §4.5; the result goes to a [`DirStatsCache`], together with the
//! stats of the subfolders down to depth 3 when the target has no globs, so
//! that nested roots are not walked again (FR-03-08). Files are never opened,
//! except for lock probes of often locked file types; cloud-only files are
//! never probed (FR-03-03).

mod all;
mod cache;

use std::collections::HashMap;
use std::ffi::OsStr;
use std::io;
use std::path::Path;
use std::sync::Arc;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use sk_core::model::{Target, TargetStats};
use time::OffsetDateTime;

use crate::win::FILE_ATTRIBUTE_DIRECTORY;
use crate::{
    CancellationToken, CloudState, EntryKind, EntryMeta, ExcludeSet, Exclusion, FsError, FsScanner,
    PathFilter, Readability, ReparseKind, WalkControl, WalkOptions,
};
pub use all::measure_all;
pub use cache::DirStatsCache;
use cache::{CacheKey, Cached};

/// Default limit of entries per root.
const MAX_ENTRIES: u64 = 2_000_000;
/// Lock probes per root at most (probes are expensive, §4.3 step 4).
const MAX_PROBES: u32 = 500;
/// Extensions of files that are often held open by their programs (lowercase).
const LOCKABLE_EXTS: &[&str] = &[
    "db", "sqlite", "ldb", "log", "dat", "lock", "pst", "ost", "vhdx",
];
/// Subfolders down to this depth are cached after a walk (§4.3 `DirStatsCache`).
const CACHE_DEPTH: usize = 3;

/// Parameters of [`measure`] and [`measure_all`] (SPEC-03 §4.1).
#[derive(Debug, Clone)]
pub struct MeasureOptions {
    /// Probe often locked files for sharing violations (true by default).
    pub probe_locks: bool,
    /// Switch the global exclusions off (CLI debugging; false by default).
    pub include_excluded: bool,
    /// Global exclusions: `ExcludeSet::with_user(env, &config.scan.exclude_globs)`.
    pub excludes: Arc<ExcludeSet>,
    /// Walk depth limit (`config.scan.max_depth`).
    pub max_depth: u32,
    /// Entries per root at most (2 000 000 by default); then `truncated`.
    pub max_entries: u64,
    /// Walk threads; 0 means the limit of the root's drive (§4.2).
    pub threads: usize,
}

impl MeasureOptions {
    /// Options with `excludes` and `max_depth`; the other fields get their defaults.
    pub fn new(excludes: Arc<ExcludeSet>, max_depth: u32) -> Self {
        Self {
            probe_locks: true,
            include_excluded: false,
            excludes,
            max_depth,
            max_entries: MAX_ENTRIES,
            threads: 0,
        }
    }
}

/// Which global exclusions a walk uses (§4.5); part of the cache key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Mode {
    /// The whole set.
    Full,
    /// `names_only`: the root lies in a path exclusion (`explicit_root`).
    NamesOnly,
    /// `include_excluded`: no exclusions.
    Off,
}

impl Mode {
    /// The mode for `root` (§4.5): `explicit_root` when the full set excludes
    /// the root and its names-only part keeps it.
    pub(crate) fn for_root(opts: &MeasureOptions, root: &Path) -> Self {
        if opts.include_excluded {
            return Mode::Off;
        }
        let name = root.file_name().unwrap_or(root.as_os_str());
        if opts.excludes.check(root, name, true) == Exclusion::Exclude
            && opts.excludes.names_only().check(root, name, true) == Exclusion::Keep
        {
            Mode::NamesOnly
        } else {
            Mode::Full
        }
    }

    fn filter(self, excludes: &Arc<ExcludeSet>) -> Arc<dyn PathFilter> {
        match self {
            Mode::Full => Arc::clone(excludes) as Arc<dyn PathFilter>,
            Mode::NamesOnly => Arc::new(excludes.names_only()),
            Mode::Off => Arc::new(KeepAll),
        }
    }
}

/// The filter of `include_excluded`: keeps everything.
#[derive(Debug)]
struct KeepAll;

impl PathFilter for KeepAll {
    fn check(&self, _: &Path, _: &OsStr, _: bool) -> Exclusion {
        Exclusion::Keep
    }
}

/// How an entry counts (§4.3 step 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Counted {
    File,
    Dir,
    /// Symlink, junction or another reparse point.
    Not,
}

fn counted(meta: &EntryMeta) -> Counted {
    match meta.kind {
        EntryKind::File | EntryKind::Reparse(ReparseKind::AppExecLink) => Counted::File,
        EntryKind::Dir => Counted::Dir,
        EntryKind::Reparse(ReparseKind::CloudPlaceholder) => {
            if meta.attrs & FILE_ATTRIBUTE_DIRECTORY != 0 {
                Counted::Dir
            } else {
                Counted::File
            }
        }
        EntryKind::Reparse(_) => Counted::Not,
    }
}

/// Size counted for a file: an app alias counts as 0 bytes (§5).
fn file_size(meta: &EntryMeta) -> u64 {
    match meta.kind {
        EntryKind::Reparse(ReparseKind::AppExecLink) => 0,
        _ => meta.size,
    }
}

/// Running totals of a subtree.
#[derive(Debug, Clone, Default)]
struct Acc {
    total_bytes: u64,
    file_count: u64,
    dir_count: u64,
    newest: Option<OffsetDateTime>,
    oldest: Option<OffsetDateTime>,
    locked: u32,
    cloud_bytes: u64,
    cloud_files: u64,
    largest: Option<u64>,
}

/// One counted file.
#[derive(Debug, Clone, Copy)]
struct FileFacts {
    size: u64,
    mtime: Option<OffsetDateTime>,
    cloud: bool,
    locked: bool,
}

impl Acc {
    fn add_file(&mut self, f: FileFacts) {
        self.total_bytes = self.total_bytes.saturating_add(f.size);
        self.file_count += 1;
        if f.cloud {
            self.cloud_bytes = self.cloud_bytes.saturating_add(f.size);
            self.cloud_files += 1;
        }
        if f.locked {
            self.locked = self.locked.saturating_add(1);
        }
        if let Some(t) = f.mtime {
            self.newest = Some(self.newest.map_or(t, |n| n.max(t)));
            self.oldest = Some(self.oldest.map_or(t, |o| o.min(t)));
        }
        self.largest = Some(self.largest.map_or(f.size, |l| l.max(f.size)));
    }

    fn stats(&self, truncated: bool) -> TargetStats {
        TargetStats {
            total_bytes: self.total_bytes,
            file_count: self.file_count,
            dir_count: self.dir_count,
            newest_mtime: self.newest,
            oldest_mtime: self.oldest,
            locked_files: self.locked,
            cloud_only_bytes: self.cloud_bytes,
            cloud_only_files: self.cloud_files,
            largest_file_bytes: self.largest,
            truncated,
        }
    }
}

/// Stats of a target (SPEC-03 §4.3). `Ok(None)` for `Registry` and
/// `SystemExport` (their size is estimated by SPEC-06 `plan()`).
///
/// A missing root is [`FsError::NotFound`]; a `File` target that is a folder
/// is [`FsError::Io`]; cancellation is [`FsError::Cancelled`] and leaves
/// `cache` unchanged.
pub fn measure(
    fs: &dyn FsScanner,
    target: &Target,
    cache: &DirStatsCache,
    opts: &MeasureOptions,
    cancel: &CancellationToken,
) -> Result<Option<TargetStats>, FsError> {
    measure_target(fs, target, cache, opts, cancel).map(|m| m.map(|c| c.stats))
}

/// [`measure`] with the number of walk errors, for `measure_all` issues.
fn measure_target(
    fs: &dyn FsScanner,
    target: &Target,
    cache: &DirStatsCache,
    opts: &MeasureOptions,
    cancel: &CancellationToken,
) -> Result<Option<Cached>, FsError> {
    match target {
        Target::File { resolved, .. } => measure_file(fs, resolved, opts).map(Some),
        Target::FileSet {
            resolved,
            include,
            exclude,
            ..
        } => measure_set(fs, resolved, include, exclude, cache, opts, cancel).map(Some),
        Target::Registry { .. } | Target::SystemExport { .. } => Ok(None),
    }
}

fn measure_file(fs: &dyn FsScanner, path: &Path, opts: &MeasureOptions) -> Result<Cached, FsError> {
    let meta = fs.metadata(path)?;
    let mut acc = Acc::default();
    match counted(&meta) {
        Counted::Dir => {
            return Err(FsError::Io(io::Error::new(
                io::ErrorKind::IsADirectory,
                "the file target is a folder",
            )));
        }
        Counted::File => {
            let cloud = meta.cloud == CloudState::CloudOnly;
            let locked =
                opts.probe_locks && !cloud && fs.probe_readable(path) == Readability::Locked;
            acc.add_file(FileFacts {
                size: file_size(&meta),
                mtime: meta.mtime,
                cloud,
                locked,
            });
        }
        Counted::Not => {}
    }
    Ok(Cached {
        stats: acc.stats(false),
        errors: 0,
    })
}

/// A glob set relative to the root, or `None` for no globs.
fn glob_set(globs: &[String]) -> Result<Option<GlobSet>, FsError> {
    if globs.is_empty() {
        return Ok(None);
    }
    let invalid = |e: globset::Error| FsError::Io(io::Error::new(io::ErrorKind::InvalidInput, e));
    let mut builder = GlobSetBuilder::new();
    for g in globs {
        let glob = GlobBuilder::new(g)
            .case_insensitive(true)
            .literal_separator(true)
            .build()
            .map_err(invalid)?;
        builder.add(glob);
    }
    builder.build().map(Some).map_err(invalid)
}

/// Whether a file of this name is probed for locks (§4.3 step 4).
fn lockable(name: &OsStr) -> bool {
    let ext = Path::new(name)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase());
    ext.is_some_and(|e| LOCKABLE_EXTS.contains(&e.as_str()))
}

fn measure_set(
    fs: &dyn FsScanner,
    root: &Path,
    include: &[String],
    exclude: &[String],
    cache: &DirStatsCache,
    opts: &MeasureOptions,
    cancel: &CancellationToken,
) -> Result<Cached, FsError> {
    let mode = Mode::for_root(opts, root);
    let key = CacheKey::new(root, include, exclude, mode);
    if let Some(hit) = cache.get(&key) {
        return Ok(hit);
    }
    let walk_opts = WalkOptions {
        max_depth: opts.max_depth,
        max_entries: opts.max_entries,
        follow_links: false,
        excludes: mode.filter(&opts.excludes),
        include: glob_set(include)?,
        exclude: glob_set(exclude)?,
        threads: opts.threads,
    };
    let with_subs = include.is_empty() && exclude.is_empty();
    let mut acc = Acc::default();
    // Subfolders down to `CACHE_DEPTH`, by their lowercase relative components.
    let mut subs: HashMap<Vec<String>, Acc> = HashMap::new();
    let mut probes = 0u32;
    let mut depth_cut = false;
    let mut visit = |e: &crate::DirEntryInfo| {
        let kind = counted(&e.meta);
        let depth = usize::try_from(e.depth).unwrap_or(usize::MAX);
        let rel: Vec<String> = if with_subs {
            e.rel
                .iter()
                .take(CACHE_DEPTH)
                .map(|c| c.to_string_lossy().to_lowercase())
                .collect()
        } else {
            Vec::new()
        };
        // Ancestors of the entry that are cached subfolders.
        let ancestors = depth.saturating_sub(1).min(CACHE_DEPTH).min(rel.len());
        match kind {
            Counted::File => {
                let cloud = e.meta.cloud == CloudState::CloudOnly;
                let probe = opts.probe_locks
                    && !cloud
                    && probes < MAX_PROBES
                    && e.path.file_name().is_some_and(lockable);
                if probe {
                    probes += 1;
                }
                let facts = FileFacts {
                    size: file_size(&e.meta),
                    mtime: e.meta.mtime,
                    cloud,
                    locked: probe && fs.probe_readable(&e.path) == Readability::Locked,
                };
                acc.add_file(facts);
                for k in 1..=ancestors {
                    subs.entry(rel[..k].to_vec()).or_default().add_file(facts);
                }
            }
            Counted::Dir => {
                acc.dir_count += 1;
                for k in 1..=ancestors {
                    subs.entry(rel[..k].to_vec()).or_default().dir_count += 1;
                }
                if with_subs && depth <= CACHE_DEPTH {
                    subs.entry(rel.clone()).or_default();
                }
                if e.depth >= opts.max_depth {
                    depth_cut = true;
                }
            }
            Counted::Not => {}
        }
        WalkControl::Continue
    };
    let walk = fs.walk(root, &walk_opts, &mut visit, cancel)?;
    let result = Cached {
        stats: acc.stats(walk.truncated || walk.errors > 0),
        errors: walk.errors,
    };
    let complete = !walk.truncated && walk.errors == 0 && !depth_cut && opts.max_depth > 0;
    if with_subs && complete {
        cache.put_subfolders(&key, subs.into_iter().map(|(rel, a)| (rel, a.stats(false))));
    }
    cache.put(key, result.clone());
    Ok(result)
}

#[cfg(test)]
#[path = "measure_tests.rs"]
mod tests;
