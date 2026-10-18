//! Anthropic provider, Messages API (SPEC-08 §4.2.3).

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use reqwest::header::{HeaderValue, CONTENT_TYPE};
use reqwest::{RequestBuilder, StatusCode, Url};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use serde_json::{json, Value};
use sk_core::config::CloudLlmConfig;
use sk_core::CancellationToken;

use crate::http::{self, parse_json, usize_of};
use crate::{
    parse_response, user_message, Classification, Classifier, ClassifierInfo, ClassifyItem,
    HealthReport, LlmError, ModelPricing, UsageStats, HEALTH_TIMEOUT, RESPONSE_SCHEMA,
    SYSTEM_PROMPT,
};

/// [`ClassifierInfo::provider`] of [`AnthropicClassifier`].
pub const ANTHROPIC_PROVIDER: &str = "anthropic";

/// Base URL of the Anthropic API.
pub const ANTHROPIC_ENDPOINT: &str = "https://api.anthropic.com";

/// Value of the `anthropic-version` header.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// `max_tokens` of a classification request.
pub const ANTHROPIC_MAX_TOKENS: u32 = 4096;

/// Header carrying the API key.
const API_KEY_HEADER: &str = "x-api-key";

/// Classifier backed by the Anthropic Messages API: `POST /v1/messages`
/// with structured outputs (`output_config.format`) and a cached system prompt.
///
/// Token usage of all requests (also of clones) is accumulated and reported by
/// [`Classifier::usage`].
#[derive(Debug, Clone)]
pub struct AnthropicClassifier {
    client: reqwest::Client,
    endpoint: Url,
    model: String,
    /// Never shown by `Debug` (NFR-08-04); exposed only to build the header.
    api_key: SecretString,
    timeout: Duration,
    max_batch: usize,
    max_concurrency: usize,
    usage: Arc<Mutex<UsageStats>>,
}

impl AnthropicClassifier {
    /// A classifier for the `llm.cloud` config section with the API key `api_key`.
    ///
    /// Uses `model`, `timeout_s` (per request; at least 1 s), `max_batch` and
    /// `max_concurrency` (each at least 1); `kind` and the key source are not
    /// checked. Talks to [`ANTHROPIC_ENDPOINT`]. Makes no request. A blank key
    /// or one that cannot be sent in a header is [`LlmError::Auth`].
    pub fn new(cfg: &CloudLlmConfig, api_key: impl Into<String>) -> Result<Self, LlmError> {
        let raw = SecretString::from(api_key.into());
        let key = raw.expose_secret().trim();
        if key.is_empty() || HeaderValue::from_str(key).is_err() {
            return Err(LlmError::Auth);
        }
        let key = SecretString::from(key);
        let endpoint = http::parse_endpoint(ANTHROPIC_ENDPOINT)?;
        let client = http::client(&endpoint)?;
        Ok(Self {
            client,
            endpoint,
            model: cfg.model.trim().to_owned(),
            api_key: key,
            timeout: Duration::from_secs(u64::from(cfg.timeout_s.max(1))),
            max_batch: usize_of(cfg.max_batch),
            max_concurrency: usize_of(cfg.max_concurrency),
            usage: Arc::new(Mutex::new(UsageStats::default())),
        })
    }

    /// Talks to `endpoint` instead of [`ANTHROPIC_ENDPOINT`] (tests, gateways).
    /// An invalid URL is [`LlmError::Unreachable`].
    pub fn with_endpoint(mut self, endpoint: &str) -> Result<Self, LlmError> {
        let endpoint = http::parse_endpoint(endpoint)?;
        self.client = http::client(&endpoint)?;
        self.endpoint = endpoint;
        Ok(self)
    }

