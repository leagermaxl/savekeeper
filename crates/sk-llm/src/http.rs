//! HTTP plumbing shared by the providers (SPEC-08 §4.2, §5).
//!
//! All providers use `reqwest` over raw HTTP (rustls, no OpenSSL). Errors are
//! mapped to [`LlmError`] without request URLs, headers or bodies, so an API
//! key can never end up in a message (NFR-08-04).

use std::net::IpAddr;
use std::time::Duration;

use reqwest::header::{HeaderMap, RETRY_AFTER};
use reqwest::{RequestBuilder, StatusCode, Url};
use sk_core::CancellationToken;

use crate::LlmError;

/// Upper bound of a response body; real answers are a few kilobytes.
pub(crate) const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

/// Maximum length of a provider error message kept in [`LlmError::BadResponse`].
const MAX_ERROR_CHARS: usize = 200;

/// A complete HTTP response.
#[derive(Debug)]
pub(crate) struct HttpResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

/// Parses a configured endpoint (`http://127.0.0.1:11434`, trailing `/` allowed).
pub(crate) fn parse_endpoint(endpoint: &str) -> Result<Url, LlmError> {
    let url = Url::parse(endpoint.trim())
        .map_err(|e| LlmError::Unreachable(format!("invalid endpoint URL: {e}")))?;
    if !matches!(url.scheme(), "http" | "https") || url.host().is_none() {
        return Err(LlmError::Unreachable(
            "invalid endpoint URL: expected http(s)://host[:port]".to_owned(),
        ));
    }
    Ok(url)
}

/// `{endpoint}/{path}`: keeps a path prefix of the endpoint (`http://h/ollama`).
pub(crate) fn join(endpoint: &Url, path: &str) -> Url {
    let mut url = endpoint.clone();
    let base = url.path().trim_end_matches('/').to_owned();
    url.set_path(&format!("{base}/{}", path.trim_start_matches('/')));
    url.set_query(None);
    url.set_fragment(None);
    url
}

/// HTTP client for `endpoint`.
///
/// Loopback endpoints (`127.0.0.0/8`, `::1`, `localhost`) bypass any proxy
/// (SPEC-08 §5, corporate proxy); other endpoints use the system proxy.
pub(crate) fn client(endpoint: &Url) -> Result<reqwest::Client, LlmError> {
    // reqwest is built with `rustls-no-provider`: use ring unless the process
    // already installed a provider (then `install_default` fails harmlessly).
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut builder =
        reqwest::Client::builder().user_agent(concat!("SaveKeeper/", env!("CARGO_PKG_VERSION")));
    if is_loopback(endpoint) {
        builder = builder.no_proxy();
    }
    builder
        .build()
        .map_err(|e| LlmError::Unreachable(format!("http client: {}", e.without_url())))
}

/// The endpoint is on this machine.
pub(crate) fn is_loopback(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Sends `request` and reads the whole body, racing the cancellation token.
///
/// Cancellation drops the request future (FR-08-10): [`LlmError::Cancelled`].
/// A timeout set on the request is [`LlmError::Timeout`]; connection
/// failures are [`LlmError::Unreachable`]. The status is not checked here.
pub(crate) async fn send(
    request: RequestBuilder,
    cancel: &CancellationToken,
) -> Result<HttpResponse, LlmError> {
    if cancel.is_cancelled() {
        return Err(LlmError::Cancelled);
    }
    let exchange = async {
        let mut response = request.send().await.map_err(transport)?;
        let status = response.status();
        let headers = response.headers().clone();
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(transport)? {
            if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
                return Err(LlmError::BadResponse("response too large".to_owned()));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    };
    cancel
        .run_until_cancelled(exchange)
        .await
        .unwrap_or(Err(LlmError::Cancelled))
}

/// Maps a transport error, without its URL.
fn transport(e: reqwest::Error) -> LlmError {
    if e.is_timeout() {
        LlmError::Timeout
    } else if e.is_connect() {
        LlmError::Unreachable("connection failed".to_owned())
    } else if e.is_body() || e.is_decode() {
        LlmError::Unreachable("connection lost while reading the response".to_owned())
    } else {
        LlmError::Unreachable(e.without_url().to_string())
    }
}

/// Maps a non-success status the provider gives no special meaning to
/// (SPEC-08 §4.12, §5): 401/403 → `Auth`; 429 → `RateLimited` with
/// `retry-after` in seconds; 529 and other 5xx → `Overloaded` (retried with
/// backoff); anything else → `BadResponse` with the provider's error text.
pub(crate) fn status_error(response: &HttpResponse) -> LlmError {
    let status = response.status;
    match status.as_u16() {
        401 | 403 => LlmError::Auth,
        429 => LlmError::RateLimited(retry_after(&response.headers)),
        500..=599 => LlmError::Overloaded,
        code => LlmError::BadResponse(format!("HTTP {code}: {}", error_message(&response.body))),
    }
}

/// `retry-after` as a number of seconds; the HTTP-date form is ignored.
pub(crate) fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let value = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();
    if let Ok(secs) = value.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    let secs = value.parse::<f64>().ok()?;
    (secs.is_finite() && secs >= 0.0).then(|| Duration::from_secs_f64(secs.min(86_400.0)))
}

/// The error text of a provider error body: `{"error": "..."}` (Ollama) or
/// `{"error": {"message": "..."}}` (OpenAI, Anthropic); otherwise the body
/// itself. Control characters removed, at most 200 characters.
pub(crate) fn error_message(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let from_json = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| match v.get("error") {
            Some(serde_json::Value::String(s)) => Some(s.clone()),
            Some(e) => e.get("message").and_then(|m| m.as_str()).map(str::to_owned),
            None => None,
        });
    let raw = from_json.unwrap_or_else(|| text.into_owned());
    let cleaned: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX_ERROR_CHARS)
        .collect();
    cleaned.trim().to_owned()
}

#[cfg(test)]
#[path = "http_tests.rs"]
mod tests;
