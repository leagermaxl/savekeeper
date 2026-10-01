//! `Finding`, its target, category and application (SPEC-02 §2.1–§2.4, §2.7).

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use specta::Type;

use super::{Evidence, Score, Sensitivity, TargetStats};
use crate::template::PathTemplate;

/// Something worth saving, with the evidence why (SPEC-02 §2.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub struct Finding {
    /// Stable between scans and machines (§2.7).
    pub id: FindingId,
    /// What is saved.
    pub target: Target,
    /// Kind of data.
    pub category: Category,
    /// Application or game the finding belongs to.
    pub app: Option<AppRef>,
    /// Display title, e.g. "Elden Ring — saves".
    pub title: String,
    /// Why this is a finding; at least one entry (principle P3).
    pub evidence: Vec<Evidence>,
    /// Filled in the Measure phase (SPEC-03).
    pub stats: Option<TargetStats>,
    /// How sensitive the data is.
    pub sensitivity: Sensitivity,
    /// Filled in the Score phase (SPEC-09).
    pub score: Option<Score>,
    /// Selected for backup by default (SPEC-09).
    pub default_selected: bool,
    /// Backup needs administrator rights.
    pub requires_elevation: bool,
    /// Free-form labels: "steam", "unity", "cloud-synced".
    pub tags: Vec<String>,
    /// Nested findings absorbed during merging (SPEC-09).
    pub children: Vec<FindingId>,
    /// i18n key of a hint for the user (SPEC-04).
    pub notes_key: Option<String>,
}

/// Stable finding identifier: the first 16 hex characters of a BLAKE3 hash
/// of the canonical target key (SPEC-02 §2.7). Computed in T-02-06.
///
/// Serialized as a plain string.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Type)]
#[serde(transparent)]
pub struct FindingId(String);

impl FindingId {
    /// The identifier string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What exactly is saved (SPEC-02 §2.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    /// A set of files under a root.
    FileSet {
        /// Root as a template.
        root: PathTemplate,
        /// Absolute root on the current machine.
        #[serde(with = "crate::serde_util::lossy_path")]
        #[specta(type = String)]
        resolved: PathBuf,
        /// Globs relative to `root`; empty means `**`.
        include: Vec<String>,
        /// Globs relative to `root`.
        exclude: Vec<String>,
    },
    /// A single file (common for configs).
    File {
        /// File path as a template.
        path: PathTemplate,
        /// Absolute path on the current machine.
        #[serde(with = "crate::serde_util::lossy_path")]
        #[specta(type = String)]
        resolved: PathBuf,
    },
    /// A registry key, exported to `.reg`.
    Registry {
        /// Registry hive.
        hive: RegHive,
        /// Key path inside the hive.
        key: String,
        /// Include subkeys.
        recursive: bool,
    },
    /// Output of a system exporter (winget export, netsh ...), SPEC-06.
    SystemExport {
        /// Exporter identifier.
        exporter_id: String,
        /// Exporter-specific parameters.
        params: serde_json::Value,
    },
}

/// Registry hive.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Type,
)]
#[serde(rename_all = "snake_case")]
pub enum RegHive {
    /// `HKEY_CURRENT_USER`.
    Hkcu,
    /// `HKEY_LOCAL_MACHINE`; read only, for reference (Uninstall keys etc.).
    Hklm,
}

/// Kind of data (SPEC-02 §2.3). Declaration order is the display order.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Type,
)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// Game saves.
    GameSave,
    /// Game settings, configs and mods.
    GameConfig,
    /// Application settings (small and important).
    AppConfig,
    /// Application data: databases, profiles, local libraries.
    AppData,
    /// User documents, media and projects outside standard locations.
    UserFiles,
    /// SSH, git, IDEs, repositories, WSL.
    DevEnvironment,
    /// Keys, certificates, password managers (sensitivity is always at least High).
    Credentials,
    /// winget, Wi-Fi, drivers, fonts, hosts ...
    SystemSettings,
    /// Restored by reinstalling or downloading.
    Reinstallable,
    /// Caches, temporary files, logs.
    Cache,
    /// Not recognized; a candidate for the LLM (SPEC-08).
    Unknown,
}

/// Application or game a finding belongs to (SPEC-02 §2.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub struct AppRef {
    /// Normalized slug: "elden-ring", "vscode", "obs-studio".
    pub id: String,
    /// Display name: "ELDEN RING".
    pub name: String,
    /// Kind of application.
    pub kind: AppKind,
    /// Identifiers in external sources: `{"steam": "1245620", "winget": "..."}`.
    pub source_ids: BTreeMap<String, String>,
    /// Whether it is installed now (Uninstall keys or launchers).
    pub installed: Option<bool>,
    /// Lowercase executable names, e.g. "eldenring.exe" (SPEC-10, SPEC-13).
    pub process_names: Vec<String>,
}

/// Kind of application.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Type,
)]
#[serde(rename_all = "snake_case")]
pub enum AppKind {
    /// A game.
    Game,
    /// A regular application.
    Application,
    /// Part of the operating system.
    System,
    /// A developer tool.
    DevTool,
}
