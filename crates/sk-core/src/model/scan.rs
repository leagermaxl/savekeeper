//! Scan options shared by the config and `ScanReport` (SPEC-02 §6).

use serde::{Deserialize, Serialize};
use specta::Type;

/// Collectors switched on or off by their id (SPEC-01 §4.3). All on by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub struct CollectorToggles {
    /// Known-location rules (SPEC-04).
    pub rules: bool,
    /// Game saves (SPEC-05).
    pub games: bool,
    /// System exports (SPEC-06).
    pub system: bool,
    /// Heuristics (SPEC-07).
    pub heuristics: bool,
}

impl Default for CollectorToggles {
    fn default() -> Self {
        Self {
            rules: true,
            games: true,
            system: true,
            heuristics: true,
        }
    }
}

/// LLM classification mode: `llm.mode` in the config (SPEC-01 §4.8.2) and
/// `ScanOptions.llm` (SPEC-08 FR-08-01).
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Type,
)]
#[serde(rename_all = "snake_case")]
pub enum LlmMode {
    /// No LLM, no network requests.
    #[default]
    Off,
    /// Local model (Ollama, LM Studio ...).
    Local,
    /// Cloud model.
    Cloud,
}
