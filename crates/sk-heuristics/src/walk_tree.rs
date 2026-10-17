//! Per-folder aggregates of the shared walk of the roots (SPEC-07 §4.3.3,
//! steps 1–2) and the folder tree that holds them.
//!
//! [`TreeBuilder`] collects the folders and files of one root in the order
//! the walk gives them (no order between folders, SPEC-03 §4.4); `finish`
//! puts the folders in a fixed pre-order (children by name) and rolls the
//! aggregates up the tree. Byte sums are integers and the weights are applied
//! once, in a fixed order, so the result does not depend on the walk order
//! (NFR-07-04).

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use sk_core::fs::{CloudState, DirEntryInfo, EntryKind, EntryMeta, ReparseKind, WalkControl};
use sk_core::path::PathSet;
use time::OffsetDateTime;

use crate::data::{ContentKind, ExtensionTable};
use crate::unk_zones::is_dir;

/// A file counts in `user_weight` with at most this many bytes: 50 MiB.
const WEIGHT_CAP_BYTES: u64 = 50 * 1024 * 1024;
/// `user_weight` is measured in MiB.
const MIB: f64 = 1024.0 * 1024.0;

/// Git marks of a folder ([`DirNode::git`]).
const GIT_DIR: u8 = 1;
const GIT_FILE: u8 = 1 << 1;
const BARE_HEAD: u8 = 1 << 2;
const BARE_OBJECTS: u8 = 1 << 3;
const BARE_REFS: u8 = 1 << 4;
const BARE: u8 = BARE_HEAD | BARE_OBJECTS | BARE_REFS;

/// Files of one content kind (SPEC-07 §4.3.2) in a folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KindStat {
    /// The kind.
    pub kind: ContentKind,
    /// Number of files.
    pub files: u64,
    /// Their size.
    pub bytes: u64,
    /// Their size with every file capped at 50 MiB, as in `user_weight`.
    pub capped_bytes: u64,
}

/// Aggregates of a folder (SPEC-07 §4.3.3, step 1): of its own files
/// ([`DirNode::own`]) or of its whole subtree ([`DirNode::subtree`]).
///
/// Files are counted as in SPEC-03 §4.3 step 3: regular files, cloud
/// placeholder files and app execution aliases (size 0); links are not.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct DirAggregate {
    /// `Σ weight(ext) × min(size, 50 MiB) / 1 MiB`.
    pub user_weight: f64,
    /// Size of the files.
    pub bytes: u64,
    /// Number of files.
    pub files: u64,
    /// Size of the files of the `binary` group.
    pub binary_bytes: u64,
    /// Newest modification time of the files.
    pub newest_mtime: Option<OffsetDateTime>,
    /// Files that are only in the cloud (not downloaded).
    pub cloud_only_files: u64,
    /// Files per content kind; only listed kinds with files, in the order of
    /// [`ContentKind::ALL`].
    pub kinds: Vec<KindStat>,
}

impl DirAggregate {
    /// Counts a file of `size` bytes with the content kind `kind`.
    fn add_file(&mut self, size: u64, meta: &EntryMeta, kind: Option<ContentKind>) {
        self.bytes = self.bytes.saturating_add(size);
        self.files += 1;
        if meta.cloud == CloudState::CloudOnly {
            self.cloud_only_files += 1;
        }
        self.newest_mtime = self.newest_mtime.max(meta.mtime);
        if let Some(kind) = kind {
            let stat = KindStat {
                kind,
                files: 1,
                bytes: size,
                capped_bytes: size.min(WEIGHT_CAP_BYTES),
            };
            add_kinds(&mut self.kinds, std::slice::from_ref(&stat));
        }
    }

    /// Adds the aggregates of a subfolder; the weights are set by `finish`.
    pub(crate) fn add(&mut self, other: &DirAggregate) {
        self.bytes = self.bytes.saturating_add(other.bytes);
        self.files += other.files;
        self.cloud_only_files += other.cloud_only_files;
        self.newest_mtime = self.newest_mtime.max(other.newest_mtime);
        add_kinds(&mut self.kinds, &other.kinds);
    }

    /// Sets `user_weight` and `binary_bytes` from the kinds.
    pub(crate) fn finish(&mut self, table: Option<&ExtensionTable>) {
        let weight = |kind: ContentKind| {
            table
                .and_then(|t| t.groups().iter().find(|g| g.kind == kind))
                .map_or(0.0, |g| f64::from(g.weight))
        };
        self.user_weight = self
            .kinds
            .iter()
            .map(|k| weight(k.kind) * k.capped_bytes as f64 / MIB)
            .sum();
        self.binary_bytes = self
            .kinds
            .iter()
            .find(|k| k.kind == ContentKind::Binary)
            .map_or(0, |k| k.bytes);
    }
}

