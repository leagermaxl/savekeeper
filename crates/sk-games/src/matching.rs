//! Matching installed games with manifest entries (SPEC-05 §4.5).
//!
//! [`MatchIndex`] is built once per manifest; it finds the manifest key of a
//! game reported by a launcher by store id (Steam, GOG), by normalized name
//! and by the name of the install folder (`installDir`).

use std::collections::HashMap;

use sk_core::env::{InstalledGame, LauncherInfo};

use crate::manifest::Manifest;

/// Confidence of a match picked among several equally good entries (§4.5 step 3).
pub(crate) const AMBIGUOUS_CONFIDENCE: f32 = 0.6;

/// Confidence of an unambiguous match.
pub(crate) const FULL_CONFIDENCE: f32 = 1.0;

/// Launcher ids whose `store_game_id` is a manifest id.
const STEAM: &str = "steam";
const GOG: &str = "gog";

/// What a match was found by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MatchBy {
    /// Steam app id or GOG product id (`steam.id`, `gog.id`, `id.steamExtra`, `id.gogExtra`).
    StoreId,
    /// Normalized game name ([`name_key`]).
    Name,
    /// Name of the install folder against `installDir` of the manifest.
    InstallDir,
}

/// The manifest entry of an installed game.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct GameMatch {
    /// Key of the entry in [`Manifest::games`]; never an alias entry.
    pub(crate) key: String,
    /// [`FULL_CONFIDENCE`], or [`AMBIGUOUS_CONFIDENCE`] when several entries matched equally.
    pub(crate) confidence: f32,
    /// What matched.
    pub(crate) by: MatchBy,
}

/// Lookup tables of a manifest for [`MatchIndex::match_game`].
///
/// Entries are numbered in byte order of their keys, and every candidate list
/// is kept in that order, so "the first entry" of §4.5 is the one with the
/// smallest key and the result does not depend on `HashMap` order.
#[derive(Debug, Clone, Default)]
pub(crate) struct MatchIndex {
    /// Keys of the non-alias entries, sorted.
    keys: Vec<String>,
    /// Lowercase `installDir` names of each entry of [`keys`](Self::keys).
    install_dirs: Vec<Vec<String>>,
    /// `steam.id` → entries.
    steam: HashMap<u32, Vec<usize>>,
    /// `id.steamExtra` → entries.
    steam_extra: HashMap<u32, Vec<usize>>,
    /// `gog.id` → entries.
    gog: HashMap<u64, Vec<usize>>,
    /// `id.gogExtra` → entries.
    gog_extra: HashMap<u64, Vec<usize>>,
    /// [`name_key`] of entry keys and of alias keys (→ alias target) → entries.
    names: HashMap<String, Vec<usize>>,
    /// Lowercase `installDir` name → entries.
    dirs: HashMap<String, Vec<usize>>,
}

#[cfg_attr(not(test), allow(dead_code))] // used by GamesCollector (T-05-09)
impl MatchIndex {
    /// Builds the tables of `manifest`.
    ///
    /// Alias entries have no data of their own: they are not matched by id or
    /// `installDir`, but their name leads to the entry they point to (if that
    /// exists and is not an alias itself).
    pub(crate) fn new(manifest: &Manifest) -> Self {
        let mut keys: Vec<&String> = manifest
            .games
            .iter()
            .filter(|(_, g)| !g.is_alias())
            .map(|(k, _)| k)
            .collect();
        keys.sort_unstable();
        let position: HashMap<&str, usize> = keys
            .iter()
            .enumerate()
            .map(|(i, k)| (k.as_str(), i))
            .collect();

        let mut index = Self::default();
        for (i, key) in keys.iter().enumerate() {
            let Some(game) = manifest.games.get(*key) else {
                continue;
            };
            if let Some(steam) = game.steam {
                push(&mut index.steam, steam.id, i);
            }
            if let Some(gog) = game.gog {
                push(&mut index.gog, gog.id, i);
            }
            if let Some(ids) = &game.id {
                for &id in &ids.steam_extra {
                    push(&mut index.steam_extra, id, i);
                }
                for &id in &ids.gog_extra {
                    push(&mut index.gog_extra, id, i);
                }
            }
            push_name(&mut index.names, key, i);
            let dirs: Vec<String> = game.install_dir.keys().map(|d| d.to_lowercase()).collect();
            for dir in &dirs {
                push(&mut index.dirs, dir.clone(), i);
            }
            index.install_dirs.push(dirs);
        }
        for (alias, game) in &manifest.games {
            let target = game.alias.as_deref().and_then(|t| position.get(t));
            if let Some(&i) = target {
                push_name(&mut index.names, alias, i);
            }
        }
        for list in index.names.values_mut() {
            list.sort_unstable();
            list.dedup();
        }
        index.keys = keys.into_iter().cloned().collect();
        index
    }

