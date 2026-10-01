//! `savekeeper.config.json` and the data folder (SPEC-01 §4.8).
//!
//! All sections are typed here, because the UI edits the whole config and
//! `sk-core` cannot depend on feature crates; their fields and defaults are
//! normative in the feature specs, and feature crates re-export their section.

mod data_dir;
mod heuristics;
mod llm;
mod load;
mod scoring;
mod sections;

use serde::{Deserialize, Serialize};
use specta::Type;

pub use data_dir::DataDir;
pub use heuristics::{
    GitHeuristicConfig, HeuristicThresholds, HeuristicToggles, HeuristicsConfig, UsrHeuristicConfig,
};
pub use llm::{
    ApiKeySource, CloudLlmConfig, CloudLlmKind, LlmConfig, LocalLlmConfig, LocalLlmKind,
};
pub use load::{ConfigWarning, LoadedConfig};
pub use scoring::{default_weights, CategoryWeights, ScoringConfig};
pub use sections::{
    BackupConfig, BackupFormat, GamesConfig, ScanConfig, SystemConfig, Theme, UiConfig, UiLanguage,
    UpdatesConfig,
};

/// The program configuration. `Default` gives the values of SPEC-01 §4.8.2.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "snake_case")]
pub struct Config {
    /// Format version, [`Config::SCHEMA_VERSION`] after loading.
    pub schema_version: u32,
    /// Interface.
    pub ui: UiConfig,
    /// Scanning (SPEC-03).
    pub scan: ScanConfig,
    /// Games (SPEC-05).
    pub games: GamesConfig,
    /// System exports (SPEC-06).
    pub system: SystemConfig,
    /// Heuristics (SPEC-07).
    pub heuristics: HeuristicsConfig,
    /// LLM classification (SPEC-08).
    pub llm: LlmConfig,
    /// Scoring (SPEC-09).
    pub scoring: ScoringConfig,
    /// Backup (SPEC-10).
    pub backup: BackupConfig,
    /// Update checks (SPEC-14).
    pub updates: UpdatesConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: Self::SCHEMA_VERSION,
            ui: UiConfig::default(),
            scan: ScanConfig::default(),
            games: GamesConfig::default(),
            system: SystemConfig::default(),
            heuristics: HeuristicsConfig::default(),
            llm: LlmConfig::default(),
            scoring: ScoringConfig::default(),
            backup: BackupConfig::default(),
            updates: UpdatesConfig::default(),
        }
    }
}

impl Config {
    /// Current format version.
    pub const SCHEMA_VERSION: u32 = 1;
}

/// Failure to write the config or to find a data folder.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// File system error.
    #[error("config I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// The config could not be serialized.
    #[error("config serialization error: {0}")]
    Json(#[from] serde_json::Error),
    /// Neither the program folder nor `%LOCALAPPDATA%` is writable.
    #[error("no writable data folder")]
    NoDataDir,
}
