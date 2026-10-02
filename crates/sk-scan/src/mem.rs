//! `MemFs`: an in-memory [`FsScanner`] for tests (SPEC-03 §4.1).
//!
//! Conventions:
//! - a `&str` path is split at both `/` and `\`; components compare without
//!   case, as on NTFS, and a leading `\\?\` is ignored;
//! - `mtime` uses the fixture syntax of SPEC-12 §4.3 (`-1d` or RFC 3339);
//!   anything else gives `EntryMeta::mtime = None`;
//! - parent folders are created implicitly;
//! - a file without `content` reads as `size` zero bytes;
//! - [`MemFs::calls`] counts `read_dir`, `exists` and `read_head` calls.
//!
//! YAML fixtures are loaded by `sk_testkit::mem_fixture` (SPEC-12 §4.2).

mod walk;

use std::collections::BTreeMap;
use std::io;
use std::ops::Bound;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};

use crate::{
    CancellationToken, CloudState, DirEntryInfo, EntryKind, EntryMeta, FsError, FsScanner,
    Readability, ReparseKind, WalkControl, WalkOptions, WalkStats,
};
use walk::Walker;

use crate::win::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS, FILE_ATTRIBUTE_REPARSE_POINT,
};

/// Lower-cased path components: the key of a node.
type Key = Vec<String>;

/// An in-memory file system for tests (SPEC-03 §4.1).
///
/// Attributes follow NTFS: folders have `FILE_ATTRIBUTE_DIRECTORY`, reparse
/// points `FILE_ATTRIBUTE_REPARSE_POINT` and cloud-only entries
/// `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS`. A builder call on an existing path
/// changes that entry; a file on the way to a new entry becomes a folder.
#[derive(Debug, Default)]
pub struct MemFs {
    nodes: BTreeMap<Key, MemNode>,
    calls: Counters,
}

/// Numbers of calls made to a [`MemFs`] since it was created.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemFsCalls {
    /// Directory listings: [`FsScanner::read_dir`] calls and folders listed by
    /// [`FsScanner::walk`].
    pub read_dir: u64,
    /// [`FsScanner::exists`] calls.
    pub exists: u64,
    /// [`FsScanner::read_head`] calls.
    pub read_head: u64,
}

#[derive(Debug, Default)]
struct Counters {
    read_dir: AtomicU64,
    exists: AtomicU64,
    read_head: AtomicU64,
}

#[derive(Debug, Clone)]
struct MemNode {
    /// Last path component as first given.
    name: String,
    kind: EntryKind,
    size: u64,
    mtime: Option<OffsetDateTime>,
    content: Option<Vec<u8>>,
    attrs: u32,
    cloud: CloudState,
    locked: bool,
}

impl MemNode {
    fn new(name: &str, kind: EntryKind, attrs: u32) -> Self {
        Self {
            name: name.to_owned(),
            kind,
            size: 0,
            mtime: None,
            content: None,
            attrs,
            cloud: CloudState::Local,
            locked: false,
        }
    }

    fn dir(name: &str) -> Self {
        Self::new(name, EntryKind::Dir, FILE_ATTRIBUTE_DIRECTORY)
    }

    fn is_dir(&self) -> bool {
        self.attrs & FILE_ATTRIBUTE_DIRECTORY != 0
    }

    /// Folders and cloud placeholder folders are entered by `walk` (SPEC-03 §4.2).
    fn is_walked(&self) -> bool {
        match self.kind {
            EntryKind::Dir => true,
            EntryKind::Reparse(ReparseKind::CloudPlaceholder) => self.is_dir(),
            EntryKind::File | EntryKind::Reparse(_) => false,
        }
    }

    fn meta(&self) -> EntryMeta {
        EntryMeta {
            kind: self.kind,
            size: self.size,
            mtime: self.mtime,
            ctime: self.mtime,
            attrs: self.attrs,
            cloud: self.cloud,
        }
    }
}

impl MemFs {
    /// An empty file system.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds or replaces a file. `size` is the logical size reported by
    /// metadata; reads return `content`, or `size` zero bytes without it.
    pub fn add_file(
        &mut self,
        path: &str,
        size: u64,
        mtime: &str,
        content: Option<&[u8]>,
    ) -> &mut Self {
        let mtime = parse_mtime(mtime, OffsetDateTime::now_utc());
        if let Some(node) = self.upsert(path, |name| MemNode::new(name, EntryKind::File, 0)) {
            *node = MemNode {
                size,
                mtime,
                content: content.map(<[u8]>::to_vec),
                ..MemNode::new(&node.name, EntryKind::File, 0)
            };
        }
        self
    }

    /// Adds a folder; an existing folder or reparse point is kept as is.
    pub fn add_dir(&mut self, path: &str) -> &mut Self {
        if let Some(node) = self.upsert(path, MemNode::dir) {
            if node.kind == EntryKind::File {
                *node = MemNode::dir(&node.name);
            }
        }
        self
    }