/// Adds `stats` into the sorted `kinds`.
fn add_kinds(kinds: &mut Vec<KindStat>, stats: &[KindStat]) {
    for stat in stats {
        match kinds.binary_search_by(|k| k.kind.cmp(&stat.kind)) {
            Ok(i) => {
                let k = &mut kinds[i];
                k.files += stat.files;
                k.bytes = k.bytes.saturating_add(stat.bytes);
                k.capped_bytes = k.capped_bytes.saturating_add(stat.capped_bytes);
            }
            Err(i) => kinds.insert(i, *stat),
        }
    }
}

/// A walked folder.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DirNode {
    /// Absolute path.
    pub path: PathBuf,
    /// Depth below its root (the root is 0).
    pub depth: u32,
    /// Index of the parent folder; `None` for a root.
    pub parent: Option<usize>,
    /// End (exclusive) of the range of its descendants in
    /// [`RootsWalk::dirs`], which starts right after the folder.
    pub end: usize,
    /// Its own files.
    pub own: DirAggregate,
    /// Its files and those of all its subfolders (step 2).
    pub subtree: DirAggregate,
    /// Git marks.
    git: u8,
}

impl DirNode {
    /// The folder holds `.git` (a folder or a gitlink file): the working copy
    /// of a repository (SPEC-07 §4.4.1). The `.git` folder is not walked.
    pub(crate) fn is_git_worktree(&self) -> bool {
        self.git & (GIT_DIR | GIT_FILE) != 0
    }

    /// The folder holds the file `HEAD` and the folders `objects` and `refs`:
    /// a bare repository (SPEC-07 §4.4.1).
    pub(crate) fn is_bare_repo(&self) -> bool {
        self.git & BARE == BARE
    }
}

/// A file of the `credentials` or `ebook_mail` group (SPEC-07 §4.3.3, rule 5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValuableFile {
    /// Absolute path.
    pub path: PathBuf,
    /// Size.
    pub size: u64,
    /// `Credentials` or `EbookMail`.
    pub kind: ContentKind,
}

/// A walked root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WalkedRoot {
    /// Absolute path.
    pub path: PathBuf,
    /// Index of its folder in [`RootsWalk::dirs`].
    pub node: usize,
    /// Folders that could not be read.
    pub errors: u64,
}

/// Result of the shared walk of the roots (SPEC-07 §4.3.1).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct RootsWalk {
    /// The walked roots, sorted by path.
    pub roots: Vec<WalkedRoot>,
    /// Folders of all roots: each root followed by its descendants in
    /// pre-order, children sorted by name (lowercase, then as is).
    pub dirs: Vec<DirNode>,
    /// Files of the `credentials` and `ebook_mail` groups, sorted by path.
    pub valuable_files: Vec<ValuableFile>,
    /// The scan was cancelled: the walk is incomplete.
    pub cancelled: bool,
}

impl RootsWalk {
    /// Indexes of the direct subfolders of the folder `i`, by name.
    pub(crate) fn children(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        let end = self.dirs.get(i).map_or(i, |d| d.end);
        let mut next = i + 1;
        std::iter::from_fn(move || {
            (next < end).then(|| {
                let child = next;
                next = self.dirs[child].end;
                child
            })
        })
    }
}

/// A folder while its root is walked.
struct RawNode {
    path: PathBuf,
    depth: u32,
    parent: Option<usize>,
    children: Vec<usize>,
    own: DirAggregate,
    git: u8,
}

/// Collects the folders and files of one root.
pub(crate) struct TreeBuilder<'a> {
    nodes: Vec<RawNode>,
    /// Folder path relative to the root → index in `nodes`.
    index: HashMap<PathBuf, usize>,
    /// Folders skipped with their contents (SPEC-07 §4.3.1).
    skip: &'a PathSet,
    table: Option<&'a ExtensionTable>,
    valuable: Vec<ValuableFile>,
}

impl<'a> TreeBuilder<'a> {
    /// A builder for `root`; folders covered by `skip` are not entered.
    pub(crate) fn new(root: &Path, skip: &'a PathSet, table: Option<&'a ExtensionTable>) -> Self {
        let node = RawNode {
            path: root.to_path_buf(),
            depth: 0,
            parent: None,
            children: Vec::new(),
            own: DirAggregate::default(),
            git: 0,
        };
        Self {
            nodes: vec![node],
            index: HashMap::from([(PathBuf::new(), 0)]),
            skip,
            table,
            valuable: Vec::new(),
        }
    }

