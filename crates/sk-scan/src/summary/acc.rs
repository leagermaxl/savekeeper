//! Running state of `summarize`: one pass over the walked entries, with
//! bounded selections for the samples (SPEC-03 §4.4 «Аккумуляция»).

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::path::PathBuf;

use time::OffsetDateTime;

use super::rules::{
    is_cache_name, is_project_file, CACHE_EXTS, CONFIG_EXTS, DOC_EXTS, ELECTRON_NAMES, EXEC_EXTS,
    MEDIA_EXTS, SQLITE_EXTS, UNITY_LOGS,
};
use crate::measure::{counted, file_size, Counted};
use crate::{CloudState, DirEntryInfo};

/// Entries of the first `sample_names` step.
pub(super) const TOP_LEVEL_SAMPLES: usize = 5;
/// Files of the second `sample_names` step.
pub(super) const NEWEST_SAMPLES: usize = 10;
/// Signature reads of `SqliteFiles` at most.
pub(super) const SQLITE_READS: usize = 5;
/// Minimum size of an SQLite candidate: the signature length.
const SQLITE_MIN_SIZE: u64 = 16;

/// A relative path, ordered by its lowercase form, then ordinally.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct RelPath {
    pub(super) lower: String,
    /// Components joined with `\`, lossy UTF-8.
    pub(super) display: String,
}

/// The `n` smallest items pushed so far.
#[derive(Debug)]
pub(super) struct Smallest<T: Ord> {
    n: usize,
    heap: BinaryHeap<T>,
}

impl<T: Ord> Smallest<T> {
    fn new(n: usize) -> Self {
        Self {
            n,
            heap: BinaryHeap::with_capacity(n + 1),
        }
    }

    fn push(&mut self, item: T) {
        self.heap.push(item);
        if self.heap.len() > self.n {
            self.heap.pop();
        }
    }

    /// The kept items in ascending order.
    pub(super) fn into_sorted(self) -> Vec<T> {
        self.heap.into_sorted_vec()
    }
}

/// Files of one extension.
#[derive(Debug, Clone)]
pub(super) struct ExtAcc {
    pub(super) count: u64,
    pub(super) bytes: u64,
    /// The file with the smallest path (third `sample_names` step).
    pub(super) first: RelPath,
}

/// A direct subfolder.
#[derive(Debug, Clone, Default)]
pub(super) struct ChildAcc {
    pub(super) name: String,
    pub(super) bytes: u64,
    pub(super) files: u64,
}

/// A top-level entry: its path and, for a file, its extension.
pub(super) type TopLevel = (RelPath, Option<String>);
/// A file with a time, newest first, then by path; with its extension.
pub(super) type Fresh = (Reverse<OffsetDateTime>, RelPath, String);
/// An SQLite candidate: depth, path, absolute path.
pub(super) type SqliteCandidate = (u32, RelPath, PathBuf);

/// Everything `summarize` collects during the walk.
#[derive(Debug)]
pub(super) struct Acc {
    pub(super) total_bytes: u64,
    pub(super) file_count: u64,
    pub(super) dir_count: u64,
    pub(super) max_depth: u32,
    pub(super) newest: Option<OffsetDateTime>,
    pub(super) oldest: Option<OffsetDateTime>,
    pub(super) exts: HashMap<String, ExtAcc>,
    pub(super) top_level: Smallest<TopLevel>,
    /// Enough newest files to fill the second step after the first one.
    pub(super) fresh: Smallest<Fresh>,
    /// Direct subfolders by name.
    pub(super) children: HashMap<String, ChildAcc>,
    sqlite: Smallest<SqliteCandidate>,
    // Shares.
    pub(super) exec_bytes: u64,
    pub(super) shallow_exe_files: u64,
    pub(super) cache_ext_files: u64,
    pub(super) config_files: u64,
    pub(super) media_bytes: u64,
    pub(super) doc_files: u64,
    /// Bytes of direct child files with cache-like names.
    pub(super) cache_child_file_bytes: u64,
    // Names.
    pub(super) git: bool,
    pub(super) electron: u32,
    pub(super) local_state: bool,
    pub(super) default_preferences: bool,
    pub(super) preferences: bool,
    pub(super) bookmarks_or_history: bool,
    pub(super) unity_log: bool,
    pub(super) unity_dir: bool,
    /// Folder (lowercase relative path) → (has a `*_Data` folder, has `UnityPlayer.dll`).
    pub(super) unity_player: HashMap<String, (bool, bool)>,
    /// Folder → (has a `ProjectSettings` folder, has an `Assets` folder).
    pub(super) unity_project: HashMap<String, (bool, bool)>,
    pub(super) unreal_save_games: bool,
    /// `Saved` folder → (has `Config\Windows*`, has a `.sav` below).
    pub(super) unreal_saved: HashMap<String, (bool, bool)>,
    pub(super) project_file: bool,
    /// Lowercase name of the summarized folder: the folder of depth 0.
    pub(super) dir_name: String,
}

