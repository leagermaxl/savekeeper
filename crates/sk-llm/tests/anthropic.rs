//! Anthropic provider against a local `wiremock` server (SPEC-08 §6.2).
//! No real network: every endpoint is on 127.0.0.1.

use std::net::TcpListener;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sk_core::config::CloudLlmConfig;
use sk_core::model::{Category, FolderSummary};
use sk_core::template::PathTemplate;
use sk_core::CancellationToken;
use sk_llm::{AnthropicClassifier, Classifier, ClassifyItem, LlmError, SYSTEM_PROMPT};
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const MODEL: &str = "claude-haiku-4-5";
const KEY: &str = "sk-ant-test-0123456789abcdef";
const MESSAGES: &str = "/v1/messages";

fn item(id: &str) -> ClassifyItem {
    let path = match PathTemplate::parse(r"{APPDATA}\App") {
        Ok(t) => t,
        Err(e) => panic!("bad template: {e}"),
    };
    ClassifyItem {
        id: id.to_owned(),
        summary: FolderSummary {
            path,
            total_bytes: 10,
            file_count: 1,
            dir_count: 0,
            max_depth: 0,
            newest_mtime: None,
            oldest_mtime: None,
            ext_histogram: Vec::new(),
            sample_names: vec!["settings.json".to_owned()],
            top_children: Vec::new(),
            markers: Vec::new(),
            truncated: false,
        },
        peek: Vec::new(),
        hint: None,
    }
}

fn classifier_with(endpoint: &str, model: &str, timeout_s: u32) -> AnthropicClassifier {
    let cfg = CloudLlmConfig {
        model: model.to_owned(),
        timeout_s,
        ..CloudLlmConfig::default()
    };
    match AnthropicClassifier::new(&cfg, KEY).and_then(|c| c.with_endpoint(endpoint)) {
        Ok(c) => c,
        Err(e) => panic!("classifier: {e}"),
    }
}

fn classifier(endpoint: &str) -> AnthropicClassifier {
    classifier_with(endpoint, MODEL, 30)
}

/// A Messages API answer with `content` blocks and `stop_reason`.
fn message(content: &Value, stop_reason: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "id": "msg_01",
        "type": "message",
        "role": "assistant",
        "model": MODEL,
        "content": content,
        "stop_reason": stop_reason,
        "stop_sequence": null,
        "usage": {
            "input_tokens": 900, "output_tokens": 80,
            "cache_creation_input_tokens": 0, "cache_read_input_tokens": 1500
        }
    }))
}

fn answer(results: &Value) -> ResponseTemplate {
    let text = json!({ "results": results }).to_string();
    message(&json!([{ "type": "text", "text": text }]), "end_turn")
}

fn result(id: &str, category: &str) -> Value {
    json!({
        "id": id, "category": category, "app_guess": "  OBS Studio ",
        "importance": 1.7, "confidence": 0.9, "reason": "Settings of OBS.",
        "suggested_include": ["*.json"], "suggested_exclude": ["../x", "logs/**"]
    })
}

fn closed_endpoint() -> String {
    let port = match TcpListener::bind("127.0.0.1:0").and_then(|l| l.local_addr()) {
        Ok(addr) => addr.port(),
        Err(e) => panic!("bind: {e}"),
    };
    format!("http://127.0.0.1:{port}")
}

fn json_body(request: &Request) -> Value {
    serde_json::from_slice(&request.body).unwrap_or(Value::Null)
}

async fn requests(server: &MockServer) -> Vec<Request> {
    server.received_requests().await.unwrap_or_default()
}

async fn classify_err(server: &MockServer, response: ResponseTemplate) -> LlmError {
    Mock::given(method("POST"))
        .and(path(MESSAGES))
        .respond_with(response)
        .mount(server)
        .await;
    let c = classifier(&server.uri());
    match c.classify(&[item("a")], &CancellationToken::new()).await {
        Ok(r) => panic!("expected an error, got {r:?}"),
        Err(e) => e,
    }
}

