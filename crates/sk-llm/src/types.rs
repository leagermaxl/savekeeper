//! Data types of the classifier API (SPEC-08 §4.1, §4.9).

use serde::{Deserialize, Serialize};
use sk_core::model::{Category, FolderSummary};

/// One folder to classify.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ClassifyItem {
    /// `FindingId` (SPEC-02 §2.7); links the answer to the finding.
    pub id: String,
    /// Folder summary, already redacted (`sk_core::privacy::redact`).
    pub summary: FolderSummary,
    /// Heads of small text files; empty unless content peek is allowed (§4.9).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub peek: Vec<ContentPeek>,
    /// Hint from the heuristics: "unity-like", "electron" ...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// The beginning of a small text file inside the folder, secrets scrubbed (§4.9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ContentPeek {
    /// File name relative to the folder.
    pub file: String,
    /// First bytes of the file after scrubbing.
    pub head: String,
}

/// The model's answer for one item, after validation (§4.6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Classification {
    /// Id of the [`ClassifyItem`].
    pub id: String,
    /// Category (SPEC-02 §2.3).
    pub category: Category,
    /// Most likely application or game: "OBS Studio".
    pub app_guess: Option<String>,
    /// How painful losing the folder would be, `0..=1`.
    pub importance: f32,
    /// How sure the model is about the category, `0..=1`.
    pub confidence: f32,
    /// Short explanation, at most 200 characters.
    pub reason: String,
    /// Globs relative to the folder to back up (empty = everything).
    pub suggested_include: Vec<String>,
    /// Globs relative to the folder to skip.
    pub suggested_exclude: Vec<String>,
    /// The answer came from the cache (§4.7), not from the provider.
    pub from_cache: bool,
}

/// Provider, model and batching limits of a classifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ClassifierInfo {
    /// `"ollama" | "openai_compat" | "anthropic" | "mock"`.
    pub provider: &'static str,
    /// Model id.
    pub model: String,
    /// Runs on the user's machine (no data leaves it).
    pub is_local: bool,
    /// Maximum items per [`Classifier::classify`](crate::Classifier::classify) call.
    pub max_batch: usize,
    /// Maximum parallel requests.
    pub max_concurrency: usize,
}

/// Result of a successful health check (FR-08-08). An unreachable provider
/// is reported as [`LlmError::Unreachable`](crate::LlmError::Unreachable).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct HealthReport {
    /// The configured model is available on the provider.
    pub model_available: bool,
    /// Response time of the check, in milliseconds.
    pub latency_ms: u64,
    /// Server version, if the provider reports one (Ollama `/api/version`).
    pub server_version: Option<String>,
}
