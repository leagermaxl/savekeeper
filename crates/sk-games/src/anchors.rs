//! Anchor index of manifest entries for games that are not installed
//! (SPEC-05 §4.6, FR-05-04).
//!
//! Checking every path of 20 000+ games with `exists` would take ~100 000
//! file system calls. Instead, every entry that resolves without a game
//! context gets an **anchor**: the root of its template (a token or a drive)
//! and its first one or two static segments, e.g. `{APPDATA}\EldenRing` or
//! `{LOCALLOW}\Team Cherry\Hollow Knight`. A scan lists each root (and, where
//! two-segment anchors need it, the matching folders one level below) and
//! fully checks only the entries whose anchor exists.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use sk_core::env::Environment;
use sk_core::fs::{DirEntryInfo, EntryKind, FsError, FsScanner};
use sk_core::template::{PathTemplate, ResolveContext};
use sk_core::CancellationToken;

use crate::manifest::{Manifest, Os, When};
use crate::translate::{translate, GameCtx};
use crate::when::{store_launcher, when_applies};
use crate::GamesError;

/// Static segments after the root that make an anchor, at most.
const ANCHOR_DEPTH: usize = 2;

/// A `files` entry of a game that resolves without a game context.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AnchoredRule {
    /// Key of the game in [`Manifest::games`] (never an alias entry).
    pub(crate) key: String,
    /// The `files` key of the entry, as written in the manifest.
    pub(crate) path: String,
    /// Static part of the path ([`translate`] with [`GameCtx::default`]).
    pub(crate) template: PathTemplate,
    /// Include globs relative to [`template`](Self::template); empty for a
    /// path without globs.
    pub(crate) include: Vec<String>,
    /// Conditions of the entry, checked against the detected launchers.
    pub(crate) when: Vec<When>,
}

/// An entry whose template exists on this machine.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AnchorHit<'a> {
    /// The entry.
    pub(crate) rule: &'a AnchoredRule,
    /// Resolved paths of its template that exist.
    pub(crate) paths: Vec<PathBuf>,
}

/// A root listed by the scan.
#[derive(Debug, Clone)]
struct Base {
    /// The root as a template: a token (`{APPDATA}`) or a drive (`D:`).
    template: PathTemplate,
    /// Lowercase first segments of the two-segment anchors under this root:
    /// these child folders are listed too.
    nested: BTreeSet<String>,
}

/// Anchors of the manifest entries of games that are not installed
/// (SPEC-05 §4.6).
///
/// Entries are kept in byte order of game keys and then of `files` keys, so
/// the result of [`find`](Self::find) does not depend on `HashMap` order.
#[derive(Debug, Clone, Default)]
pub(crate) struct AnchorIndex {
    /// Every indexed entry.
    rules: Vec<AnchoredRule>,
    /// Lowercase anchor → entries of [`rules`](Self::rules), in order.
    anchors: HashMap<String, Vec<usize>>,
    /// Entries without a static segment after the root (`<winAppData>/*/Saves`),
    /// or with fewer than two after a drive (`C:/Users/*/Game`).
    wide: Vec<usize>,
    /// Lowercase root → what to list there.
    bases: BTreeMap<String, Base>,
}

impl AnchorIndex {
    /// Indexes the `files` entries of `manifest` that a game which is not
    /// installed can have (FR-05-04, FR-05-05).
    ///
    /// Skipped: alias entries; entries that [`translate`] rejects without a
    /// game context (`<base>`, `<game>`, `<root>`, `<storeGameId>`,
    /// `<osUserName>`, unsupported placeholders); entries whose `when` can
    /// never hold on Windows (only other systems, or only stores without a
    /// launcher detector). An entry with a `store` condition is indexed and
    /// checked against the detected launchers by [`find`](Self::find).
    ///
    /// The anchor of an entry is its root and the first two static segments
    /// of its template, or the only one; an entry without static segments
    /// after the root, or with fewer than two after a drive root, is "wide"
    /// ([`wide`](Self::wide)).
    pub(crate) fn new(manifest: &Manifest) -> Self {
        let ctx = GameCtx::default();
        let mut keys: Vec<&String> = manifest
            .games
            .iter()
            .filter(|(_, game)| !game.is_alias())
            .map(|(key, _)| key)
            .collect();
        keys.sort_unstable();

        let mut index = Self::default();
        for key in keys {
            let Some(game) = manifest.games.get(key) else {
                continue;
            };
            for (path, rule) in &game.files {
                if !may_apply_on_windows(&rule.when) {
                    continue;
                }
                let Some((template, include)) = translate(path, &ctx) else {
                    continue;
                };
                let entry = index.rules.len();
                if !index.add_anchor(&template, entry) {
                    index.wide.push(entry);
                }
                index.rules.push(AnchoredRule {
                    key: key.clone(),
                    path: path.clone(),
                    template,
                    include,
                    when: rule.when.clone(),
                });
            }
        }
        index
    }

    /// Entries that apply on this machine and whose template exists, for the
    /// games that are not in `installed` (keys of [`Manifest::games`]).
    ///
    /// Lists every root of the index and the folders under it that start a
    /// two-segment anchor (SPEC-05 §4.6 step 3), then resolves the templates
    /// of the entries with an existing anchor and checks each path with
    /// `exists` (step 4). Entries whose `when` does not hold for
    /// `env.launchers` are not checked. A root or folder that cannot be
    /// listed is skipped.
    ///
    /// # Errors
    /// [`GamesError::Cancelled`] when `cancel` is cancelled.
    pub(crate) fn find(
        &self,
        fs: &dyn FsScanner,
        env: &Environment,
        installed: &HashSet<&str>,
        cancel: &CancellationToken,
    ) -> Result<Vec<AnchorHit<'_>>, GamesError> {
        let existing = self.existing_anchors(fs, env, cancel)?;
        let mut candidates: Vec<usize> = existing
            .iter()
            .filter_map(|anchor| self.anchors.get(anchor))
            .flatten()
            .copied()
            .collect();
        candidates.sort_unstable();