    /// The `/v1/messages` body for one batch (§4.2.3).
    pub(crate) fn request_body(&self, items: &[ClassifyItem]) -> Result<Value, LlmError> {
        let schema: Value = serde_json::from_str(RESPONSE_SCHEMA)
            .map_err(|e| LlmError::BadResponse(format!("response schema: {e}")))?;
        let mut output_config = json!({ "format": { "type": "json_schema", "schema": schema } });
        if supports_effort(&self.model) {
            output_config["effort"] = json!("low");
        }
        Ok(json!({
            "model": self.model,
            "max_tokens": ANTHROPIC_MAX_TOKENS,
            "system": [{
                "type": "text",
                "text": SYSTEM_PROMPT,
                "cache_control": { "type": "ephemeral" },
            }],
            "messages": [{ "role": "user", "content": user_message(items) }],
            "output_config": output_config,
        }))
    }

    /// Adds the API key (header marked sensitive) and `anthropic-version`.
    fn authorize(&self, request: RequestBuilder) -> RequestBuilder {
        let request = request.header("anthropic-version", ANTHROPIC_VERSION);
        match HeaderValue::from_str(self.api_key.expose_secret()) {
            Ok(mut value) => {
                value.set_sensitive(true);
                request.header(API_KEY_HEADER, value)
            }
            // Checked in `new`; without a key the API answers 401 (`Auth`).
            Err(_) => request,
        }
    }

    /// Adds one response's usage to the running totals and returns it.
    fn record_usage(&self, usage: Option<&ApiUsage>) -> UsageStats {
        let mut stats = UsageStats {
            requests: 1,
            ..UsageStats::default()
        };
        if let Some(u) = usage {
            stats.input_tokens = u.input_tokens.unwrap_or(0);
            stats.output_tokens = u.output_tokens.unwrap_or(0);
            stats.cache_creation_input_tokens = u.cache_creation_input_tokens.unwrap_or(0);
            stats.cache_read_input_tokens = u.cache_read_input_tokens.unwrap_or(0);
        }
        if let Some(pricing) = ModelPricing::for_model(ANTHROPIC_PROVIDER, &self.model) {
            stats.cost_usd = pricing.cost_usd(&stats);
        }
        self.usage
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .add(&stats);
        stats
    }
}

/// Models that take `output_config.effort` (§4.2.3): Sonnet 5 gets `"low"`;
/// Haiku 4.5 rejects the parameter, so it is never sent to other models.
pub(crate) fn supports_effort(model: &str) -> bool {
    let model = model.trim().to_lowercase();
    model == "claude-sonnet-5" || model.starts_with("claude-sonnet-5-")
}

/// `POST /v1/messages`.
#[derive(Deserialize)]
struct MessagesResponse {
    #[serde(default)]
    content: Vec<ContentBlock>,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    usage: Option<ApiUsage>,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize)]
struct ApiUsage {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
    #[serde(default)]
    cache_creation_input_tokens: Option<u64>,
    #[serde(default)]
    cache_read_input_tokens: Option<u64>,
}

/// Maps `stop_reason` (§4.2.3, §5): `end_turn` is a complete answer;
/// `max_tokens` (and `model_context_window_exceeded`) is
/// [`LlmError::MaxTokens`]; `refusal` is [`LlmError::Refusal`]; anything
/// else (or none) is [`LlmError::BadResponse`].
pub(crate) fn check_stop_reason(stop_reason: Option<&str>) -> Result<(), LlmError> {
    match stop_reason {
        Some("end_turn") => Ok(()),
        Some("max_tokens" | "model_context_window_exceeded") => Err(LlmError::MaxTokens),
        Some("refusal") => Err(LlmError::Refusal),
        Some(other) => Err(LlmError::BadResponse(format!(
            "unexpected stop_reason: {}",
            other.chars().take(40).collect::<String>()
        ))),
        None => Err(LlmError::BadResponse("no stop_reason".to_owned())),
    }
}

