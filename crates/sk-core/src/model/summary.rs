//! `FolderSummary`: a compact, anonymized description of a folder for
//! heuristics (SPEC-07) and the LLM (SPEC-08). Computed by SPEC-03 (SPEC-02 §4).

use serde::{Deserialize, Serialize};
use specta::Type;
use time::OffsetDateTime;

use crate::template::PathTemplate;

/// Summary of a folder. Never contains absolute paths or the user name (SPEC-02 §4.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub struct FolderSummary {
    /// Anonymized path.
    pub path: PathTemplate,
    /// Size of all files.
    pub total_bytes: u64,
    /// Number of files.
    pub file_count: u64,
    /// Number of subfolders.
    pub dir_count: u64,
    /// Deepest nesting level below the folder.
    pub max_depth: u32,
    /// Newest modification time.
    #[serde(default, with = "time::serde::rfc3339::option")]
    #[specta(type = Option<String>)]
    pub newest_mtime: Option<OffsetDateTime>,
    /// Oldest modification time.
    #[serde(default, with = "time::serde::rfc3339::option")]
    #[specta(type = Option<String>)]
    pub oldest_mtime: Option<OffsetDateTime>,
    /// Top 15 extensions by file count.
    pub ext_histogram: Vec<ExtStat>,
    /// Up to 20 relative paths: top level first, then the newest, then
    /// different extensions; redacted (`sk-core::privacy::redact`).
    pub sample_names: Vec<String>,
    /// Up to 10 largest subfolders.
    pub top_children: Vec<ChildStat>,
    /// Quick signs of what the folder is (§4.1).
    pub markers: Vec<Marker>,
    /// The traversal hit a limit.
    pub truncated: bool,
}

/// Files with one extension.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub struct ExtStat {
    /// Lowercase extension without the dot; `""` for files without one.
    pub ext: String,
    /// Number of files.
    pub count: u64,
    /// Their total size.
    pub bytes: u64,
}

/// A direct subfolder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub struct ChildStat {
    /// Folder name (redacted).
    pub name: String,
    /// Size of all files inside.
    pub bytes: u64,
    /// Number of files inside.
    pub files: u64,
}

/// Quick signs of a folder. The rules are in SPEC-03; the list changes only
/// together with SPEC-02 §4.1.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Type,
)]
#[serde(rename_all = "snake_case")]
pub enum Marker {
    /// `.exe`/`.dll`/`.sys` make up more than 30% of bytes.
    HasExecutables,
    /// Contains `.git`.
    GitRepo,
    /// `LocalLow\<Company>\<Product>` layout, `Player.log` / `output_log.txt`.
    UnityGame,
    /// `Saved\SaveGames`.
    UnrealSaveGames,
    /// `Local Storage`, `IndexedDB`, `Cache`, `GPUCache`, `Code Cache`.
    ElectronApp,
    /// `Default\Preferences`, `Local State`.
    ChromiumProfile,
    /// `*.sqlite`, `*.db` with the "SQLite format 3" signature.
    SqliteFiles,
    /// Name with cache/temp/tmp/logs/crash, or mostly `.tmp`/`.log`.
    CacheLike,
    /// Mostly `.json`/`.ini`/`.xml`/`.cfg`/`.toml`/`.yaml`.
    ConfigLike,
    /// Images/video/audio make up more than 70% of bytes.
    MediaHeavy,
    /// Office/pdf/txt/md make up more than 50% of files.
    DocumentHeavy,
    /// `package.json`, `Cargo.toml`, `*.sln`, `*.csproj`, `pyproject.toml`, `*.uproject`, `*.blend`.
    ProjectLike,
    /// Inside one of `Environment.cloud_roots`.
    CloudSynced,
    /// `{LOCALAPPDATA}\Packages\<PFN>` (MS Store / UWP).
    UwpPackage,
}

impl Marker {
    /// All markers, in declaration order.
    pub const ALL: [Marker; 14] = [
        Marker::HasExecutables,
        Marker::GitRepo,
        Marker::UnityGame,
        Marker::UnrealSaveGames,
        Marker::ElectronApp,
        Marker::ChromiumProfile,
        Marker::SqliteFiles,
        Marker::CacheLike,
        Marker::ConfigLike,
        Marker::MediaHeavy,
        Marker::DocumentHeavy,
        Marker::ProjectLike,
        Marker::CloudSynced,
        Marker::UwpPackage,
    ];
}