#[tokio::test]
async fn classify_sends_the_spec_request_and_normalizes_the_answer() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(MESSAGES))
        .and(header("x-api-key", KEY))
        .and(header("anthropic-version", "2023-06-01"))
        .and(header("content-type", "application/json"))
        .and(body_partial_json(json!({
            "model": MODEL,
            "max_tokens": 4096,
            "system": [{ "type": "text", "cache_control": { "type": "ephemeral" } }],
            "output_config": { "format": { "type": "json_schema",
                                           "schema": { "type": "object", "required": ["results"] } } }
        })))
        .respond_with(message(
            &json!([
                { "type": "thinking", "thinking": "", "signature": "x" },
                { "type": "text", "text": json!({ "results": [
                    result("b", "app_config"),
                    result("stranger", "cache"),
                    result("a", "no_such_category"),
                ] }).to_string() }
            ]),
            "end_turn",
        ))
        .expect(1)
        .mount(&server)
        .await;
    let c = classifier(&server.uri());
    let items = [item("a"), item("b"), item("c")];
    let mut out = match c.classify(&items, &CancellationToken::new()).await {
        Ok(r) => r,
        Err(e) => panic!("classify: {e}"),
    };
    out.sort_by(|x, y| x.id.cmp(&y.id));
    let ids: Vec<&str> = out.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["a", "b"], "foreign id ignored, skipped id missing");
    assert_eq!(out[0].category, Category::Unknown);
    assert_eq!(out[1].category, Category::AppConfig);
    assert_eq!(out[1].app_guess.as_deref(), Some("OBS Studio"));
    assert!((out[1].importance - 1.0).abs() < f32::EPSILON);
    assert_eq!(out[1].suggested_exclude, ["logs/**"]);

    let sent = requests(&server).await;
    let body = json_body(&sent[0]);
    assert_eq!(body["system"][0]["text"], SYSTEM_PROMPT);
    assert!(body["output_config"].get("effort").is_none());
    assert_eq!(body["messages"][0]["role"], "user");
    let user: Value = serde_json::from_str(body["messages"][0]["content"].as_str().unwrap_or(""))
        .unwrap_or(Value::Null);
    assert_eq!(user["items"].as_array().map(Vec::len), Some(3));
    let raw = String::from_utf8_lossy(&sent[0].body);
    assert!(!raw.contains(KEY), "the key is never in the body");
}

#[tokio::test]
async fn usage_and_cost_are_accumulated_across_clones() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(MESSAGES))
        .respond_with(answer(&json!([result("a", "app_config")])))
        .mount(&server)
        .await;
    let c = classifier(&server.uri());
    let clone = c.clone();
    let cancel = CancellationToken::new();
    assert!(c.classify(&[item("a")], &cancel).await.is_ok());
    assert!(clone.classify(&[item("a")], &cancel).await.is_ok());
    let usage = c.usage();
    assert_eq!(usage.requests, 2);
    assert_eq!(usage.input_tokens, 1800);
    assert_eq!(usage.output_tokens, 160);
    assert_eq!(usage.cache_read_input_tokens, 3000);
    assert_eq!(usage.cache_creation_input_tokens, 0);
    // Haiku 4.5: 1800 × $1 + 160 × $5 + 3000 × $0.10 per million tokens.
    let expected = (1800.0 + 160.0 * 5.0 + 3000.0 * 0.1) / 1_000_000.0;
    assert!((usage.cost_usd - expected).abs() < 1e-12, "{usage:?}");
    assert_eq!(clone.usage(), usage);
}

#[tokio::test]
async fn sonnet_5_request_has_low_effort() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(MESSAGES))
        .and(body_partial_json(json!({
            "model": "claude-sonnet-5",
            "output_config": { "effort": "low", "format": { "type": "json_schema" } }
        })))
        .respond_with(answer(&json!([])))
        .expect(1)
        .mount(&server)
        .await;
    let c = classifier_with(&server.uri(), "claude-sonnet-5", 30);
    assert_eq!(
        c.classify(&[item("a")], &CancellationToken::new()).await,
        Ok(Vec::new())
    );
}