impl Acc {
    /// An empty state for the folder named `dir_name` (any case).
    pub(super) fn new(dir_name: &str) -> Self {
        Self {
            total_bytes: 0,
            file_count: 0,
            dir_count: 0,
            max_depth: 0,
            newest: None,
            oldest: None,
            exts: HashMap::new(),
            top_level: Smallest::new(TOP_LEVEL_SAMPLES),
            fresh: Smallest::new(TOP_LEVEL_SAMPLES + NEWEST_SAMPLES),
            children: HashMap::new(),
            sqlite: Smallest::new(SQLITE_READS),
            exec_bytes: 0,
            shallow_exe_files: 0,
            cache_ext_files: 0,
            config_files: 0,
            media_bytes: 0,
            doc_files: 0,
            cache_child_file_bytes: 0,
            git: false,
            electron: 0,
            local_state: false,
            default_preferences: false,
            preferences: false,
            bookmarks_or_history: false,
            unity_log: false,
            unity_dir: false,
            unity_player: HashMap::new(),
            unity_project: HashMap::new(),
            unreal_save_games: false,
            unreal_saved: HashMap::new(),
            project_file: false,
            dir_name: dir_name.to_lowercase(),
        }
    }

    /// Lowercase name of the folder of depth `d` on the way to an entry with
    /// components `lower` (§4.4 «Глубина»: `dir` itself has depth 0).
    fn name_at<'a>(&'a self, lower: &'a [String], d: usize) -> &'a str {
        match d.checked_sub(1) {
            None => &self.dir_name,
            Some(i) => lower.get(i).map_or("", String::as_str),
        }
    }

    /// Takes one walked entry into account (symlinks and junctions are ignored).
    pub(super) fn visit(&mut self, e: &DirEntryInfo) {
        let kind = counted(&e.meta);
        if kind == Counted::Not {
            return;
        }
        let names: Vec<String> = e
            .rel
            .iter()
            .map(|c| c.to_string_lossy().into_owned())
            .collect();
        let lower: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();
        let (Some(name), Some(first)) = (lower.last(), names.first()) else {
            return;
        };
        let path = RelPath {
            lower: lower.join("\\"),
            display: names.join("\\"),
        };
        let parent = lower[..lower.len() - 1].join("\\");
        self.max_depth = self.max_depth.max(e.depth);
        if e.depth == 1 {
            self.top_name(name);
        } else if lower.len() == 2 && lower[0] == "default" && lower[1] == "preferences" {
            self.default_preferences = true;
        }
        if kind == Counted::File {
            self.file(e, &lower, path, parent, first);
        } else {
            self.dir(e, &lower, path, parent, first);
        }
    }

    /// Names of direct children that count for any entry kind.
    fn top_name(&mut self, name: &str) {
        match name {
            ".git" => self.git = true,
            "local state" => self.local_state = true,
            "preferences" => self.preferences = true,
            "bookmarks" | "history" => self.bookmarks_or_history = true,
            _ => {}
        }
        if let Some(i) = ELECTRON_NAMES.iter().position(|n| *n == name) {
            self.electron |= 1 << i;
        }
    }

