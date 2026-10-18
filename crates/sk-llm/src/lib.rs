//! LLM classifier abstraction, providers, prompt, privacy and cache (SPEC-08).
//!
//! The crate classifies folders that rules and heuristics left as
//! [`Category::Unknown`](sk_core::model::Category::Unknown). It is optional
//! (principle P4): with `llm.mode = off` it makes no network requests.

mod anthropic;
mod apply;
mod cache;
mod classifier;
mod detect;
mod error;
mod http;
#[cfg(any(test, feature = "mock"))]
pub mod mock;
mod ollama;
mod openai_compat;
pub mod privacy;
mod prompt;
mod request;
mod types;
mod usage;
pub mod validate;

pub use anthropic::{
    AnthropicClassifier, ANTHROPIC_ENDPOINT, ANTHROPIC_MAX_TOKENS, ANTHROPIC_PROVIDER,
    ANTHROPIC_VERSION,
};
pub use apply::{apply_classification, LLM_EVIDENCE_KEY};
pub use cache::{
    cache_key, LlmCache, CACHE_FILE, CACHE_TTL_DAYS, COMPACT_MAX_LINES, COMPACT_STALE_RATIO,
};
pub use classifier::Classifier;
pub use detect::{
    detect_local_providers, detect_local_providers_at, DetectedProvider, DETECT_TIMEOUT,
    LOCAL_CANDIDATES,
};
pub use error::LlmError;
pub use ollama::{OllamaClassifier, HEALTH_TIMEOUT, OLLAMA_PROVIDER};
pub use openai_compat::{
    OpenAiCompatClassifier, FALLBACK_SCHEMA_INTRO, JSON_SCHEMA_NAME, OPENAI_COMPAT_PROVIDER,
};
pub use privacy::{final_check, PrivacyViolation};
pub use prompt::{PROMPT_VERSION, RESPONSE_SCHEMA, SYSTEM_PROMPT};
pub use request::{
    item_for_prompt, summary_for_prompt, user_message, RequestPreview, MAX_NAME_CHARS,
    MAX_SUMMARY_CHARS, PREVIEW_SAMPLE_ITEMS,
};
pub use types::{Classification, ClassifierInfo, ClassifyItem, ContentPeek, HealthReport};
pub use usage::{
    CostEstimate, ModelPricing, UsageStats, EST_INPUT_TOKENS_PER_ITEM, EST_OUTPUT_TOKENS_PER_ITEM,
};
pub use validate::{parse_response, ParsedResponse};

/// The `llm` config section (SPEC-08 §4.4); defined in `sk-core::config`.
pub use sk_core::config::{
    ApiKeySource, CloudLlmConfig, CloudLlmKind, LlmConfig, LocalLlmConfig, LocalLlmKind,
};
/// `llm.mode` (SPEC-02 §6); defined in `sk-core::model`.
pub use sk_core::model::LlmMode;