#[tokio::test]
async fn stop_reasons_are_reported() {
    let truncated = message(
        &json!([{ "type": "text", "text": "{\"results\": [" }]),
        "max_tokens",
    );
    assert_eq!(
        classify_err(&MockServer::start().await, truncated).await,
        LlmError::MaxTokens
    );
    let refused = message(&json!([]), "refusal");
    assert_eq!(
        classify_err(&MockServer::start().await, refused).await,
        LlmError::Refusal
    );
}

#[tokio::test]
async fn usage_is_recorded_even_for_a_truncated_answer() {
    let server = MockServer::start().await;
    let truncated = message(&json!([{ "type": "text", "text": "{" }]), "max_tokens");
    Mock::given(method("POST"))
        .and(path(MESSAGES))
        .respond_with(truncated)
        .mount(&server)
        .await;
    let c = classifier(&server.uri());
    assert_eq!(
        c.classify(&[item("a")], &CancellationToken::new()).await,
        Err(LlmError::MaxTokens)
    );
    assert_eq!(c.usage().requests, 1);
    assert_eq!(c.usage().output_tokens, 80);
}

#[tokio::test]
async fn http_errors_map_to_llm_errors() {
    let limited = ResponseTemplate::new(429)
        .insert_header("retry-after", "7")
        .set_body_json(json!({ "type": "error",
            "error": { "type": "rate_limit_error", "message": "Number of requests exceeded" } }));
    assert_eq!(
        classify_err(&MockServer::start().await, limited).await,
        LlmError::RateLimited(Some(Duration::from_secs(7)))
    );
    let limited_no_header = ResponseTemplate::new(429);
    assert_eq!(
        classify_err(&MockServer::start().await, limited_no_header).await,
        LlmError::RateLimited(None)
    );
    let overloaded = ResponseTemplate::new(529).set_body_json(json!({ "type": "error",
        "error": { "type": "overloaded_error", "message": "Overloaded" } }));
    assert_eq!(
        classify_err(&MockServer::start().await, overloaded).await,
        LlmError::Overloaded
    );
    for code in [500, 502, 503] {
        assert_eq!(
            classify_err(&MockServer::start().await, ResponseTemplate::new(code)).await,
            LlmError::Overloaded,
            "{code}"
        );
    }
    for code in [401, 403] {
        let server = MockServer::start().await;
        assert_eq!(
            classify_err(&server, ResponseTemplate::new(code)).await,
            LlmError::Auth,
            "{code}"
        );
        assert_eq!(
            requests(&server).await.len(),
            1,
            "no retries inside classify"
        );
    }
    let bad = ResponseTemplate::new(400).set_body_json(json!({ "type": "error",
        "error": { "type": "invalid_request_error", "message": "max_tokens: too large" } }));
    assert_eq!(
        classify_err(&MockServer::start().await, bad).await,
        LlmError::BadResponse("HTTP 400: max_tokens: too large".to_owned())
    );
    let unknown_model = ResponseTemplate::new(404).set_body_json(json!({ "type": "error",
        "error": { "type": "not_found_error", "message": "model: claude-haiku-4-5" } }));
    assert_eq!(
        classify_err(&MockServer::start().await, unknown_model).await,
        LlmError::ModelNotFound(MODEL.to_owned())
    );
}

#[tokio::test]
async fn invalid_answers_are_bad_responses() {
    let not_json_text = message(
        &json!([{ "type": "text", "text": "Sure! Here" }]),
        "end_turn",
    );
    let no_text_block = message(&json!([{ "type": "thinking", "thinking": "" }]), "end_turn");
    let not_json = ResponseTemplate::new(200).set_body_string("<html></html>");
    let odd_stop = message(&json!([{ "type": "text", "text": "{}" }]), "tool_use");
    for response in [not_json_text, no_text_block, not_json, odd_stop] {
        assert!(matches!(
            classify_err(&MockServer::start().await, response).await,
            LlmError::BadResponse(_)
        ));
    }
}