    fn file(
        &mut self,
        e: &DirEntryInfo,
        lower: &[String],
        path: RelPath,
        parent: String,
        first: &str,
    ) {
        let size = file_size(&e.meta);
        let name = lower.last().map_or("", String::as_str);
        let ext = e
            .rel
            .extension()
            .map(|x| x.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        self.total_bytes = self.total_bytes.saturating_add(size);
        self.file_count += 1;
        if let Some(t) = e.meta.mtime {
            self.newest = Some(self.newest.map_or(t, |n| n.max(t)));
            self.oldest = Some(self.oldest.map_or(t, |o| o.min(t)));
            self.fresh.push((Reverse(t), path.clone(), ext.clone()));
        }
        let x = ext.as_str();
        if EXEC_EXTS.contains(&x) {
            self.exec_bytes = self.exec_bytes.saturating_add(size);
        }
        if x == "exe" && e.depth <= 2 {
            self.shallow_exe_files += 1;
        }
        if CACHE_EXTS.contains(&x) {
            self.cache_ext_files += 1;
        }
        if CONFIG_EXTS.contains(&x) {
            self.config_files += 1;
        }
        if MEDIA_EXTS.contains(&x) {
            self.media_bytes = self.media_bytes.saturating_add(size);
        }
        if DOC_EXTS.contains(&x) {
            self.doc_files += 1;
        }
        if SQLITE_EXTS.contains(&x) && e.meta.cloud == CloudState::Local && size >= SQLITE_MIN_SIZE
        {
            self.sqlite.push((e.depth, path.clone(), e.path.clone()));
        }
        if e.depth <= 3 && is_project_file(name) {
            self.project_file = true;
        }
        if name == "unityplayer.dll" {
            self.unity_player.entry(parent).or_default().1 = true;
        }
        if x == "sav" {
            // `Saved` folders of depth ≤ 3 above the file, `dir` included.
            for d in 0..=lower.len().saturating_sub(1).min(3) {
                if self.name_at(lower, d) == "saved" {
                    let key = lower[..d].join("\\");
                    self.unreal_saved.entry(key).or_default().1 = true;
                }
            }
        }
        if e.depth == 1 {
            if UNITY_LOGS.contains(&name) {
                self.unity_log = true;
            }
            if is_cache_name(name) {
                self.cache_child_file_bytes = self.cache_child_file_bytes.saturating_add(size);
            }
            self.top_level.push((path.clone(), Some(ext.clone())));
        } else {
            let child = self.child(first);
            child.bytes = child.bytes.saturating_add(size);
            child.files += 1;
        }
        match self.exts.get_mut(&ext) {
            Some(a) => {
                a.count += 1;
                a.bytes = a.bytes.saturating_add(size);
                if path < a.first {
                    a.first = path;
                }
            }
            None => {
                let a = ExtAcc {
                    count: 1,
                    bytes: size,
                    first: path,
                };
                self.exts.insert(ext, a);
            }
        }
    }

    fn dir(
        &mut self,
        e: &DirEntryInfo,
        lower: &[String],
        path: RelPath,
        parent: String,
        first: &str,
    ) {
        let name = lower.last().map_or("", String::as_str);
        let n = lower.len();
        self.dir_count += 1;
        self.child(first);
        if e.depth == 1 {
            if name == "unity" {
                self.unity_dir = true;
            }
            self.top_level.push((path, None));
        }
        if name.ends_with("_data") {
            self.unity_player.entry(parent.clone()).or_default().0 = true;
        }
        if e.depth <= 3 && (name == "projectsettings" || name == "assets") {
            let entry = self.unity_project.entry(parent).or_default();
            if name == "assets" {
                entry.1 = true;
            } else {
                entry.0 = true;
            }
        }
        // `SaveGames` of depth `n` ≤ 3 under a `Saved` folder of depth `n - 1` ≥ 0.
        if n <= 3 && name == "savegames" && self.name_at(lower, n - 1) == "saved" {
            self.unreal_save_games = true;
        }
        // `Saved\Config\Windows*` with `Saved` of depth `n - 2` ≤ 3 (`dir` included).
        if n >= 2
            && n - 2 <= 3
            && name.starts_with("windows")
            && self.name_at(lower, n - 1) == "config"
            && self.name_at(lower, n - 2) == "saved"
        {
            let key = lower[..n - 2].join("\\");
            self.unreal_saved.entry(key).or_default().0 = true;
        }
    }

    /// The SQLite candidates in reading order; the selection is left empty.
    pub(super) fn take_sqlite(&mut self) -> Vec<SqliteCandidate> {
        std::mem::replace(&mut self.sqlite, Smallest::new(SQLITE_READS)).into_sorted()
    }

    /// The direct subfolder `name`, created on first use.
    fn child(&mut self, name: &str) -> &mut ChildAcc {
        self.children
            .entry(name.to_owned())
            .or_insert_with(|| ChildAcc {
                name: name.to_owned(),
                ..ChildAcc::default()
            })
    }
}
