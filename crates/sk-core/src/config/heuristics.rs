//! `heuristics` section; fields and defaults are normative in SPEC-07 §4.9.

use serde::{Deserialize, Serialize};
use specta::Type;

/// `heuristics` section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "snake_case")]
pub struct HeuristicsConfig {
    /// Each heuristic on or off.
    pub enabled: HeuristicToggles,
    /// Below this confidence a folder becomes `Unknown`.
    pub unknown_threshold: f32,
    /// H-USR: user files outside standard folders.
    pub usr: UsrHeuristicConfig,
    /// H-GIT: git repositories.
    pub git: GitHeuristicConfig,
    /// Thresholds of the classification rules.
    pub thresholds: HeuristicThresholds,
}

impl Default for HeuristicsConfig {
    fn default() -> Self {
        Self {
            enabled: HeuristicToggles::default(),
            unknown_threshold: 0.6,
            usr: UsrHeuristicConfig::default(),
            git: GitHeuristicConfig::default(),
            thresholds: HeuristicThresholds::default(),
        }
    }
}

/// Heuristics switched on or off. All on by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "snake_case")]
pub struct HeuristicToggles {
    /// H-UNK: unknown folders in AppData and similar zones.
    pub unk: bool,
    /// H-USR: user files.
    pub usr: bool,
    /// H-GIT: git repositories.
    pub git: bool,
    /// H-JUNK: caches and temporary data.
    pub junk: bool,
    /// H-WEB: browser and Electron profiles.
    pub web: bool,
}

impl Default for HeuristicToggles {
    fn default() -> Self {
        Self {
            unk: true,
            usr: true,
            git: true,
            junk: true,
            web: true,
        }
    }
}

/// H-USR parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "snake_case")]
pub struct UsrHeuristicConfig {
    /// Minimum weight of a candidate folder.
    pub min_weight: u32,
    /// Maximum number of candidates.
    pub max_candidates: u32,
    /// Look on every fixed drive, not only the system one.
    pub scan_all_fixed_drives: bool,
}

impl Default for UsrHeuristicConfig {
    fn default() -> Self {
        Self {
            min_weight: 20,
            max_candidates: 200,
            scan_all_fixed_drives: true,
        }
    }
}

/// H-GIT parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "snake_case")]
pub struct GitHeuristicConfig {
    /// Search depth.
    pub max_depth: u32,
    /// Maximum number of repositories.
    pub max_repos: u32,
}

impl Default for GitHeuristicConfig {
    fn default() -> Self {
        Self {
            max_depth: 6,
            max_repos: 200,
        }
    }
}

/// Thresholds of the classification rules (SPEC-07 §4.7).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "snake_case")]
pub struct HeuristicThresholds {
    /// Share of save-like extensions.
    pub save_ext_ratio: f32,
    /// Maximum size of a config-like folder.
    pub config_max_bytes: u64,
    /// SQLite files newer than this are "recent".
    pub sqlite_recent_days: u32,
}

impl Default for HeuristicThresholds {
    fn default() -> Self {
        Self {
            save_ext_ratio: 0.2,
            config_max_bytes: 5 * 1024 * 1024,
            sqlite_recent_days: 90,
        }
    }
}