    /// The manifest entry of `game`, installed through launcher `launcher`
    /// (SPEC-05 §4.5), or `None` when nothing matches.
    ///
    /// 1. Steam and GOG games: by store id, `steam.id` / `gog.id` before
    ///    `id.steamExtra` / `id.gogExtra`.
    /// 2. Otherwise, and when the id is unknown to the manifest: by
    ///    [`name_key`] of the launcher name.
    /// 3. Otherwise: by the install folder name against `installDir`
    ///    (case-insensitive).
    ///
    /// When a step gives several entries, the one whose `installDir` contains
    /// the install folder name wins; if there is not exactly one such entry,
    /// the first remaining entry is taken with [`AMBIGUOUS_CONFIDENCE`].
    pub(crate) fn match_game(&self, launcher: &str, game: &InstalledGame) -> Option<GameMatch> {
        let folder = game
            .install_dir
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .filter(|n| !n.is_empty());
        let folder = folder.as_deref();
        let id = game.store_game_id.trim();
        let by_id = match launcher {
            STEAM => id
                .parse::<u32>()
                .ok()
                .and_then(|id| first_hit(&self.steam, &self.steam_extra, &id)),
            GOG => id
                .parse::<u64>()
                .ok()
                .and_then(|id| first_hit(&self.gog, &self.gog_extra, &id)),
            _ => None,
        };
        if let Some(candidates) = by_id {
            return self.pick(candidates, folder, MatchBy::StoreId);
        }
        let name = name_key(&game.name);
        if let Some(candidates) = self.names.get(&name).filter(|_| !name.is_empty()) {
            return self.pick(candidates, folder, MatchBy::Name);
        }
        let candidates = self.dirs.get(folder?)?;
        self.pick(candidates, folder, MatchBy::InstallDir)
    }

    /// Sets `manifest_key` of every game of `launchers` to its match (or `None`).
    pub(crate) fn annotate(&self, launchers: &mut [LauncherInfo]) {
        for launcher in launchers {
            for game in &mut launcher.games {
                game.manifest_key = self.match_game(&launcher.id, game).map(|m| m.key);
            }
        }
    }

    /// Chooses among `candidates` (sorted, not empty) as in §4.5 step 3.
    fn pick(&self, candidates: &[usize], folder: Option<&str>, by: MatchBy) -> Option<GameMatch> {
        let (&chosen, ambiguous) = match candidates {
            [] => return None,
            [only] => (only, false),
            _ => {
                let mut with_dir = candidates.iter().filter(|&&i| {
                    let dirs = self.install_dirs.get(i).map_or(&[][..], Vec::as_slice);
                    folder.is_some_and(|f| dirs.iter().any(|d| d == f))
                });
                match (with_dir.next(), with_dir.next()) {
                    (Some(i), None) => (i, false),
                    (Some(i), Some(_)) => (i, true),
                    (None, _) => (&candidates[0], true),
                }
            }
        };
        Some(GameMatch {
            key: self.keys.get(chosen)?.clone(),
            confidence: if ambiguous {
                AMBIGUOUS_CONFIDENCE
            } else {
                FULL_CONFIDENCE
            },
            by,
        })
    }
}

/// Normalized game name for matching (SPEC-05 §4.5): `™`, `®`, `©` are
/// dropped, letters are lowercased, every run of other characters than
/// letters and digits (spaces, `:`, `-`, `_`, `'`, ...) becomes one `-`, and
/// `-` is trimmed. For ASCII names this is the `AppRef` id of SPEC-02 §2.4;
/// non-ASCII letters are kept as they are (lowercased), so names in other
/// scripts still match each other. Empty for a name without letters or digits.
pub(crate) fn name_key(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars().filter(|c| !matches!(c, '™' | '®' | '©')) {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    if out.ends_with('-') {
        out.pop();
    }
    out
}

/// Entries of `id` in `primary`, else in `extra`.
fn first_hit<'a, K: std::hash::Hash + Eq>(
    primary: &'a HashMap<K, Vec<usize>>,
    extra: &'a HashMap<K, Vec<usize>>,
    id: &K,
) -> Option<&'a [usize]> {
    primary.get(id).or_else(|| extra.get(id)).map(Vec::as_slice)
}

fn push<K: std::hash::Hash + Eq>(map: &mut HashMap<K, Vec<usize>>, key: K, entry: usize) {
    let list = map.entry(key).or_default();
    if list.last() != Some(&entry) {
        list.push(entry);
    }
}

fn push_name(map: &mut HashMap<String, Vec<usize>>, name: &str, entry: usize) {
    let key = name_key(name);
    if !key.is_empty() {
        map.entry(key).or_default().push(entry);
    }
}

#[cfg(test)]
#[path = "matching_tests.rs"]
mod tests;