#[tokio::test]
async fn slow_answer_is_a_timeout() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(MESSAGES))
        .respond_with(answer(&json!([])).set_delay(Duration::from_secs(5)))
        .mount(&server)
        .await;
    let c = classifier_with(&server.uri(), MODEL, 1);
    let started = Instant::now();
    assert_eq!(
        c.classify(&[item("a")], &CancellationToken::new()).await,
        Err(LlmError::Timeout)
    );
    assert!(started.elapsed() < Duration::from_secs(4));
}

#[tokio::test]
async fn cancellation_drops_the_request_within_a_second() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(MESSAGES))
        .respond_with(answer(&json!([])).set_delay(Duration::from_secs(30)))
        .mount(&server)
        .await;
    let c = classifier_with(&server.uri(), MODEL, 60);
    let cancel = CancellationToken::new();
    let batch = [item("a")];
    let started = Instant::now();
    let (r, ()) = tokio::join!(c.classify(&batch, &cancel), async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel.cancel();
    });
    assert_eq!(r, Err(LlmError::Cancelled));
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(c.classify(&batch, &cancel).await, Err(LlmError::Cancelled));
    assert_eq!(requests(&server).await.len(), 1);
}

#[tokio::test]
async fn empty_and_oversized_batches_send_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(answer(&json!([])))
        .expect(0)
        .mount(&server)
        .await;
    let c = classifier(&server.uri());
    let cancel = CancellationToken::new();
    assert_eq!(c.classify(&[], &cancel).await, Ok(Vec::new()));
    let many: Vec<ClassifyItem> = (0..21).map(|i| item(&i.to_string())).collect();
    assert!(matches!(
        c.classify(&many, &cancel).await,
        Err(LlmError::BadResponse(_))
    ));
}

#[tokio::test]
async fn closed_port_is_unreachable() {
    let c = classifier(&closed_endpoint());
    let r = c.classify(&[item("a")], &CancellationToken::new()).await;
    assert!(matches!(r, Err(LlmError::Unreachable(_))), "{r:?}");
    assert!(matches!(c.health().await, Err(LlmError::Unreachable(_))));
}

async fn model_server(response: ResponseTemplate) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/v1/models/{MODEL}")))
        .and(header("x-api-key", KEY))
        .and(header("anthropic-version", "2023-06-01"))
        .respond_with(response)
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn health_checks_the_model_without_spending_tokens() {
    let server = model_server(ResponseTemplate::new(200).set_body_json(json!({
        "type": "model", "id": MODEL, "display_name": "Claude Haiku 4.5"
    })))
    .await;
    let c = classifier(&server.uri());
    let report = match c.health().await {
        Ok(r) => r,
        Err(e) => panic!("health: {e}"),
    };
    assert!(report.model_available);
    assert_eq!(report.server_version, None);
    assert!(report.latency_ms < 10_000);
    assert_eq!(c.usage().requests, 0, "health is not a classification");
    assert!(requests(&server)
        .await
        .iter()
        .all(|r| r.method.as_str() == "GET"));

    let missing = ResponseTemplate::new(404).set_body_json(json!({ "type": "error",
        "error": { "type": "not_found_error", "message": "model: claude-haiku-4-5" } }));
    let server = model_server(missing).await;
    let report = classifier(&server.uri()).health().await;
    assert!(matches!(report, Ok(r) if !r.model_available));
}

#[tokio::test]
async fn health_errors() {
    let server = model_server(ResponseTemplate::new(401)).await;
    assert_eq!(
        classifier(&server.uri()).health().await,
        Err(LlmError::Auth)
    );
    let server = model_server(ResponseTemplate::new(529)).await;
    assert_eq!(
        classifier(&server.uri()).health().await,
        Err(LlmError::Overloaded)
    );
    let server = model_server(ResponseTemplate::new(429).insert_header("retry-after", "3")).await;
    assert_eq!(
        classifier(&server.uri()).health().await,
        Err(LlmError::RateLimited(Some(Duration::from_secs(3))))
    );
}
