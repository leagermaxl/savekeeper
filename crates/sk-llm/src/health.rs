//! «Check connection» of the settings screen (SPEC-08 FR-08-08, §4.11;
//! SPEC-11 `llm_health_check`): the health of the configured provider as one
//! value the UI can show without interpreting [`LlmError`].

use serde::Serialize;

use crate::recommended::ollama_pull_command;
use crate::runner::setup;
use crate::{Classifier, LlmConfig, LlmError, LlmMode, SecretStore, OLLAMA_PROVIDER};

/// Outcome of [`llm_health`]; the UI text is `llm.health.<status>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmHealthStatus {
    /// `mode = off`: nothing was checked, no request was made.
    Off,
    /// The provider answered and the model is available.
    Ok,
    /// The provider answered but the model is not installed / not found
    /// ([`LlmHealth::pull_command`] for Ollama).
    ModelMissing,
    /// No connection, timeout, invalid endpoint or not a server of this kind.
    Unreachable,
    /// `mode = cloud` without a stored API key (issue `llm.no_api_key`).
    NoApiKey,
    /// The provider rejected the key (401/403).
    AuthFailed,
    /// Any other failure (429, 5xx, bad answer); see [`LlmHealth::error`].
    Error,
}

/// Health of the provider of one `llm.mode`, for the settings screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct LlmHealth {
    /// The checked mode.
    pub mode: LlmMode,
    /// Outcome.
    pub status: LlmHealthStatus,
    /// `"ollama" | "openai_compat" | "anthropic" | "mock"`; `None` when no
    /// classifier was built (`off`, no key, invalid config).
    pub provider: Option<&'static str>,
    /// Model id; `None` when no classifier was built.
    pub model: Option<String>,
    /// Data stays on this machine; `None` when no classifier was built.
    pub is_local: Option<bool>,
    /// Duration of the check, when the provider answered.
    pub latency_ms: Option<u64>,
    /// Server version (Ollama `/api/version`).
    pub server_version: Option<String>,
    /// `ollama pull <model>` for [`LlmHealthStatus::ModelMissing`] on Ollama.
    pub pull_command: Option<String>,
    /// Error message for `unreachable`, `auth_failed` and `error`; never holds
    /// a URL or a key.
    pub error: Option<String>,
}

impl LlmHealth {
    fn empty(mode: LlmMode, status: LlmHealthStatus) -> Self {
        Self {
            mode,
            status,
            provider: None,
            model: None,
            is_local: None,
            latency_ms: None,
            server_version: None,
            pull_command: None,
            error: None,
        }
    }
}

/// Checks the provider of `mode` with the settings of `cfg` (its own
/// `cfg.mode` is ignored, so the UI can check a mode before switching to it).
///
/// Builds the classifier as [`ClassifyRunner::from_config`](crate::ClassifyRunner::from_config)
/// does (the cloud key from `secrets`) and calls [`Classifier::health`] once:
/// timeouts are `min(timeout_s, HEALTH_TIMEOUT)` per request, no retries.
/// `mode = off` makes no request and creates no client. Never fails: every
/// outcome is a [`LlmHealthStatus`].
pub async fn llm_health(cfg: &LlmConfig, mode: LlmMode, secrets: &dyn SecretStore) -> LlmHealth {
    let mut cfg = cfg.clone();
    cfg.mode = mode;
    match setup::classifier(&cfg, secrets) {
        Ok(None) => LlmHealth::empty(mode, LlmHealthStatus::Off),
        Ok(Some(classifier)) => classifier_health(mode, classifier.as_ref()).await,
        Err(LlmError::Auth) => LlmHealth::empty(mode, LlmHealthStatus::NoApiKey),
        Err(e) => {
            let mut health = LlmHealth::empty(mode, status_of(&e));
            health.error = Some(e.to_string());
            health
        }
    }
}

/// [`Classifier::health`] of `classifier` as an [`LlmHealth`] for `mode`.
pub async fn classifier_health(mode: LlmMode, classifier: &dyn Classifier) -> LlmHealth {
    let info = classifier.info();
    let result = classifier.health().await;
    let mut health = LlmHealth {
        provider: Some(info.provider),
        model: Some(info.model.clone()),
        is_local: Some(info.is_local),
        ..LlmHealth::empty(mode, LlmHealthStatus::Ok)
    };
    match result {
        Ok(report) => {
            health.latency_ms = Some(report.latency_ms);
            health.server_version = report.server_version;
            if !report.model_available {
                health.status = LlmHealthStatus::ModelMissing;
            }
        }
        Err(e) => {
            health.status = status_of(&e);
            if health.status != LlmHealthStatus::ModelMissing {
                health.error = Some(e.to_string());
            }
        }
    }
    if health.status == LlmHealthStatus::ModelMissing && info.provider == OLLAMA_PROVIDER {
        health.pull_command = Some(ollama_pull_command(&info.model));
    }
    tracing::debug!(
        provider = info.provider,
        model = %info.model,
        status = ?health.status,
        "llm health check"
    );
    health
}

/// Same mapping as the health check of `ClassifyRunner::run` (§4.12).
fn status_of(e: &LlmError) -> LlmHealthStatus {
    match e {
        LlmError::ModelNotFound(_) => LlmHealthStatus::ModelMissing,
        LlmError::Unreachable(_) | LlmError::Timeout => LlmHealthStatus::Unreachable,
        LlmError::Auth => LlmHealthStatus::AuthFailed,
        _ => LlmHealthStatus::Error,
    }
}

#[cfg(test)]
#[path = "health_tests.rs"]
mod tests;
