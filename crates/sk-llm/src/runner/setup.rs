//! The classifier of the configured `llm.mode` (SPEC-08 §4.2, §4.4, §4.10).

use std::sync::Arc;

use crate::{
    AnthropicClassifier, Classifier, CloudLlmKind, ExposeSecret, LlmConfig, LlmError, LlmMode,
    LocalLlmKind, OllamaClassifier, OpenAiCompatClassifier, SecretStore, API_KEY_NAME,
};

/// `None` for `mode = off`: nothing is built, no client exists (FR-08-01).
pub(super) fn classifier(
    cfg: &LlmConfig,
    secrets: &dyn SecretStore,
) -> Result<Option<Arc<dyn Classifier>>, LlmError> {
    let classifier: Arc<dyn Classifier> = match cfg.mode {
        LlmMode::Off => return Ok(None),
        LlmMode::Local => match cfg.local.kind {
            LocalLlmKind::Ollama => Arc::new(OllamaClassifier::new(&cfg.local)?),
            LocalLlmKind::OpenaiCompat => Arc::new(OpenAiCompatClassifier::new(&cfg.local)?),
        },
        LlmMode::Cloud => match cfg.cloud.kind {
            CloudLlmKind::Anthropic => {
                let key = secrets.get(API_KEY_NAME).ok_or(LlmError::Auth)?;
                Arc::new(AnthropicClassifier::new(
                    &cfg.cloud,
                    key.expose_secret().to_owned(),
                )?)
            }
            CloudLlmKind::OpenaiCompat => {
                return Err(LlmError::Unreachable(
                    "cloud openai_compat has no endpoint: configure it in llm.local".to_owned(),
                ))
            }
        },
    };
    Ok(Some(classifier))
}