#[async_trait]
impl Classifier for AnthropicClassifier {
    fn info(&self) -> ClassifierInfo {
        ClassifierInfo {
            provider: ANTHROPIC_PROVIDER,
            model: self.model.clone(),
            is_local: false,
            max_batch: self.max_batch,
            max_concurrency: self.max_concurrency,
        }
    }

    /// `GET /v1/models/{model}` (free, no tokens): 200 means the model is
    /// available, 404 is `Ok` with `model_available = false`, 401/403 is
    /// [`LlmError::Auth`], other statuses are mapped as in §4.12; no
    /// connection is [`LlmError::Unreachable`]. `server_version` is `None`.
    async fn health(&self) -> Result<HealthReport, LlmError> {
        let url = http::join(&self.endpoint, &format!("v1/models/{}", self.model));
        let request = self
            .client
            .get(url)
            .timeout(self.timeout.min(HEALTH_TIMEOUT));
        let started = Instant::now();
        // Health checks are not cancellable: they are short and bounded by the timeout.
        let response = http::send(self.authorize(request), &CancellationToken::new()).await?;
        let latency_ms = http::elapsed_ms(started);
        let model_available = match response.status {
            s if s.is_success() => true,
            StatusCode::NOT_FOUND => false,
            _ => return Err(http::status_error(&response)),
        };
        Ok(HealthReport {
            model_available,
            latency_ms,
            server_version: None,
        })
    }

    /// One `POST /v1/messages`. An empty batch makes no request; a batch
    /// larger than `max_batch` is [`LlmError::BadResponse`]. HTTP 404 about
    /// the model is [`LlmError::ModelNotFound`]; other statuses are mapped as
    /// in §4.12. Usage is recorded for every 2xx answer, then `stop_reason`
    /// is checked ([`check_stop_reason`]) and the first `text` block goes
    /// through [`parse_response`]: skipped ids are missing.
    async fn classify(
        &self,
        items: &[ClassifyItem],
        cancel: &CancellationToken,
    ) -> Result<Vec<Classification>, LlmError> {
        if !http::check_batch(items.len(), self.max_batch, cancel)? {
            return Ok(Vec::new());
        }
        let body = self.request_body(items)?.to_string();
        let request = self
            .client
            .post(http::join(&self.endpoint, "v1/messages"))
            .header(CONTENT_TYPE, "application/json")
            .timeout(self.timeout)
            .body(body);
        let started = Instant::now();
        let response = http::send(self.authorize(request), cancel).await?;
        let elapsed_ms = http::elapsed_ms(started);
        if http::is_model_not_found(&response) {
            return Err(LlmError::ModelNotFound(self.model.clone()));
        }
        if !response.status.is_success() {
            tracing::debug!(
                provider = ANTHROPIC_PROVIDER,
                model = %self.model,
                items = items.len(),
                status = response.status.as_u16(),
                elapsed_ms,
                "llm request failed"
            );
            return Err(http::status_error(&response));
        }
        let message: MessagesResponse = parse_json(&response.body, "invalid messages JSON")?;
        let usage = self.record_usage(message.usage.as_ref());
        tracing::debug!(
            provider = ANTHROPIC_PROVIDER,
            model = %self.model,
            items = items.len(),
            elapsed_ms,
            stop_reason = message.stop_reason.as_deref().unwrap_or(""),
            input_tokens = usage.input_tokens,
            output_tokens = usage.output_tokens,
            cache_read_input_tokens = usage.cache_read_input_tokens,
            cache_creation_input_tokens = usage.cache_creation_input_tokens,
            "llm request finished"
        );
        check_stop_reason(message.stop_reason.as_deref())?;
        let text = message
            .content
            .into_iter()
            .find(|b| b.kind == "text")
            .and_then(|b| b.text)
            .ok_or_else(|| LlmError::BadResponse("no text block in content".to_owned()))?;
        Ok(parse_response(&text, items)?.results)
    }

    fn usage(&self) -> UsageStats {
        *self.usage.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
#[path = "anthropic_tests.rs"]
mod tests;