    /// Takes one entry of the walk.
    pub(crate) fn visit(&mut self, e: &DirEntryInfo) -> WalkControl {
        let Some(&parent) = e.rel.parent().and_then(|p| self.index.get(p)) else {
            // Never happens: the walk gives a folder before its contents.
            return WalkControl::SkipDir;
        };
        let name = e.path.file_name().map(|n| n.to_string_lossy());
        let is_named = |expected: &str| {
            name.as_ref()
                .is_some_and(|n| n.eq_ignore_ascii_case(expected))
        };
        if is_dir(&e.meta) {
            if is_named(".git") {
                // Repository data, not user files; H-GIT reads it through gix.
                self.nodes[parent].git |= GIT_DIR;
                return WalkControl::SkipDir;
            }
            if self.skip.covers(&e.path) {
                return WalkControl::SkipDir;
            }
            if is_named("objects") {
                self.nodes[parent].git |= BARE_OBJECTS;
            } else if is_named("refs") {
                self.nodes[parent].git |= BARE_REFS;
            }
            let i = self.nodes.len();
            self.nodes.push(RawNode {
                path: e.path.clone(),
                depth: e.depth,
                parent: Some(parent),
                children: Vec::new(),
                own: DirAggregate::default(),
                git: 0,
            });
            self.nodes[parent].children.push(i);
            self.index.insert(e.rel.clone(), i);
            return WalkControl::Continue;
        }
        let size = match e.meta.kind {
            EntryKind::File => e.meta.size,
            EntryKind::Reparse(ReparseKind::CloudPlaceholder) => e.meta.size,
            EntryKind::Reparse(ReparseKind::AppExecLink) => 0,
            // Links are not counted (SPEC-03 §4.3 step 3).
            EntryKind::Dir | EntryKind::Reparse(_) => return WalkControl::Continue,
        };
        if is_named(".git") {
            self.nodes[parent].git |= GIT_FILE;
        } else if is_named("HEAD") {
            self.nodes[parent].git |= BARE_HEAD;
        }
        let kind = self.kind_of(&e.path);
        if let Some(kind @ (ContentKind::Credentials | ContentKind::EbookMail)) = kind {
            self.valuable.push(ValuableFile {
                path: e.path.clone(),
                size,
                kind,
            });
        }
        self.nodes[parent].own.add_file(size, &e.meta, kind);
        WalkControl::Continue
    }

    /// Content kind of a file by its extension (`Path::extension`, as in
    /// SPEC-03 §4.4: `.gitignore` has none).
    fn kind_of(&self, path: &Path) -> Option<ContentKind> {
        let ext = path.extension()?.to_string_lossy();
        self.table?.group_of(&ext).map(|g| g.kind)
    }

    /// Appends the folders of the root to `out` in pre-order, children by
    /// name, and rolls the aggregates up (step 2).
    pub(crate) fn finish(mut self, root_errors: u64, out: &mut RootsWalk) {
        let names: Vec<(String, String)> = self.nodes.iter().map(|n| name_key(&n.path)).collect();
        for node in &mut self.nodes {
            node.children
                .sort_by(|&a, &b| by_name(&names[a], &names[b]));
        }
        let order = pre_order(&self.nodes);
        let base = out.dirs.len();
        let mut new_index = vec![0; self.nodes.len()];
        for (pos, &old) in order.iter().enumerate() {
            new_index[old] = base + pos;
        }
        let mut raw: Vec<Option<RawNode>> = self.nodes.into_iter().map(Some).collect();
        for &old in &order {
            let Some(node) = raw[old].take() else {
                continue;
            };
            out.dirs.push(DirNode {
                path: node.path,
                depth: node.depth,
                parent: node.parent.map(|p| new_index[p]),
                end: 0,
                subtree: node.own.clone(),
                own: node.own,
                git: node.git,
            });
        }
        // Reverse pre-order reaches every folder after all its descendants.
        let mut sizes = vec![1usize; out.dirs.len() - base];
        for i in (base..out.dirs.len()).rev() {
            let node = &mut out.dirs[i];
            node.end = i + sizes[i - base];
            node.own.finish(self.table);
            node.subtree.finish(self.table);
            if let Some(p) = node.parent {
                let subtree = node.subtree.clone();
                sizes[p - base] += sizes[i - base];
                out.dirs[p].subtree.add(&subtree);
            }
        }
        out.roots.push(WalkedRoot {
            path: out.dirs[base].path.clone(),
            node: base,
            errors: root_errors,
        });
        out.valuable_files.append(&mut self.valuable);
    }
}

/// Indexes of `nodes` in pre-order from node 0, children in their order.
fn pre_order(nodes: &[RawNode]) -> Vec<usize> {
    let mut order = Vec::with_capacity(nodes.len());
    let mut stack = vec![0];
    while let Some(i) = stack.pop() {
        order.push(i);
        stack.extend(nodes[i].children.iter().rev());
    }
    order
}

/// Sort key of a folder name: lowercase, then as is.
fn name_key(path: &Path) -> (String, String) {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    (name.to_lowercase(), name)
}

fn by_name(a: &(String, String), b: &(String, String)) -> Ordering {
    a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1))
}
