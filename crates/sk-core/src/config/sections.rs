//! Small config sections: ui, scan, games, system, backup, updates (SPEC-01 §4.8.2).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use specta::Type;

/// `ui` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "snake_case")]
pub struct UiConfig {
    /// Interface language.
    pub language: UiLanguage,
    /// Color theme.
    pub theme: Theme,
}

/// Interface language.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum UiLanguage {
    /// From the Windows UI language.
    #[default]
    Auto,
    /// Russian.
    Ru,
    /// English.
    En,
}

/// Color theme.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    /// Follow the system setting.
    #[default]
    System,
    /// Light.
    Light,
    /// Dark.
    Dark,
}

/// `scan` section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "snake_case")]
pub struct ScanConfig {
    /// Additional roots for heuristics.
    #[serde(with = "crate::serde_util::lossy_path_vec")]
    #[specta(type = Vec<String>)]
    pub extra_roots: Vec<PathBuf>,
    /// Added to the built-in exclusions (SPEC-03).
    pub exclude_globs: Vec<String>,
    /// Follow symbolic links (always false in the MVP).
    pub follow_symlinks: bool,
    /// Maximum traversal depth.
    pub max_depth: u32,
    /// Templates ignored in "Not recognized" (SPEC-11); validated by the consumer.
    pub ignored_templates: Vec<String>,
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            extra_roots: Vec::new(),
            exclude_globs: Vec::new(),
            follow_symlinks: false,
            max_depth: 32,
            ignored_templates: Vec::new(),
        }
    }
}

/// `games` section (SPEC-05).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "snake_case")]
pub struct GamesConfig {
    /// Ludusavi manifest URL.
    pub manifest_url: String,
    /// Update the manifest automatically.
    pub auto_update: bool,
    /// Minimum time between manifest updates.
    pub update_interval_hours: u32,
}

impl Default for GamesConfig {
    fn default() -> Self {
        Self {
            manifest_url: "https://raw.githubusercontent.com/mtkennerly/ludusavi-manifest/master/data/manifest.yaml"
                .to_owned(),
            auto_update: true,
            update_interval_hours: 168,
        }
    }
}

/// `system` section (SPEC-06).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "snake_case")]
pub struct SystemConfig {
    /// Exporter ids that are switched off.
    pub disabled_exporters: Vec<String>,
    /// Timeout of `winget export`.
    pub winget_timeout_s: u32,
}

impl Default for SystemConfig {
    fn default() -> Self {
        Self {
            disabled_exporters: Vec::new(),
            winget_timeout_s: 180,
        }
    }
}

/// `backup` section (SPEC-10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "snake_case")]
pub struct BackupConfig {
    /// Container format.
    pub format: BackupFormat,
    /// Zip compression level, `0..=9`.
    pub compression_level: u8,
    /// Encrypt the archive.
    pub encrypt: bool,
    /// Read every entry back after writing.
    pub verify: bool,
}

impl Default for BackupConfig {
    fn default() -> Self {
        Self {
            format: BackupFormat::Zip,
            compression_level: 6,
            encrypt: false,
            verify: true,
        }
    }
}

/// Backup container format; also used by SPEC-10.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum BackupFormat {
    /// A zip archive.
    #[default]
    Zip,
    /// A plain folder.
    Dir,
}

/// `updates` section (SPEC-14 FR-14-07).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "snake_case")]
pub struct UpdatesConfig {
    /// Check for new releases.
    pub check: bool,
    /// Days between checks.
    pub interval_days: u32,
}

impl Default for UpdatesConfig {
    fn default() -> Self {
        Self {
            check: false,
            interval_days: 7,
        }
    }
}
