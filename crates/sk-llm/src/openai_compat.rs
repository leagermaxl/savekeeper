//! OpenAI-compatible provider: llama.cpp `llama-server`, LM Studio, vLLM (SPEC-08 §4.2.2).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use reqwest::header::{HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::{RequestBuilder, StatusCode, Url};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use serde_json::{json, Value};
use sk_core::config::LocalLlmConfig;
use sk_core::CancellationToken;

use crate::http::{self, parse_json, usize_of, HttpResponse};
use crate::{
    parse_response, user_message, Classification, Classifier, ClassifierInfo, ClassifyItem,
    HealthReport, LlmError, HEALTH_TIMEOUT, RESPONSE_SCHEMA, SYSTEM_PROMPT,
};

/// [`ClassifierInfo::provider`] of [`OpenAiCompatClassifier`].
pub const OPENAI_COMPAT_PROVIDER: &str = "openai_compat";

/// `response_format.json_schema.name` of the structured-output request.
pub const JSON_SCHEMA_NAME: &str = "classifications";

/// Text appended to the system prompt in the `json_object` fallback, followed
/// by the compact response schema (§4.2.2: "схема в промпте").
pub const FALLBACK_SCHEMA_INTRO: &str =
    "\n\nOutput a single JSON object that matches this JSON Schema:\n";

/// Classifier backed by an OpenAI-compatible server:
/// `POST {endpoint}/v1/chat/completions` with `response_format: json_schema`.
///
/// A server that rejects `json_schema` (HTTP 400 mentioning `response_format`)
/// is asked again with `{"type": "json_object"}` and the schema in the system
/// prompt; that choice is remembered for the lifetime of the classifier (and
/// its clones), so later batches go straight to `json_object`.
#[derive(Debug, Clone)]
pub struct OpenAiCompatClassifier {
    client: reqwest::Client,
    endpoint: Url,
    model: String,
    /// Never shown by `Debug` (NFR-08-04); exposed only to build the header.
    api_key: Option<SecretString>,
    timeout: Duration,
    max_batch: usize,
    max_concurrency: usize,
    /// The server does not support `json_schema`: use `json_object` (session flag).
    json_object_mode: Arc<AtomicBool>,
}

/// `response_format` variant of a chat request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Format {
    /// `{"type": "json_schema", "json_schema": {...}}` (structured outputs).
    JsonSchema,
    /// `{"type": "json_object"}` with the schema in the system prompt.
    JsonObject,
}

