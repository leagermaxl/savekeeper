//! Auto-detection of local LLM servers (SPEC-08 FR-08-09).

use std::time::Duration;

use serde::{Deserialize, Serialize};
use sk_core::config::LocalLlmKind;
use sk_core::CancellationToken;

use crate::http::{self, HttpResponse};
use crate::ollama::{tag_names, TagsResponse};
use crate::LlmError;

/// Endpoints probed by [`detect_local_providers`], in this order:
/// Ollama, LM Studio, llama.cpp server (FR-08-09).
pub const LOCAL_CANDIDATES: [(LocalLlmKind, &str); 3] = [
    (LocalLlmKind::Ollama, "http://127.0.0.1:11434"),
    (LocalLlmKind::OpenaiCompat, "http://127.0.0.1:1234"),
    (LocalLlmKind::OpenaiCompat, "http://127.0.0.1:8080"),
];

/// Upper bound of one probe request.
pub const DETECT_TIMEOUT: Duration = Duration::from_secs(2);

/// A local server that answered its probe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct DetectedProvider {
    /// API flavor, for `llm.local.kind`.
    pub kind: LocalLlmKind,
    /// The probed endpoint, for `llm.local.endpoint`.
    pub endpoint: String,
    /// Models the server offers (Ollama `/api/tags`, OpenAI `/v1/models`); may be empty.
    pub models: Vec<String>,
    /// Server version (Ollama `/api/version`).
    pub server_version: Option<String>,
}

/// Probes [`LOCAL_CANDIDATES`] with [`DETECT_TIMEOUT`] per request.
pub async fn detect_local_providers() -> Vec<DetectedProvider> {
    detect_local_providers_at(&LOCAL_CANDIDATES, DETECT_TIMEOUT).await
}

/// Probes `candidates` one by one and returns those that answered, in order.
///
/// - Ollama: `GET /api/version` must return 200 with a JSON object that has a
///   string `version`; then `GET /api/tags` lists the models (a failure there
///   gives an empty list).
/// - OpenAI-compatible: `GET /v1/models` must return 200 with a JSON object
///   that has a `data` array; `data[].id` are the models.
///
/// Anything else (connection refused, timeout, other status or JSON) means
/// "not detected". Never fails; makes only the probe requests.
pub async fn detect_local_providers_at(
    candidates: &[(LocalLlmKind, &str)],
    timeout: Duration,
) -> Vec<DetectedProvider> {
    let mut found = Vec::new();
    for &(kind, endpoint) in candidates {
        let probe = match kind {
            LocalLlmKind::Ollama => probe_ollama(endpoint, timeout).await,
            LocalLlmKind::OpenaiCompat => probe_openai(endpoint, timeout).await,
        };
        if let Ok(Some(p)) = probe {
            found.push(p);
        }
    }
    tracing::debug!(
        probed = candidates.len(),
        found = found.len(),
        "llm auto-detect"
    );
    found
}

async fn get(
    client: &reqwest::Client,
    endpoint: &reqwest::Url,
    path: &str,
    timeout: Duration,
) -> Result<HttpResponse, LlmError> {
    let request = client.get(http::join(endpoint, path)).timeout(timeout);
    http::send(request, &CancellationToken::new()).await
}

async fn probe_ollama(
    endpoint: &str,
    timeout: Duration,
) -> Result<Option<DetectedProvider>, LlmError> {
    #[derive(Deserialize)]
    struct Version {
        version: String,
    }
    let url = http::parse_endpoint(endpoint)?;
    let client = http::client(&url)?;
    let version = get(&client, &url, "api/version", timeout).await?;
    if !version.status.is_success() {
        return Ok(None);
    }
    let Ok(v) = serde_json::from_slice::<Version>(&version.body) else {
        return Ok(None);
    };
    let models = match get(&client, &url, "api/tags", timeout).await {
        Ok(r) if r.status.is_success() => serde_json::from_slice::<TagsResponse>(&r.body)
            .map(tag_names)
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let server_version = Some(v.version.trim().to_owned()).filter(|s| !s.is_empty());
    Ok(Some(DetectedProvider {
        kind: LocalLlmKind::Ollama,
        endpoint: endpoint.to_owned(),
        models,
        server_version,
    }))
}

async fn probe_openai(
    endpoint: &str,
    timeout: Duration,
) -> Result<Option<DetectedProvider>, LlmError> {
    #[derive(Deserialize)]
    struct Models {
        data: Vec<serde_json::Value>,
    }
    let url = http::parse_endpoint(endpoint)?;
    let client = http::client(&url)?;
    let response = get(&client, &url, "v1/models", timeout).await?;
    if !response.status.is_success() {
        return Ok(None);
    }
    let Ok(list) = serde_json::from_slice::<Models>(&response.body) else {
        return Ok(None);
    };
    let models = list
        .data
        .iter()
        .filter_map(|m| m.get("id").and_then(|id| id.as_str()))
        .map(str::to_owned)
        .collect();
    Ok(Some(DetectedProvider {
        kind: LocalLlmKind::OpenaiCompat,
        endpoint: endpoint.to_owned(),
        models,
        server_version: None,
    }))
}
