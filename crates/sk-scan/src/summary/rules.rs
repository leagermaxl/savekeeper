//! Name and extension sets of the marker table and the marker decisions
//! (SPEC-03 §4.4 «Точные правила маркеров»). All names are compared in
//! lowercase.

use std::path::{Component, Path};

use sk_core::env::{Environment, KnownFolder};
use sk_core::model::Marker;
use sk_core::path::starts_with_ci;
use sk_core::template::PathTemplate;

use super::acc::Acc;

/// Executables and libraries (`HasExecutables`).
pub(super) const EXEC_EXTS: &[&str] = &["exe", "dll", "sys", "msi", "ocx"];
/// Temporary files, logs and dumps (`CacheLike`).
pub(super) const CACHE_EXTS: &[&str] = &["tmp", "log", "dmp", "etl"];
/// Configuration files (`ConfigLike`).
pub(super) const CONFIG_EXTS: &[&str] = &[
    "json", "ini", "xml", "cfg", "conf", "toml", "yaml", "yml", "reg", "config", "prefs", "plist",
];
/// Images, video and audio (`MediaHeavy`).
pub(super) const MEDIA_EXTS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "webp", "heic", "raw", "cr2", "nef", "arw", "dng", "psd", "tif",
    "tiff", "bmp", "mp4", "mkv", "mov", "avi", "webm", "m4v", "mp3", "flac", "wav", "ogg", "m4a",
    "aac", "opus",
];
/// Documents (`DocumentHeavy`).
pub(super) const DOC_EXTS: &[&str] = &[
    "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp", "pdf", "txt", "md", "rtf",
    "epub", "djvu",
];
/// SQLite databases (`SqliteFiles`).
pub(super) const SQLITE_EXTS: &[&str] = &["sqlite", "sqlite3", "db", "db3"];
/// The first 16 bytes of an SQLite database.
pub(super) const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\0";
/// Direct children of an Electron app folder (`ElectronApp`); bit `i` of
/// `Acc::electron` is set when name `i` is present.
pub(super) const ELECTRON_NAMES: &[&str] = &[
    "local storage",
    "indexeddb",
    "cache",
    "gpucache",
    "code cache",
    "session storage",
    "blob_storage",
    "service worker",
];
/// Unity player logs directly in the folder (`UnityGame`).
pub(super) const UNITY_LOGS: &[&str] = &["player.log", "player-prev.log", "output_log.txt"];
/// Exact project file names (`ProjectLike`).
const PROJECT_NAMES: &[&str] = &[
    "package.json",
    "cargo.toml",
    "go.mod",
    "pyproject.toml",
    "requirements.txt",
    "pom.xml",
    "cmakelists.txt",
    "makefile",
];
/// Project file name suffixes, from the `*.ext` globs (`ProjectLike`).
const PROJECT_SUFFIXES: &[&str] = &[
    ".sln",
    ".csproj",
    ".vcxproj",
    ".uproject",
    ".blend",
    ".aep",
    ".prproj",
    ".als",
    ".flp",
    ".rpp",
    ".kra",
    ".xcf",
];
/// `CacheLike` names that are not `*cache`.
const CACHE_NAMES: &[&str] = &[
    "caches",
    "temp",
    "tmp",
    "log",
    "logs",
    "crash",
    "crashes",
    "crashdumps",
    "crashpad",
];
/// 50 MiB: the size limit of `ConfigLike`.
const CONFIG_MAX_BYTES: u64 = 50 * 1024 * 1024;

/// `(?i)^(cache|caches|.*cache|temp|tmp|logs?|crash(es|dumps|pad)?|shadercache|dxcache|gpucache)$`
/// for a lowercase name.
pub(super) fn is_cache_name(lower: &str) -> bool {
    lower.ends_with("cache") || CACHE_NAMES.contains(&lower)
}

/// Whether a lowercase file name matches one of the project globs.
pub(super) fn is_project_file(lower: &str) -> bool {
    PROJECT_NAMES.contains(&lower)
        || lower.starts_with("build.gradle")
        || PROJECT_SUFFIXES.iter().any(|s| lower.ends_with(s))
}

