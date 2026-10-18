//! LLM classifier abstraction, providers, prompt, privacy and cache (SPEC-08).
//!
//! The crate classifies folders that rules and heuristics left as
//! [`Category::Unknown`](sk_core::model::Category::Unknown). It is optional
//! (principle P4): with `llm.mode = off` it makes no network requests.

mod apply;
mod classifier;
mod error;
#[cfg(any(test, feature = "mock"))]
pub mod mock;
mod prompt;
mod types;
pub mod validate;

pub use apply::{apply_classification, LLM_EVIDENCE_KEY};
pub use classifier::Classifier;
pub use error::LlmError;
pub use prompt::{PROMPT_VERSION, RESPONSE_SCHEMA, SYSTEM_PROMPT};
pub use types::{Classification, ClassifierInfo, ClassifyItem, ContentPeek, HealthReport};
pub use validate::{parse_response, ParsedResponse};

/// The `llm` config section (SPEC-08 §4.4); defined in `sk-core::config`.
pub use sk_core::config::{
    ApiKeySource, CloudLlmConfig, CloudLlmKind, LlmConfig, LocalLlmConfig, LocalLlmKind,
};
/// `llm.mode` (SPEC-02 §6); defined in `sk-core::model`.
pub use sk_core::model::LlmMode;
