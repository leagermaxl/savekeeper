//! `DirStatsCache`: stats of walked roots and their subfolders within one scan
//! (SPEC-03 §4.3, FR-03-08).

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};

use sk_core::model::TargetStats;

use super::Mode;

/// Cache of per-folder stats; `Send + Sync`, one per scan (SPEC-03 §4.3).
///
/// [`measure`](super::measure) stores the stats of every completely walked
/// root, and for a root without `include`/`exclude` also those of its walked
/// subfolders down to depth 3, so that a nested root is not walked again.
/// Nothing is stored for a cancelled or failed walk. `measure_all` creates its own.
#[derive(Debug, Default)]
pub struct DirStatsCache {
    map: Mutex<HashMap<CacheKey, Cached>>,
}

/// A cached result: the stats and the number of walk errors behind them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Cached {
    pub(crate) stats: TargetStats,
    pub(crate) errors: u64,
}

/// Key of a cached result (§4.3): the normalized root, the globs as given
/// and the exclusion mode.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct CacheKey {
    /// Normalized path, see [`norm_path`].
    pub(crate) path: String,
    pub(crate) include: Vec<String>,
    pub(crate) exclude: Vec<String>,
    pub(crate) mode: Mode,
}

impl CacheKey {
    pub(crate) fn new(root: &Path, include: &[String], exclude: &[String], mode: Mode) -> Self {
        Self {
            path: norm_path(root),
            include: include.to_vec(),
            exclude: exclude.to_vec(),
            mode,
        }
    }

    /// Whether this key is a subfolder of `parent` 1 to `max` levels down,
    /// both without globs and with the same mode.
    pub(crate) fn nested_in(&self, parent: &CacheKey, max: usize) -> bool {
        if !self.no_globs() || !parent.no_globs() || self.mode != parent.mode {
            return false;
        }
        let Some(rest) = self.path.strip_prefix(&parent.path) else {
            return false;
        };
        // A parent that is a drive root (`c:\`) already ends with the separator.
        let rest = if parent.path.ends_with('\\') {
            rest
        } else {
            match rest.strip_prefix('\\') {
                Some(rest) => rest,
                None => return false,
            }
        };
        let levels = rest.split('\\').filter(|c| !c.is_empty()).count();
        (1..=max).contains(&levels)
    }

    fn no_globs(&self) -> bool {
        self.include.is_empty() && self.exclude.is_empty()
    }
}

/// The path without `\\?\`, lowercase, with `\` separators and without a
/// trailing separator (except for a root such as `c:\` or `\`).
pub(crate) fn norm_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    let s = match s.strip_prefix(r"\\?\UNC\") {
        Some(rest) => format!(r"\\{rest}"),
        None => s.strip_prefix(r"\\?\").unwrap_or(&s).to_owned(),
    };
    let mut s = s.replace('/', "\\").to_lowercase();
    while s.len() > 1 && s.ends_with('\\') && !s.ends_with(":\\") {
        s.pop();
    }
    s
}

impl DirStatsCache {
    /// An empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn get(&self, key: &CacheKey) -> Option<Cached> {
        self.lock().get(key).cloned()
    }

    pub(crate) fn put(&self, key: CacheKey, value: Cached) {
        self.lock().insert(key, value);
    }

    /// Stores the stats of subfolders of `root` given by their lowercase
    /// relative components; entries already present are kept.
    pub(crate) fn put_subfolders(
        &self,
        root: &CacheKey,
        subs: impl IntoIterator<Item = (Vec<String>, TargetStats)>,
    ) {
        let mut map = self.lock();
        for (rel, stats) in subs {
            let mut path = root.path.clone();
            for c in rel {
                if !path.ends_with('\\') {
                    path.push('\\');
                }
                path.push_str(&c);
            }
            let key = CacheKey {
                path,
                include: Vec::new(),
                exclude: Vec::new(),
                mode: root.mode,
            };
            map.entry(key).or_insert(Cached { stats, errors: 0 });
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.lock().len()
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<CacheKey, Cached>> {
        // The map stays consistent even if a holder panicked.
        self.map.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