impl OpenAiCompatClassifier {
    /// A classifier for the `llm.local` config section (`kind = openai_compat`).
    ///
    /// Uses `endpoint` (without `/v1`), `model`, `timeout_s` (per request; at
    /// least 1 s), `max_batch` and `max_concurrency` (each at least 1);
    /// `kind` is not checked. No API key; see [`Self::with_api_key`]. Makes no
    /// request. An invalid endpoint URL is [`LlmError::Unreachable`].
    pub fn new(cfg: &LocalLlmConfig) -> Result<Self, LlmError> {
        let endpoint = http::parse_endpoint(&cfg.endpoint)?;
        let client = http::client(&endpoint)?;
        Ok(Self {
            client,
            endpoint,
            model: cfg.model.trim().to_owned(),
            api_key: None,
            timeout: Duration::from_secs(u64::from(cfg.timeout_s.max(1))),
            max_batch: usize_of(cfg.max_batch),
            max_concurrency: usize_of(cfg.max_concurrency),
            json_object_mode: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Sends `Authorization: Bearer <key>` with every request (optional,
    /// §4.2.2). A blank key means no header.
    #[must_use]
    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        let raw = SecretString::from(key.into());
        let key = raw.expose_secret().trim();
        self.api_key = (!key.is_empty()).then(|| SecretString::from(key));
        self
    }

    /// The server rejected `json_schema` earlier in this session.
    pub fn uses_json_object_fallback(&self) -> bool {
        self.json_object_mode.load(Ordering::Relaxed)
    }

    /// The `/v1/chat/completions` body for one batch (§4.2.2).
    pub(crate) fn request_body(
        &self,
        items: &[ClassifyItem],
        format: Format,
    ) -> Result<Value, LlmError> {
        let schema: Value = serde_json::from_str(RESPONSE_SCHEMA)
            .map_err(|e| LlmError::BadResponse(format!("response schema: {e}")))?;
        let (system, response_format) = match format {
            Format::JsonSchema => (
                SYSTEM_PROMPT.to_owned(),
                json!({
                    "type": "json_schema",
                    "json_schema": { "name": JSON_SCHEMA_NAME, "strict": true, "schema": schema },
                }),
            ),
            Format::JsonObject => (
                format!("{SYSTEM_PROMPT}{FALLBACK_SCHEMA_INTRO}{schema}"),
                json!({ "type": "json_object" }),
            ),
        };
        Ok(json!({
            "model": self.model,
            "temperature": 0,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user_message(items) },
            ],
            "response_format": response_format,
        }))
    }

    /// Adds `Authorization: Bearer` when a key is set; the header is marked sensitive.
    fn authorize(&self, request: RequestBuilder) -> RequestBuilder {
        let Some(key) = &self.api_key else {
            return request;
        };
        let bearer = SecretString::from(format!("Bearer {}", key.expose_secret()));
        match HeaderValue::from_str(bearer.expose_secret()) {
            Ok(mut value) => {
                value.set_sensitive(true);
                request.header(AUTHORIZATION, value)
            }
            // A key with characters not allowed in a header: send none, the
            // server answers 401 (`Auth`) if it needs one.
            Err(_) => request,
        }
    }

    async fn chat(
        &self,
        items: &[ClassifyItem],
        format: Format,
        cancel: &CancellationToken,
    ) -> Result<HttpResponse, LlmError> {
        let body = self.request_body(items, format)?.to_string();
        let request = self
            .client
            .post(http::join(&self.endpoint, "v1/chat/completions"))
            .header(CONTENT_TYPE, "application/json")
            .timeout(self.timeout)
            .body(body);
        let started = Instant::now();
        let response = http::send(self.authorize(request), cancel).await?;
        tracing::debug!(
            provider = OPENAI_COMPAT_PROVIDER,
            model = %self.model,
            items = items.len(),
            json_object = format == Format::JsonObject,
            status = response.status.as_u16(),
            elapsed_ms = http::elapsed_ms(started),
            "llm request finished"
        );
        Ok(response)
    }
}

/// HTTP 400 that complains about `response_format`: the server does not
/// support `json_schema` (§4.2.2). The whole body is searched, case-insensitively.
pub(crate) fn rejects_json_schema(response: &HttpResponse) -> bool {
    response.status == StatusCode::BAD_REQUEST
        && String::from_utf8_lossy(&response.body)
            .to_lowercase()
            .contains("response_format")
}

/// Does model `id` from `/v1/models` name the configured `model`?
///
/// Case-insensitive; besides the full id, its last `/` or `\` segment
/// without a `.gguf` suffix also matches (llama.cpp lists the model file path).
pub(crate) fn model_matches(id: &str, model: &str) -> bool {
    let model = model.trim().to_lowercase();
    let id = id.trim().to_lowercase();
    if model.is_empty() {
        return false;
    }
    if id == model {
        return true;
    }
    let file = id.rsplit(['/', '\\']).next().unwrap_or(&id);
    file.strip_suffix(".gguf").unwrap_or(file) == model
}

/// `GET /v1/models`.
#[derive(Deserialize)]
struct ModelsResponse {
    data: Vec<Value>,
}

/// `POST /v1/chat/completions`.
#[derive(Deserialize)]
struct ChatResponse {
    #[serde(default)]
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: Option<ChoiceMessage>,
}

#[derive(Deserialize)]
struct ChoiceMessage {
    content: Option<String>,
}

#[async_trait]
impl Classifier for OpenAiCompatClassifier {
    fn info(&self) -> ClassifierInfo {
        ClassifierInfo {
            provider: OPENAI_COMPAT_PROVIDER,
            model: self.model.clone(),
            is_local: http::is_loopback(&self.endpoint),
            max_batch: self.max_batch,
            max_concurrency: self.max_concurrency,
        }
    }

