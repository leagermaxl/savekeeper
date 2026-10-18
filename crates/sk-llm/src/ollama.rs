//! Ollama provider (SPEC-08 §4.2.1).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use reqwest::header::CONTENT_TYPE;
use reqwest::{StatusCode, Url};
use serde::Deserialize;
use serde_json::{json, Value};
use sk_core::config::LocalLlmConfig;
use sk_core::CancellationToken;

use crate::http::{self, parse_json, usize_of, HttpResponse};
use crate::{
    parse_response, user_message, Classification, Classifier, ClassifierInfo, ClassifyItem,
    HealthReport, LlmError, RESPONSE_SCHEMA, SYSTEM_PROMPT,
};

/// [`ClassifierInfo::provider`] of [`OllamaClassifier`].
pub const OLLAMA_PROVIDER: &str = "ollama";

/// Upper bound of one health-check request: the UI waits for it ("Check connection").
pub const HEALTH_TIMEOUT: Duration = Duration::from_secs(10);

/// Context window requested from Ollama (`options.num_ctx`).
const NUM_CTX: u32 = 8192;
/// Fixed seed for reproducible answers (`options.seed`).
const SEED: u32 = 42;
/// How long Ollama keeps the model loaded between batches (`keep_alive`).
const KEEP_ALIVE: &str = "10m";

/// Classifier backed by an Ollama server: `POST {endpoint}/api/chat`
/// with the response schema in `format` (structured outputs).
///
/// Requests carry `"think": false` (reasoning off, §4.2.1). A server that
/// rejects it (HTTP 400 mentioning `think`) is asked again without the field;
/// that choice is remembered for the lifetime of the classifier (and its
/// clones), so later batches are sent without `think` right away.
#[derive(Debug, Clone)]
pub struct OllamaClassifier {
    client: reqwest::Client,
    endpoint: Url,
    model: String,
    timeout: Duration,
    max_batch: usize,
    max_concurrency: usize,
    /// The server rejected `think`: send requests without it (session flag).
    no_think: Arc<AtomicBool>,
}

impl OllamaClassifier {
    /// A classifier for the `llm.local` config section.
    ///
    /// Uses `endpoint`, `model`, `timeout_s` (per request; at least 1 s),
    /// `max_batch` and `max_concurrency` (each at least 1); `kind` is not
    /// checked. Makes no request. An invalid endpoint URL is
    /// [`LlmError::Unreachable`].
    pub fn new(cfg: &LocalLlmConfig) -> Result<Self, LlmError> {
        let endpoint = http::parse_endpoint(&cfg.endpoint)?;
        let client = http::client(&endpoint)?;
        Ok(Self {
            client,
            endpoint,
            model: cfg.model.trim().to_owned(),
            timeout: Duration::from_secs(u64::from(cfg.timeout_s.max(1))),
            max_batch: usize_of(cfg.max_batch),
            max_concurrency: usize_of(cfg.max_concurrency),
            no_think: Arc::new(AtomicBool::new(false)),
        })
    }

    /// The server rejected `"think": false` earlier in this session, so
    /// requests are sent without the field.
    pub fn uses_no_think_fallback(&self) -> bool {
        self.no_think.load(Ordering::Relaxed)
    }

    /// The `/api/chat` body for one batch (§4.2.1); `send_think` adds `"think": false`.
    pub(crate) fn request_body(
        &self,
        items: &[ClassifyItem],
        send_think: bool,
    ) -> Result<Value, LlmError> {
        let schema: Value = serde_json::from_str(RESPONSE_SCHEMA)
            .map_err(|e| LlmError::BadResponse(format!("response schema: {e}")))?;
        let mut body = json!({
            "model": self.model,
            "stream": false,
            "messages": [
                { "role": "system", "content": SYSTEM_PROMPT },
                { "role": "user", "content": user_message(items) },
            ],
            "format": schema,
            "options": { "temperature": 0, "num_ctx": NUM_CTX, "seed": SEED },
            "keep_alive": KEEP_ALIVE,
        });
        if send_think {
            body["think"] = Value::Bool(false);
        }
        Ok(body)
    }

    async fn chat(
        &self,
        items: &[ClassifyItem],
        send_think: bool,
        cancel: &CancellationToken,
    ) -> Result<HttpResponse, LlmError> {
        let body = self.request_body(items, send_think)?.to_string();
        let request = self
            .client
            .post(http::join(&self.endpoint, "api/chat"))
            .header(CONTENT_TYPE, "application/json")
            .timeout(self.timeout)
            .body(body);
        let started = Instant::now();
        let response = http::send(request, cancel).await?;
        tracing::debug!(
            provider = OLLAMA_PROVIDER,
            model = %self.model,
            items = items.len(),
            send_think,
            status = response.status.as_u16(),
            elapsed_ms = http::elapsed_ms(started),
            "llm request finished"
        );
        Ok(response)
    }

    async fn get(&self, path: &str, timeout: Duration) -> Result<HttpResponse, LlmError> {
        let request = self
            .client
            .get(http::join(&self.endpoint, path))
            .timeout(timeout);
        // Health checks are not cancellable: they are short and bounded by `timeout`.
        http::send(request, &CancellationToken::new()).await
    }
}

