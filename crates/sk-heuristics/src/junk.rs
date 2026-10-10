//! H-JUNK: junk and reinstallable folders (SPEC-07 §4.5).
//!
//! [`junk_verdict`] looks only at the `FolderSummary` and the `Environment`.
//! A folder is matched by its name (the last segments of its path), by its
//! markers and by the programs, launchers and games of the environment.
//! Contents never save a folder whose name says it is junk: a `settings.json`
//! inside `logs` is the business of the rules (SPEC-04).
//!
//! When several rows of the table match, the most confident one wins; rows
//! with the same confidence win in the order of the table.

use std::path::{Component, Path, PathBuf};

use sk_core::env::Environment;
use sk_core::model::{Category, FolderSummary, Marker};
use sk_core::path::{eq_ci, starts_with_ci};
use sk_core::template::ResolveContext;

use crate::verdict::MarkerVerdict;

#[cfg(test)]
#[path = "junk_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "junk_env_tests.rs"]
mod env_tests;

/// Reason: a dependency or build folder that a package manager or build restores.
pub(crate) const REASON_RESTORABLE: &str = "heuristic.junk.restorable";
/// Reason: a folder of a game launcher or a game library.
pub(crate) const REASON_LAUNCHER: &str = "heuristic.junk.launcher";
/// Reason: the name of a cache, temp or log folder.
pub(crate) const REASON_CACHE_NAME: &str = "heuristic.junk.cache_name";
/// Reason: programs inside the installation folder of an installed program.
pub(crate) const REASON_INSTALLED: &str = "heuristic.junk.installed_program";
/// Reason: the name of known downloadable content (updates, installer caches).
pub(crate) const REASON_DOWNLOADABLE: &str = "heuristic.junk.downloadable";
/// Reason: the `CacheLike` marker without `ConfigLike`.
pub(crate) const REASON_CACHE_LIKE: &str = "heuristic.junk.cache_like";
/// Reason: programs whose files were all written at the same time.
pub(crate) const REASON_UNIFORM_BINARIES: &str = "heuristic.junk.uniform_binaries";

/// Folder names that are caches, temporary data or logs (§4.5, row 1).
const CACHE_NAMES: &[&[&str]] = &[
    &["cache"],
    &["caches"],
    &["cache2"],
    &["gpucache"],
    &["code cache"],
    &["shadercache"],
    &["dxcache"],
    &["glcache"],
    &["temp"],
    &["tmp"],
    &["crashdumps"],
    &["crashpad"],
    &["logs"],
    &["log"],
    &["blob_storage"],
    &["service worker", "cachestorage"],
    &["webcache"],
];

/// Dependency and build folders restored without conditions (§4.5, row 3).
const RESTORABLE_NAMES: &[&[&str]] = &[
    &["node_modules"],
    &[".venv"],
    &["venv"],
    &["__pycache__"],
    &[".gradle"],
    &[".m2", "repository"],
    &[".nuget", "packages"],
    &["pip", "cache"],
];

/// Known downloadable content (§4.5, row 7); `*\Updates` is any `Updates`.
const DOWNLOADABLE_NAMES: &[&[&str]] = &[
    &["steam", "steamapps", "shadercache"],
    &["nvidia", "dxcache"],
    &["package cache"],
    &["downloaded installations"],
    &["squirreltemp"],
    &["updates"],
    &["update"],
];

/// Largest spread of file times of a folder of identical binaries: 1 hour.
const UNIFORM_MTIME_SECONDS: i64 = 3600;

/// Whether the folder is junk or reinstallable (SPEC-07 §4.5), from its
/// summary alone.
///
/// The rows that depend on the folder's neighbours (`target` next to
/// `Cargo.toml`, `bin`/`obj` next to `*.csproj`) never match here; use
/// [`junk_verdict_with_siblings`] when the parent's listing is known.
pub fn junk_verdict(s: &FolderSummary, env: &Environment) -> Option<MarkerVerdict> {
    junk_verdict_with_siblings::<&str>(s, env, &[])
}

/// [`junk_verdict`] for a folder whose neighbours are known: `siblings` are the
/// names of the other entries (files and folders) of its parent folder.
pub fn junk_verdict_with_siblings<S: AsRef<str>>(
    s: &FolderSummary,
    env: &Environment,
    siblings: &[S],
) -> Option<MarkerVerdict> {
    let resolved = s.path.resolve(env, &ResolveContext::default());
    let names = segment_lists(s, &resolved);
    let has_marker = |m: Marker| s.markers.contains(&m);

    if is_restorable(&names, siblings) {
        return Some(MarkerVerdict::whole(
            Category::Cache,
            0.95,
            REASON_RESTORABLE,
        ));
    }
    if is_launcher_or_library(&names, &resolved, env) {
        return Some(MarkerVerdict::whole(
            Category::Reinstallable,
            0.95,
            REASON_LAUNCHER,
        ));
    }
    if matches_any(&names, CACHE_NAMES) {
        return Some(MarkerVerdict::whole(
            Category::Cache,
            0.9,
            REASON_CACHE_NAME,
        ));
    }
    if has_marker(Marker::HasExecutables) && is_inside_install_location(&resolved, env) {
        return Some(MarkerVerdict::whole(
            Category::Reinstallable,
            0.9,
            REASON_INSTALLED,
        ));
    }
    if matches_any(&names, DOWNLOADABLE_NAMES) {
        return Some(MarkerVerdict::whole(
            Category::Cache,
            0.9,
            REASON_DOWNLOADABLE,
        ));
    }
    if has_marker(Marker::CacheLike) && !has_marker(Marker::ConfigLike) {
        return Some(MarkerVerdict::whole(
            Category::Cache,
            0.75,
            REASON_CACHE_LIKE,
        ));
    }
    if has_marker(Marker::HasExecutables)
        && !has_marker(Marker::ConfigLike)
        && !has_marker(Marker::SqliteFiles)
        && has_uniform_mtime(s)
    {
        return Some(MarkerVerdict::whole(
            Category::Reinstallable,
            0.7,
            REASON_UNIFORM_BINARIES,
        ));
    }
    None
}

