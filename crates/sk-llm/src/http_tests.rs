use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue, RETRY_AFTER};
use reqwest::StatusCode;

use super::*;

fn url(s: &str) -> Url {
    match parse_endpoint(s) {
        Ok(u) => u,
        Err(e) => panic!("bad endpoint {s}: {e}"),
    }
}

fn response(code: u16, headers: HeaderMap, body: &str) -> HttpResponse {
    HttpResponse {
        status: StatusCode::from_u16(code).unwrap_or(StatusCode::IM_A_TEAPOT),
        headers,
        body: body.as_bytes().to_vec(),
    }
}

#[test]
fn endpoint_must_be_http_with_host() {
    assert!(parse_endpoint("http://127.0.0.1:11434").is_ok());
    assert!(parse_endpoint(" https://example.org/ ").is_ok());
    for bad in [
        "",
        "127.0.0.1:11434",
        "ftp://host",
        "not a url",
        "file:///c:/x",
    ] {
        assert!(
            matches!(parse_endpoint(bad), Err(LlmError::Unreachable(_))),
            "{bad}"
        );
    }
}

#[test]
fn join_keeps_the_endpoint_path_prefix() {
    assert_eq!(
        join(&url("http://127.0.0.1:11434"), "api/chat").as_str(),
        "http://127.0.0.1:11434/api/chat"
    );
    assert_eq!(
        join(&url("http://127.0.0.1:11434/"), "/api/chat").as_str(),
        "http://127.0.0.1:11434/api/chat"
    );
    assert_eq!(
        join(&url("http://host/ollama/?x=1"), "api/tags").as_str(),
        "http://host/ollama/api/tags"
    );
}

#[test]
fn loopback_hosts_are_detected() {
    for local in [
        "http://127.0.0.1:1234",
        "http://127.1.2.3",
        "http://localhost:8080",
        "http://LOCALHOST",
        "http://[::1]:11434",
    ] {
        assert!(is_loopback(&url(local)), "{local}");
    }
    for remote in [
        "http://192.168.1.10:11434",
        "https://api.anthropic.com",
        "http://localhost.example.org",
    ] {
        assert!(!is_loopback(&url(remote)), "{remote}");
    }
}

#[test]
fn statuses_map_to_errors() {
    let none = HeaderMap::new();
    assert_eq!(
        status_error(&response(401, none.clone(), "")),
        LlmError::Auth
    );
    assert_eq!(
        status_error(&response(403, none.clone(), "")),
        LlmError::Auth
    );
    assert_eq!(
        status_error(&response(429, none.clone(), "")),
        LlmError::RateLimited(None)
    );
    for code in [500, 502, 503, 529] {
        assert_eq!(
            status_error(&response(code, none.clone(), "")),
            LlmError::Overloaded,
            "{code}"
        );
    }
    assert_eq!(
        status_error(&response(400, none, r#"{"error":"bad\nrequest"}"#)),
        LlmError::BadResponse("HTTP 400: bad request".to_owned())
    );
}

#[test]
fn retry_after_is_read_in_seconds() {
    let mut h = HeaderMap::new();
    h.insert(RETRY_AFTER, HeaderValue::from_static("7"));
    assert_eq!(retry_after(&h), Some(Duration::from_secs(7)));
    assert_eq!(
        status_error(&response(429, h.clone(), "")),
        LlmError::RateLimited(Some(Duration::from_secs(7)))
    );
    h.insert(RETRY_AFTER, HeaderValue::from_static("1.5"));
    assert_eq!(retry_after(&h), Some(Duration::from_millis(1500)));
    h.insert(
        RETRY_AFTER,
        HeaderValue::from_static("Wed, 21 Oct 2026 07:28:00 GMT"),
    );
    assert_eq!(retry_after(&h), None);
    h.insert(RETRY_AFTER, HeaderValue::from_static("-3"));
    assert_eq!(retry_after(&h), None);
}

#[test]
fn error_message_reads_known_shapes_and_is_bounded() {
    assert_eq!(
        error_message(br#"{"error":"model not found"}"#),
        "model not found"
    );
    assert_eq!(
        error_message(br#"{"error":{"type":"x","message":"bad schema"}}"#),
        "bad schema"
    );
    assert_eq!(error_message(b"plain text"), "plain text");
    let long = "x".repeat(1000);
    assert_eq!(error_message(long.as_bytes()).chars().count(), 200);
}

#[tokio::test]
async fn send_reports_cancellation_before_the_request() {
    let endpoint = url("http://127.0.0.1:9");
    let client = match client(&endpoint) {
        Ok(c) => c,
        Err(e) => panic!("client: {e}"),
    };
    let cancel = CancellationToken::new();
    cancel.cancel();
    let result = send(client.get(endpoint), &cancel).await;
    assert!(matches!(result, Err(LlmError::Cancelled)));
}