/// `GET /api/version`.
#[derive(Deserialize)]
struct VersionResponse {
    version: Option<String>,
}

/// `GET /api/tags`.
#[derive(Deserialize)]
pub(crate) struct TagsResponse {
    #[serde(default)]
    pub models: Vec<TagEntry>,
}

#[derive(Deserialize)]
pub(crate) struct TagEntry {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub model: String,
}

/// `POST /api/chat` with `stream: false`. Unknown fields, among them
/// `message.thinking`, are ignored.
#[derive(Deserialize)]
struct ChatResponse {
    message: Option<ChatMessage>,
}

#[derive(Deserialize)]
struct ChatMessage {
    content: String,
}

/// HTTP 400 that complains about `think`: the model has no reasoning mode or
/// the server is too old (§4.2.1). The whole body is searched, case-insensitively.
pub(crate) fn rejects_think(response: &HttpResponse) -> bool {
    response.status == StatusCode::BAD_REQUEST
        && String::from_utf8_lossy(&response.body)
            .to_lowercase()
            .contains("think")
}

/// Ollama model names without a tag mean `:latest`; names are compared case-insensitively.
pub(crate) fn same_model(a: &str, b: &str) -> bool {
    fn norm(name: &str) -> String {
        let name = name.trim().to_lowercase();
        if name.contains(':') {
            name
        } else {
            format!("{name}:latest")
        }
    }
    norm(a) == norm(b)
}

/// The model names listed by `/api/tags` (`name`, or `model` when `name` is empty).
pub(crate) fn tag_names(tags: TagsResponse) -> Vec<String> {
    tags.models
        .into_iter()
        .map(|t| if t.name.is_empty() { t.model } else { t.name })
        .filter(|n| !n.is_empty())
        .collect()
}

#[async_trait]
impl Classifier for OllamaClassifier {
    fn info(&self) -> ClassifierInfo {
        ClassifierInfo {
            provider: OLLAMA_PROVIDER,
            model: self.model.clone(),
            is_local: true,
            max_batch: self.max_batch,
            max_concurrency: self.max_concurrency,
        }
    }

    /// `GET /api/version` (reachability, `server_version`) and `GET /api/tags`
    /// (is the configured model installed). A missing model is
    /// `Ok` with `model_available = false`; `latency_ms` covers both requests.
    /// A server that does not answer `/api/version` with 200 is
    /// [`LlmError::Unreachable`] (not an Ollama server at this endpoint).
    async fn health(&self) -> Result<HealthReport, LlmError> {
        let timeout = self.timeout.min(HEALTH_TIMEOUT);
        let started = Instant::now();
        let version = self.get("api/version", timeout).await?;
        if !version.status.is_success() {
            return Err(LlmError::Unreachable(format!(
                "HTTP {} from /api/version",
                version.status.as_u16()
            )));
        }
        let server_version = serde_json::from_slice::<VersionResponse>(&version.body)
            .ok()
            .and_then(|v| v.version)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty());
        let tags = self.get("api/tags", timeout).await?;
        if !tags.status.is_success() {
            return Err(http::status_error(&tags));
        }
        let names = tag_names(parse_json(&tags.body, "invalid /api/tags JSON")?);
        let model_available = names.iter().any(|n| same_model(n, &self.model));
        Ok(HealthReport {
            model_available,
            latency_ms: http::elapsed_ms(started),
            server_version,
        })
    }

    /// One `POST /api/chat` (two on the first `think` rejection, see the
    /// type docs). An empty batch makes no request; a batch larger than
    /// `max_batch` is [`LlmError::BadResponse`]. HTTP 404 about the model is
    /// [`LlmError::ModelNotFound`]; other statuses are mapped as in §4.12.
    /// The answer goes through [`parse_response`]: skipped ids are missing.
    async fn classify(
        &self,
        items: &[ClassifyItem],
        cancel: &CancellationToken,
    ) -> Result<Vec<Classification>, LlmError> {
        if !http::check_batch(items.len(), self.max_batch, cancel)? {
            return Ok(Vec::new());
        }
        let send_think = !self.uses_no_think_fallback();
        let mut response = self.chat(items, send_think, cancel).await?;
        if send_think && rejects_think(&response) {
            tracing::info!(
                provider = OLLAMA_PROVIDER,
                model = %self.model,
                "server rejected think, sending requests without it"
            );
            self.no_think.store(true, Ordering::Relaxed);
            response = self.chat(items, false, cancel).await?;
        }
        if http::is_model_not_found(&response) {
            return Err(LlmError::ModelNotFound(self.model.clone()));
        }
        if !response.status.is_success() {
            return Err(http::status_error(&response));
        }
        let chat: ChatResponse = parse_json(&response.body, "invalid /api/chat JSON")?;
        let content = chat
            .message
            .map(|m| m.content)
            .ok_or_else(|| LlmError::BadResponse("no message.content".to_owned()))?;
        Ok(parse_response(&content, items)?.results)
    }
}

#[cfg(test)]
#[path = "ollama_tests.rs"]
mod tests;