/// `part / total > n%` in integers; false for `total == 0`.
fn share_above(part: u64, total: u64, n: u64) -> bool {
    total > 0 && u128::from(part) * 100 > u128::from(total) * u128::from(n)
}

/// `part / total >= n%` in integers; false for `total == 0`.
fn share_at_least(part: u64, total: u64, n: u64) -> bool {
    total > 0 && u128::from(part) * 100 >= u128::from(total) * u128::from(n)
}

/// Markers that depend only on the path of `dir` (also for a reparse root).
pub(super) fn path_markers(dir: &Path, template: &PathTemplate, env: &Environment) -> Vec<Marker> {
    let mut out = Vec::new();
    if env
        .cloud_roots
        .iter()
        .any(|root| starts_with_ci(dir, &root.path))
    {
        out.push(Marker::CloudSynced);
    }
    if is_package_dir(template) {
        out.push(Marker::UwpPackage);
    }
    out
}

/// `{PACKAGE:name}` without a tail.
fn is_package_dir(template: &PathTemplate) -> bool {
    let s = template.as_str();
    s.starts_with("{PACKAGE:") && s.ends_with('}') && !s.contains(['\\', '/'])
}

/// Number of path components, without `.`.
fn component_count(path: &Path) -> usize {
    path.components()
        .filter(|c| *c != Component::CurDir)
        .count()
}

/// `dir` is `{LOCALLOW}\Company\Product`.
fn in_locallow_product(dir: &Path, env: &Environment) -> bool {
    env.known_folder(KnownFolder::LocalLow).is_some_and(|base| {
        starts_with_ci(dir, base) && component_count(dir) == component_count(base) + 2
    })
}

/// All markers of a walked folder, in `Marker::ALL` order. `sqlite` is the
/// result of the signature reads done after the walk.
pub(super) fn markers(
    acc: &Acc,
    dir: &Path,
    template: &PathTemplate,
    env: &Environment,
    sqlite: bool,
) -> Vec<Marker> {
    let total = acc.total_bytes;
    let files = acc.file_count;
    let cache_bytes = acc.cache_child_file_bytes.saturating_add(
        acc.children
            .values()
            .filter(|c| is_cache_name(&c.name.to_lowercase()))
            .fold(0u64, |sum, c| sum.saturating_add(c.bytes)),
    );
    let path = path_markers(dir, template, env);
    let on = |m: Marker| -> bool {
        match m {
            Marker::HasExecutables => {
                share_above(acc.exec_bytes, total, 30) || acc.shallow_exe_files >= 3
            }
            Marker::GitRepo => acc.git,
            Marker::UnityGame => {
                (in_locallow_product(dir, env) && (acc.unity_log || acc.unity_dir))
                    || acc.unity_player.values().any(|&(data, dll)| data && dll)
            }
            Marker::UnrealSaveGames => {
                acc.unreal_save_games
                    || acc
                        .unreal_saved
                        .values()
                        .any(|&(config, sav)| config && sav)
            }
            Marker::ElectronApp => acc.electron.count_ones() >= 3,
            Marker::ChromiumProfile => {
                (acc.local_state && acc.default_preferences)
                    || (acc.preferences && acc.bookmarks_or_history)
            }
            Marker::SqliteFiles => sqlite,
            Marker::CacheLike => {
                is_cache_name(&acc.dir_name)
                    || share_at_least(cache_bytes, total, 50)
                    || share_above(acc.cache_ext_files, files, 60)
            }
            Marker::ConfigLike => {
                share_at_least(acc.config_files, files, 60) && total <= CONFIG_MAX_BYTES
            }
            Marker::MediaHeavy => share_above(acc.media_bytes, total, 70),
            Marker::DocumentHeavy => share_above(acc.doc_files, files, 50),
            Marker::ProjectLike => {
                acc.project_file
                    || acc
                        .unity_project
                        .values()
                        .any(|&(settings, assets)| settings && assets)
            }
            Marker::CloudSynced | Marker::UwpPackage => path.contains(&m),
        }
    };
    Marker::ALL.into_iter().filter(|&m| on(m)).collect()
}