    /// `GET /v1/models`: reachability and whether `data[].id` lists the model.
    /// 401/403 is [`LlmError::Auth`]; any other non-200 status or a body
    /// without a `data` array is [`LlmError::Unreachable`] (not an
    /// OpenAI-compatible server at this endpoint). A missing model is `Ok`
    /// with `model_available = false`; `server_version` is `None`.
    async fn health(&self) -> Result<HealthReport, LlmError> {
        let request = self
            .client
            .get(http::join(&self.endpoint, "v1/models"))
            .timeout(self.timeout.min(HEALTH_TIMEOUT));
        let started = Instant::now();
        // Health checks are not cancellable: they are short and bounded by the timeout.
        let response = http::send(self.authorize(request), &CancellationToken::new()).await?;
        let latency_ms = http::elapsed_ms(started);
        match response.status.as_u16() {
            401 | 403 => return Err(LlmError::Auth),
            _ if !response.status.is_success() => {
                return Err(LlmError::Unreachable(format!(
                    "HTTP {} from /v1/models",
                    response.status.as_u16()
                )))
            }
            _ => {}
        }
        let models: ModelsResponse = serde_json::from_slice(&response.body).map_err(|_| {
            LlmError::Unreachable("/v1/models is not an OpenAI model list".to_owned())
        })?;
        let model_available = models
            .data
            .iter()
            .filter_map(|m| m.get("id").and_then(Value::as_str))
            .any(|id| model_matches(id, &self.model));
        Ok(HealthReport {
            model_available,
            latency_ms,
            server_version: None,
        })
    }

    /// One `POST /v1/chat/completions` (two on the first `json_schema`
    /// rejection, see the type docs). An empty batch makes no request; a batch
    /// larger than `max_batch` is [`LlmError::BadResponse`]. HTTP 404 about
    /// the model is [`LlmError::ModelNotFound`]; other statuses are mapped as
    /// in §4.12. `choices[0].message.content` goes through [`parse_response`]:
    /// skipped ids are missing.
    async fn classify(
        &self,
        items: &[ClassifyItem],
        cancel: &CancellationToken,
    ) -> Result<Vec<Classification>, LlmError> {
        if !http::check_batch(items.len(), self.max_batch, cancel)? {
            return Ok(Vec::new());
        }
        let mut format = if self.uses_json_object_fallback() {
            Format::JsonObject
        } else {
            Format::JsonSchema
        };
        let mut response = self.chat(items, format, cancel).await?;
        if format == Format::JsonSchema && rejects_json_schema(&response) {
            tracing::info!(
                provider = OPENAI_COMPAT_PROVIDER,
                model = %self.model,
                "server rejected json_schema, falling back to json_object"
            );
            self.json_object_mode.store(true, Ordering::Relaxed);
            format = Format::JsonObject;
            response = self.chat(items, format, cancel).await?;
        }
        if http::is_model_not_found(&response) {
            return Err(LlmError::ModelNotFound(self.model.clone()));
        }
        if !response.status.is_success() {
            return Err(http::status_error(&response));
        }
        let chat: ChatResponse = parse_json(&response.body, "invalid chat completion JSON")?;
        let content = chat
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message)
            .and_then(|m| m.content)
            .ok_or_else(|| LlmError::BadResponse("no choices[0].message.content".to_owned()))?;
        Ok(parse_response(&content, items)?.results)
    }
}

#[cfg(test)]
#[path = "openai_compat_tests.rs"]
mod tests;
