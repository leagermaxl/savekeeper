//! LLM classifier abstraction, providers, prompt, privacy and cache (SPEC-08).
//!
//! The crate classifies folders that rules and heuristics left as
//! [`Category::Unknown`](sk_core::model::Category::Unknown). It is optional
//! (principle P4): with `llm.mode = off` it makes no network requests.

mod classifier;
mod error;
#[cfg(any(test, feature = "mock"))]
pub mod mock;
mod types;

pub use classifier::Classifier;
pub use error::LlmError;
pub use types::{Classification, ClassifierInfo, ClassifyItem, ContentPeek, HealthReport};

/// The `llm` config section (SPEC-08 §4.4); defined in `sk-core::config`.
pub use sk_core::config::{
    ApiKeySource, CloudLlmConfig, CloudLlmKind, LlmConfig, LocalLlmConfig, LocalLlmKind,
};
/// `llm.mode` (SPEC-02 §6); defined in `sk-core::model`.
pub use sk_core::model::LlmMode;
