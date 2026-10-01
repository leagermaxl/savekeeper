//! `llm` section; fields and defaults are normative in SPEC-08 §4.4.

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::model::LlmMode;

/// `llm` section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "snake_case")]
pub struct LlmConfig {
    /// Off by default: no network requests at all.
    pub mode: LlmMode,
    /// Local model server.
    pub local: LocalLlmConfig,
    /// Cloud provider.
    pub cloud: CloudLlmConfig,
    /// Maximum folders classified per scan.
    pub max_items_per_scan: u32,
    /// Let a local model see file contents.
    pub allow_content_peek: bool,
    /// Let a cloud model see file contents (separate consent).
    pub allow_content_peek_cloud: bool,
    /// Ask before every scan that uses the cloud.
    pub confirm_cloud_each_scan: bool,
    /// Minimum confidence to apply a classification.
    pub min_confidence_to_apply: f32,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            mode: LlmMode::Off,
            local: LocalLlmConfig::default(),
            cloud: CloudLlmConfig::default(),
            max_items_per_scan: 300,
            allow_content_peek: false,
            allow_content_peek_cloud: false,
            confirm_cloud_each_scan: true,
            min_confidence_to_apply: 0.5,
        }
    }
}

/// Local model server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "snake_case")]
pub struct LocalLlmConfig {
    /// API flavor.
    pub kind: LocalLlmKind,
    /// Server URL.
    pub endpoint: String,
    /// Model name.
    pub model: String,
    /// Request timeout.
    pub timeout_s: u32,
    /// Folders per request.
    pub max_batch: u32,
    /// Parallel requests.
    pub max_concurrency: u32,
}

impl Default for LocalLlmConfig {
    fn default() -> Self {
        Self {
            kind: LocalLlmKind::Ollama,
            endpoint: "http://127.0.0.1:11434".to_owned(),
            model: "qwen2.5:7b-instruct".to_owned(),
            timeout_s: 120,
            max_batch: 8,
            max_concurrency: 1,
        }
    }
}

/// API flavor of a local server.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum LocalLlmKind {
    /// Ollama.
    #[default]
    Ollama,
    /// OpenAI-compatible (LM Studio, llama.cpp server ...).
    OpenaiCompat,
}

/// Cloud provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "snake_case")]
pub struct CloudLlmConfig {
    /// Provider API.
    pub kind: CloudLlmKind,
    /// Model name.
    pub model: String,
    /// Where the API key is kept; never in this file.
    pub api_key_source: ApiKeySource,
    /// Environment variable with the key, for `api_key_source = env`.
    pub api_key_env: String,
    /// Request timeout.
    pub timeout_s: u32,
    /// Folders per request.
    pub max_batch: u32,
    /// Parallel requests.
    pub max_concurrency: u32,
}

impl Default for CloudLlmConfig {
    fn default() -> Self {
        Self {
            kind: CloudLlmKind::Anthropic,
            model: "claude-haiku-4-5".to_owned(),
            api_key_source: ApiKeySource::CredentialManager,
            api_key_env: "ANTHROPIC_API_KEY".to_owned(),
            timeout_s: 60,
            max_batch: 20,
            max_concurrency: 4,
        }
    }
}

/// Cloud provider API.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum CloudLlmKind {
    /// Anthropic Messages API.
    #[default]
    Anthropic,
    /// OpenAI-compatible API with an external URL.
    OpenaiCompat,
}

/// Where the cloud API key is kept.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ApiKeySource {
    /// Windows Credential Manager.
    #[default]
    CredentialManager,
    /// An environment variable.
    Env,
}