    /// Makes an entry a reparse point of `kind`, keeping its size and time;
    /// a new entry has size 0. A junction is always a folder, as on NTFS.
    pub fn add_reparse(&mut self, path: &str, kind: ReparseKind) -> &mut Self {
        let new = |name: &str| MemNode::new(name, EntryKind::Reparse(kind), 0);
        if let Some(node) = self.upsert(path, new) {
            node.kind = EntryKind::Reparse(kind);
            node.attrs |= FILE_ATTRIBUTE_REPARSE_POINT;
            if kind == ReparseKind::Junction {
                node.attrs |= FILE_ATTRIBUTE_DIRECTORY;
            }
        }
        self
    }

    /// Marks an entry as opened by another process without sharing; a missing
    /// entry is created as an empty file.
    pub fn set_locked(&mut self, path: &str) -> &mut Self {
        if let Some(node) = self.upsert(path, |name| MemNode::new(name, EntryKind::File, 0)) {
            node.locked = true;
        }
        self
    }

    /// Marks an entry as cloud-only; a missing entry is created as an empty file.
    pub fn set_cloud_only(&mut self, path: &str) -> &mut Self {
        if let Some(node) = self.upsert(path, |name| MemNode::new(name, EntryKind::File, 0)) {
            node.cloud = CloudState::CloudOnly;
            node.attrs |= FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS;
        }
        self
    }

    /// Numbers of calls since creation.
    pub fn calls(&self) -> MemFsCalls {
        MemFsCalls {
            read_dir: self.calls.read_dir.load(Ordering::Relaxed),
            exists: self.calls.exists.load(Ordering::Relaxed),
            read_head: self.calls.read_head.load(Ordering::Relaxed),
        }
    }

    /// The node at `path`, created by `new` with its parent folders if missing.
    /// `None` for a path without components.
    fn upsert(&mut self, path: &str, new: impl FnOnce(&str) -> MemNode) -> Option<&mut MemNode> {
        let names = components(path);
        let (last, parents) = names.split_last()?;
        let mut key = Key::with_capacity(names.len());
        for name in parents {
            key.push(name.to_lowercase());
            let parent = self
                .nodes
                .entry(key.clone())
                .or_insert_with(|| MemNode::dir(name));
            if parent.kind == EntryKind::File {
                *parent = MemNode::dir(&parent.name);
            }
        }
        key.push(last.to_lowercase());
        Some(self.nodes.entry(key).or_insert_with(|| new(last)))
    }

    fn node(&self, key: &[String]) -> Option<&MemNode> {
        if key.is_empty() {
            return None;
        }
        self.nodes.get(key)
    }

    /// Children of the folder `key`, sorted by key; counted as a listing.
    fn list(&self, key: &[String]) -> Result<Vec<(&Key, &MemNode)>, FsError> {
        self.calls.read_dir.fetch_add(1, Ordering::Relaxed);
        let node = self.node(key).ok_or(FsError::NotFound)?;
        if !node.is_dir() {
            return Err(io_error(io::ErrorKind::NotADirectory, "not a folder"));
        }
        Ok(self
            .nodes
            .range::<[String], _>((Bound::Excluded(key), Bound::Unbounded))
            .take_while(|(k, _)| k.starts_with(key))
            .filter(|(k, _)| k.len() == key.len() + 1)
            .collect())
    }

    /// The node of a file that may be read, after the checks of SPEC-03
    /// §4.1, in the order of `RealFs`: a path with `*` or `?` names nothing,
    /// a cloud-only entry is `CloudOnly`, a folder and a reparse point other
    /// than a cloud placeholder are not read, a locked file is a sharing
    /// violation.
    fn readable(&self, path: &Path) -> Result<&MemNode, FsError> {
        let key = path_key(path);
        if key.iter().any(|c| c.contains(['*', '?'])) {
            return Err(FsError::NotFound);
        }
        let node = self.node(&key).ok_or(FsError::NotFound)?;
        if node.cloud == CloudState::CloudOnly {
            return Err(FsError::CloudOnly);
        }
        if node.is_dir() {
            return Err(io_error(io::ErrorKind::IsADirectory, "is a folder"));
        }
        if matches!(node.kind, EntryKind::Reparse(k) if k != ReparseKind::CloudPlaceholder) {
            return Err(io_error(
                io::ErrorKind::InvalidInput,
                "reparse points are not read",
            ));
        }
        if node.locked {
            return Err(FsError::SharingViolation);
        }
        Ok(node)
    }
}

/// Up to `max` bytes of a file: its content, or zeros.
fn bytes(node: &MemNode, max: usize) -> Vec<u8> {
    match &node.content {
        Some(content) => content[..content.len().min(max)].to_vec(),
        None => vec![0; usize::try_from(node.size).unwrap_or(usize::MAX).min(max)],
    }
}