/// Lowercase path segments of the folder: those of its template and those of
/// every path it resolves to (`{STEAM}\steamapps` resolves to `…\Steam\steamapps`).
fn segment_lists(s: &FolderSummary, resolved: &[PathBuf]) -> Vec<Vec<String>> {
    let mut lists = vec![s
        .path
        .as_str()
        .split('\\')
        .map(str::to_lowercase)
        .collect::<Vec<_>>()];
    lists.extend(resolved.iter().map(|path| {
        path.components()
            .filter_map(|c| match c {
                Component::Normal(name) => Some(name.to_string_lossy().to_lowercase()),
                _ => None,
            })
            .collect::<Vec<_>>()
    }));
    lists
}

/// Whether one of the segment lists ends with one of the lowercase `patterns`.
fn matches_any(names: &[Vec<String>], patterns: &[&[&str]]) -> bool {
    names.iter().any(|list| {
        patterns.iter().any(|pattern| {
            list.len() >= pattern.len()
                && list[list.len() - pattern.len()..]
                    .iter()
                    .zip(pattern.iter())
                    .all(|(segment, expected)| segment == expected)
        })
    })
}

/// §4.5, row 3: dependency and build folders.
fn is_restorable<S: AsRef<str>>(names: &[Vec<String>], siblings: &[S]) -> bool {
    if matches_any(names, RESTORABLE_NAMES) {
        return true;
    }
    let sibling = |pred: &dyn Fn(&str) -> bool| {
        siblings
            .iter()
            .any(|name| pred(&name.as_ref().to_lowercase()))
    };
    (matches_any(names, &[&["target"]]) && sibling(&|name| name == "cargo.toml"))
        || (matches_any(names, &[&["bin"], &["obj"]])
            && sibling(&|name| name.len() > ".csproj".len() && name.ends_with(".csproj")))
}

/// §4.5, row 6: the folder is a launcher root, a game's installation folder
/// or a game library.
///
/// A game library is a `steamapps\common` folder, or the parent of the
/// installation folders of at least two different known games. The parent of
/// a single game is not a library: a game installed into `D:\Stuff\Game` must
/// not make the user's `D:\Stuff` reinstallable.
fn is_launcher_or_library(names: &[Vec<String>], resolved: &[PathBuf], env: &Environment) -> bool {
    if matches_any(names, &[&["steamapps", "common"]]) {
        return true;
    }
    let roots = env.launchers.iter().filter_map(|l| l.root.as_deref());
    let mut installs: Vec<&Path> = Vec::new();
    for game in env.launchers.iter().flat_map(|l| l.games.iter()) {
        let dir = game.install_dir.as_path();
        if !installs.iter().any(|known| eq_ci(known, dir)) {
            installs.push(dir);
        }
    }
    let mut libraries: Vec<&Path> = Vec::new();
    for parent in installs.iter().filter_map(|d| d.parent()) {
        let games_inside = installs
            .iter()
            .filter(|dir| dir.parent().is_some_and(|p| eq_ci(p, parent)))
            .count();
        if games_inside >= 2 && !libraries.iter().any(|known| eq_ci(known, parent)) {
            libraries.push(parent);
        }
    }
    let anchors: Vec<&Path> = roots
        .chain(installs.iter().copied())
        .chain(libraries)
        .filter(|anchor| is_specific(anchor, env))
        .collect();
    resolved
        .iter()
        .any(|path| anchors.iter().any(|anchor| eq_ci(path, anchor)))
}

/// §4.5, row 4: the folder is or lies inside the `InstallLocation` of an
/// installed program.
fn is_inside_install_location(resolved: &[PathBuf], env: &Environment) -> bool {
    let locations: Vec<&Path> = env
        .installed_programs
        .iter()
        .filter_map(|p| p.install_location.as_deref())
        .filter(|location| is_specific(location, env))
        .collect();
    resolved.iter().any(|path| {
        locations
            .iter()
            .any(|location| starts_with_ci(path, location))
    })
}

/// Whether an installation folder from the environment is specific enough to
/// mark folders as reinstallable. Broken uninstall entries and launcher data
/// can point at a drive root, `C:\Users` or `C:\Program Files`; such a folder
/// would make whole profiles reinstallable, so it is ignored.
pub(crate) fn is_specific(anchor: &Path, env: &Environment) -> bool {
    let has_name = anchor
        .components()
        .any(|c| matches!(c, Component::Normal(_)));
    has_name
        && !env
            .known_folders
            .values()
            .any(|known| starts_with_ci(known, anchor))
}

/// §4.5, row 5: newest and oldest file times differ by less than an hour.
fn has_uniform_mtime(s: &FolderSummary) -> bool {
    match (s.newest_mtime, s.oldest_mtime) {
        (Some(newest), Some(oldest)) => {
            (newest - oldest).abs().whole_seconds() < UNIFORM_MTIME_SECONDS
        }
        _ => false,
    }
}