        let ctx = ResolveContext::default();
        let mut hits = Vec::new();
        for entry in candidates {
            if cancel.is_cancelled() {
                return Err(GamesError::Cancelled);
            }
            let Some(rule) = self.rules.get(entry) else {
                continue;
            };
            if installed.contains(rule.key.as_str()) || !when_applies(&rule.when, &env.launchers) {
                continue;
            }
            let paths: Vec<PathBuf> = rule
                .template
                .resolve(env, &ctx)
                .into_iter()
                .filter(|path| fs.exists(path))
                .collect();
            if !paths.is_empty() {
                hits.push(AnchorHit { rule, paths });
            }
        }
        Ok(hits)
    }

    /// "Wide" entries: the first segment after the root is a glob
    /// (`<winAppData>/*/Saves`), or the second one after a drive
    /// (`C:/Users/[User]/…`), so they have no anchor. They are checked only
    /// for installed games, like every entry of an installed game (§4.7 step 2).
    #[cfg_attr(not(test), allow(dead_code))] // GamesCollector checks every entry of an installed game
    pub(crate) fn wide(&self) -> impl Iterator<Item = &AnchoredRule> {
        self.wide.iter().filter_map(|&entry| self.rules.get(entry))
    }

    /// Lowercase anchors of the index that exist on this machine.
    fn existing_anchors(
        &self,
        fs: &dyn FsScanner,
        env: &Environment,
        cancel: &CancellationToken,
    ) -> Result<HashSet<String>, GamesError> {
        let ctx = ResolveContext::default();
        let mut found = HashSet::new();
        for (root, base) in &self.bases {
            for dir in base.template.resolve(env, &ctx) {
                for child in list(fs, &dir, cancel)? {
                    let Some(name) = entry_name(&child) else {
                        continue;
                    };
                    let anchor = format!("{root}\\{name}");
                    if child.meta.kind != EntryKind::File && base.nested.contains(&name) {
                        for grandchild in list(fs, &child.path, cancel)? {
                            let Some(inner) = entry_name(&grandchild) else {
                                continue;
                            };
                            let deep = format!("{anchor}\\{inner}");
                            if self.anchors.contains_key(&deep) {
                                found.insert(deep);
                            }
                        }
                    }
                    if self.anchors.contains_key(&anchor) {
                        found.insert(anchor);
                    }
                }
            }
        }
        Ok(found)
    }

    /// Records the anchor of entry `entry`; `false` when `template` has none.
    fn add_anchor(&mut self, template: &PathTemplate, entry: usize) -> bool {
        let text = template.as_str();
        if text.starts_with('\\') {
            // Rooted or UNC: `translate` never makes these.
            return false;
        }
        let mut segments = text.split('\\');
        let Some(root) = segments.next() else {
            return false;
        };
        let rest: Vec<String> = segments.take(ANCHOR_DEPTH).map(str::to_lowercase).collect();
        // A drive root needs two static segments: `C:\Users` always exists,
        // and `C:/Users/[User]/…` (a bracketed user name reads as a glob
        // class) would make every scan walk all user profiles (T-05-14).
        if rest.is_empty() || (is_drive(root) && rest.len() < ANCHOR_DEPTH) {
            return false;
        }
        let root_key = root.to_lowercase();
        let base = match self.bases.entry(root_key.clone()) {
            Entry::Occupied(occupied) => occupied.into_mut(),
            Entry::Vacant(vacant) => {
                let Ok(template) = PathTemplate::parse(root) else {
                    return false;
                };
                vacant.insert(Base {
                    template,
                    nested: BTreeSet::new(),
                })
            }
        };
        if let [first, _] = rest.as_slice() {
            base.nested.insert(first.clone());
        }
        let anchor = std::iter::once(root_key).chain(rest).collect::<Vec<_>>();
        self.anchors
            .entry(anchor.join("\\"))
            .or_default()
            .push(entry);
        true
    }
}

/// Whether some item of `when` can hold on Windows (FR-05-05): `os` missing
/// or `windows`, and `store` missing or one with a launcher detector.
fn may_apply_on_windows(when: &[When]) -> bool {
    when.is_empty()
        || when.iter().any(|item| {
            matches!(item.os, None | Some(Os::Windows))
                && item
                    .store
                    .as_ref()
                    .is_none_or(|store| store_launcher(store).is_some())
        })
}

/// Whether the first segment of a template is a drive (`D:`), not a token.
fn is_drive(root: &str) -> bool {
    matches!(root.as_bytes(), [letter, b':'] if letter.is_ascii_alphabetic())
}

/// Entries of one folder; nothing when it is missing or cannot be listed.
fn list(
    fs: &dyn FsScanner,
    dir: &Path,
    cancel: &CancellationToken,
) -> Result<Vec<DirEntryInfo>, GamesError> {
    if cancel.is_cancelled() {
        return Err(GamesError::Cancelled);
    }
    match fs.read_dir(dir) {
        Ok(entries) => Ok(entries),
        Err(FsError::NotFound) => Ok(Vec::new()),
        Err(FsError::Cancelled) => Err(GamesError::Cancelled),
        Err(e) => {
            tracing::debug!(dir = %dir.display(), error = %e, "anchor folder not listed");
            Ok(Vec::new())
        }
    }
}

/// Lowercase last component of a listed entry.
fn entry_name(entry: &DirEntryInfo) -> Option<String> {
    entry
        .path
        .file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
}

#[cfg(test)]
#[path = "anchors_tests.rs"]
mod tests;