fn io_error(kind: io::ErrorKind, msg: &str) -> FsError {
    FsError::Io(io::Error::new(kind, msg))
}

impl FsScanner for MemFs {
    fn metadata(&self, path: &Path) -> Result<EntryMeta, FsError> {
        self.node(&path_key(path))
            .map(MemNode::meta)
            .ok_or(FsError::NotFound)
    }

    fn exists(&self, path: &Path) -> bool {
        self.calls.exists.fetch_add(1, Ordering::Relaxed);
        self.node(&path_key(path)).is_some()
    }

    fn read_dir(&self, path: &Path) -> Result<Vec<DirEntryInfo>, FsError> {
        Ok(self
            .list(&path_key(path))?
            .into_iter()
            .map(|(_, node)| DirEntryInfo {
                path: path.join(&node.name),
                rel: node.name.clone().into(),
                depth: 1,
                meta: node.meta(),
            })
            .collect())
    }

    fn walk(
        &self,
        root: &Path,
        opts: &WalkOptions,
        visit: &mut dyn FnMut(&DirEntryInfo) -> WalkControl,
        cancel: &CancellationToken,
    ) -> Result<WalkStats, FsError> {
        let key = path_key(root);
        let node = self.node(&key).ok_or(FsError::NotFound)?;
        if cancel.is_cancelled() {
            return Err(FsError::Cancelled);
        }
        if matches!(node.kind, EntryKind::Reparse(_)) && !node.is_walked() {
            // A reparse root is not entered (SPEC-03 §4.2 step 1).
            return Ok(WalkStats::default());
        }
        if !node.is_dir() {
            return Err(io_error(io::ErrorKind::NotADirectory, "not a folder"));
        }
        let mut walker = Walker {
            fs: self,
            opts,
            visit,
            cancel,
            stats: WalkStats::default(),
            stopped: false,
        };
        if opts.max_depth > 0 {
            walker.dir(root, &key, Path::new(""), "", 1)?;
        }
        Ok(walker.stats)
    }

    fn read_head(&self, path: &Path, max: usize) -> Result<Vec<u8>, FsError> {
        self.calls.read_head.fetch_add(1, Ordering::Relaxed);
        Ok(bytes(self.readable(path)?, max))
    }

    fn read_small(&self, path: &Path, max: usize) -> Result<Vec<u8>, FsError> {
        let node = self.readable(path)?;
        // Too large by metadata, or by what a read would return (SPEC-03 §4.1).
        let limit = u64::try_from(max).unwrap_or(u64::MAX);
        let read = node.content.as_ref().map_or(node.size, |c| c.len() as u64);
        if node.size > limit || read > limit {
            return Err(FsError::TooLarge);
        }
        Ok(bytes(node, max))
    }

    fn probe_readable(&self, path: &Path) -> Readability {
        match self.node(&path_key(path)) {
            None => Readability::Missing,
            Some(node) if node.cloud == CloudState::CloudOnly => Readability::CloudOnly,
            Some(node) if node.locked => Readability::Locked,
            Some(_) => Readability::Ok,
        }
    }
}

/// Non-empty components of a path split at `/` and `\`, without a leading
/// `\\?\` (or `\\?\UNC\`) and without `.`.
fn components(path: &str) -> Vec<&str> {
    let path = path
        .strip_prefix(r"\\?\UNC\")
        .or_else(|| path.strip_prefix(r"\\?\"))
        .unwrap_or(path);
    path.split(['/', '\\'])
        .filter(|c| !c.is_empty() && *c != ".")
        .collect()
}

fn path_key(path: &Path) -> Key {
    components(&path.to_string_lossy())
        .into_iter()
        .map(str::to_lowercase)
        .collect()
}

/// Fixture time syntax (SPEC-12 §4.3): `-1d`, `+2h`, `-30m`, `-10s`, `-1w`
/// relative to `now`, or RFC 3339. `None` if it does not parse.
fn parse_mtime(s: &str, now: OffsetDateTime) -> Option<OffsetDateTime> {
    let s = s.trim();
    let negative = match s.chars().next()? {
        '-' => true,
        '+' => false,
        _ => return OffsetDateTime::parse(s, &Rfc3339).ok(),
    };
    let rest = &s[1..];
    let split = rest.find(|c: char| !c.is_ascii_digit())?;
    let (number, unit) = rest.split_at(split);
    let number: i32 = number.parse().ok()?;
    let unit = match unit {
        "s" => Duration::SECOND,
        "m" => Duration::MINUTE,
        "h" => Duration::HOUR,
        "d" => Duration::DAY,
        "w" => Duration::WEEK,
        _ => return None,
    };
    let offset = unit.checked_mul(number)?;
    if negative {
        now.checked_sub(offset)
    } else {
        now.checked_add(offset)
    }
}

#[cfg(test)]
#[path = "mem_tests.rs"]
mod tests;
