//! OpenAI-compatible provider against a local `wiremock` server (SPEC-08 §6.2).
//! No real network: every endpoint is on 127.0.0.1.

use std::net::TcpListener;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sk_core::config::{LocalLlmConfig, LocalLlmKind};
use sk_core::model::{Category, FolderSummary};
use sk_core::template::PathTemplate;
use sk_core::CancellationToken;
use sk_llm::{
    Classifier, ClassifyItem, LlmError, OpenAiCompatClassifier, FALLBACK_SCHEMA_INTRO,
    SYSTEM_PROMPT,
};
use wiremock::matchers::{body_partial_json, header, header_exists, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const MODEL: &str = "qwen2.5-7b-instruct";
const CHAT: &str = "/v1/chat/completions";

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

fn classifier(endpoint: &str, timeout_s: u32) -> OpenAiCompatClassifier {
    let cfg = LocalLlmConfig {
        kind: LocalLlmKind::OpenaiCompat,
        endpoint: endpoint.to_owned(),
        model: MODEL.to_owned(),
        timeout_s,
        ..LocalLlmConfig::default()
    };
    match OpenAiCompatClassifier::new(&cfg) {
        Ok(c) => c,
        Err(e) => panic!("classifier: {e}"),
    }
}

/// A chat completion whose `choices[0].message.content` is `results` as JSON text.
fn completion(results: &Value) -> ResponseTemplate {
    let content = json!({ "results": results }).to_string();
    ResponseTemplate::new(200).set_body_json(json!({
        "id": "chatcmpl-1",
        "object": "chat.completion",
        "model": MODEL,
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": content },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 900, "completion_tokens": 80, "total_tokens": 980 }
    }))
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
        .and(path(CHAT))
        .respond_with(response)
        .mount(server)
        .await;
    let c = classifier(&server.uri(), 30);
    match c.classify(&[item("a")], &CancellationToken::new()).await {
        Ok(r) => panic!("expected an error, got {r:?}"),
        Err(e) => e,
    }
}

#[tokio::test]
async fn classify_sends_json_schema_and_normalizes_the_answer() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(CHAT))
        .and(header("content-type", "application/json"))
        .and(body_partial_json(json!({
            "model": MODEL,
            "temperature": 0,
            "response_format": {
                "type": "json_schema",
                "json_schema": {
                    "name": "classifications",
                    "strict": true,
                    "schema": { "type": "object", "required": ["results"] }
                }
            }
        })))
        .respond_with(completion(&json!([
            result("b", "app_config"),
            result("stranger", "cache"),
            result("a", "no_such_category"),
        ])))
        .expect(1)
        .mount(&server)
        .await;
    let c = classifier(&server.uri(), 30);
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
    assert!(!c.uses_json_object_fallback());

    let sent = requests(&server).await;
    assert!(
        sent[0].headers.get("authorization").is_none(),
        "no key, no header"
    );
    let body = json_body(&sent[0]);
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["messages"][0]["content"], SYSTEM_PROMPT);
    assert_eq!(body["messages"][1]["role"], "user");
    let user: Value = serde_json::from_str(body["messages"][1]["content"].as_str().unwrap_or(""))
        .unwrap_or(Value::Null);
    assert_eq!(user["items"].as_array().map(Vec::len), Some(3));
}

/// A server like an old LM Studio / llama.cpp: 400 for `json_schema`.
async fn server_without_json_schema() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(CHAT))
        .and(body_partial_json(
            json!({ "response_format": { "type": "json_schema" } }),
        ))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": { "message": "'response_format.type' must be 'json_object' or 'text'",
                       "type": "invalid_request_error" }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(CHAT))
        .and(body_partial_json(
            json!({ "response_format": { "type": "json_object" } }),
        ))
        .respond_with(completion(&json!([result("a", "app_config")])))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn rejected_json_schema_falls_back_to_json_object_for_the_session() {
    let server = server_without_json_schema().await;
    let c = classifier(&server.uri(), 30);
    let cancel = CancellationToken::new();
    let out = match c.classify(&[item("a")], &cancel).await {
        Ok(r) => r,
        Err(e) => panic!("classify: {e}"),
    };
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].category, Category::AppConfig);
    assert!(c.uses_json_object_fallback());

    let sent = requests(&server).await;
    assert_eq!(sent.len(), 2, "rejected json_schema, then json_object");
    let fallback = json_body(&sent[1]);
    let system = fallback["messages"][0]["content"].as_str().unwrap_or("");
    assert!(system.starts_with(SYSTEM_PROMPT));
    assert!(
        system.contains(FALLBACK_SCHEMA_INTRO),
        "schema in the prompt"
    );
    assert!(system.contains(r#""required":["results"]"#));

    // The flag is cached: the next batch, also from a clone, goes straight to json_object.
    let clone = c.clone();
    assert!(clone.classify(&[item("a")], &cancel).await.is_ok());
    let sent = requests(&server).await;
    assert_eq!(sent.len(), 3);
    assert_eq!(
        json_body(&sent[2])["response_format"],
        json!({ "type": "json_object" })
    );
}

#[tokio::test]
async fn other_400_errors_do_not_fall_back() {
    let server = MockServer::start().await;
    let bad = ResponseTemplate::new(400)
        .set_body_json(json!({ "error": { "message": "context length exceeded" } }));
    assert_eq!(
        classify_err(&server, bad).await,
        LlmError::BadResponse("HTTP 400: context length exceeded".to_owned())
    );
    assert_eq!(requests(&server).await.len(), 1);
}

#[tokio::test]
async fn fallback_that_fails_too_is_an_error() {
    let server = MockServer::start().await;
    let rejected = ResponseTemplate::new(400)
        .set_body_json(json!({ "error": "response_format is not supported" }));
    assert!(matches!(
        classify_err(&server, rejected).await,
        LlmError::BadResponse(_)
    ));
    assert_eq!(requests(&server).await.len(), 2, "one fallback, no loop");
}

#[tokio::test]
async fn api_key_is_sent_as_bearer() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(CHAT))
        .and(header("authorization", "Bearer sk-local-123"))
        .respond_with(completion(&json!([])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header_exists("authorization"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [] })))
        .expect(1)
        .mount(&server)
        .await;
    let c = classifier(&server.uri(), 30).with_api_key(" sk-local-123 ");
    assert_eq!(
        c.classify(&[item("a")], &CancellationToken::new()).await,
        Ok(Vec::new())
    );
    assert!(c.health().await.is_ok());
}

#[tokio::test]
async fn http_errors_map_to_llm_errors() {
    let not_found = ResponseTemplate::new(404).set_body_json(
        json!({ "error": { "message": "The model `qwen2.5-7b-instruct` does not exist." } }),
    );
    assert_eq!(
        classify_err(&MockServer::start().await, not_found).await,
        LlmError::ModelNotFound(MODEL.to_owned())
    );
    let limited = ResponseTemplate::new(429).insert_header("retry-after", "5");
    assert_eq!(
        classify_err(&MockServer::start().await, limited).await,
        LlmError::RateLimited(Some(Duration::from_secs(5)))
    );
    for code in [500, 503, 529] {
        assert_eq!(
            classify_err(&MockServer::start().await, ResponseTemplate::new(code)).await,
            LlmError::Overloaded,
            "{code}"
        );
    }
    for code in [401, 403] {
        assert_eq!(
            classify_err(&MockServer::start().await, ResponseTemplate::new(code)).await,
            LlmError::Auth,
            "{code}"
        );
    }
    let plain_404 = ResponseTemplate::new(404).set_body_string("File Not Found");
    assert!(matches!(
        classify_err(&MockServer::start().await, plain_404).await,
        LlmError::BadResponse(_)
    ));
}

#[tokio::test]
async fn invalid_answers_are_bad_responses() {
    let truncated = ResponseTemplate::new(200).set_body_json(json!({
        "choices": [{ "message": { "role": "assistant", "content": "{\"results\": [" },
                      "finish_reason": "length" }]
    }));
    let null_content = ResponseTemplate::new(200).set_body_json(json!({
        "choices": [{ "message": { "role": "assistant", "content": null } }]
    }));
    let no_choices = ResponseTemplate::new(200).set_body_json(json!({ "choices": [] }));
    let not_json = ResponseTemplate::new(200).set_body_string("<html></html>");
    for response in [truncated, null_content, no_choices, not_json] {
        assert!(matches!(
            classify_err(&MockServer::start().await, response).await,
            LlmError::BadResponse(_)
        ));
    }
}

#[tokio::test]
async fn slow_answer_is_a_timeout() {
    let server = MockServer::start().await;
    let slow = completion(&json!([])).set_delay(Duration::from_secs(5));
    let started = Instant::now();
    assert_eq!(
        classify_err_with_timeout(&server, slow, 1).await,
        LlmError::Timeout
    );
    assert!(started.elapsed() < Duration::from_secs(4));
}

async fn classify_err_with_timeout(
    server: &MockServer,
    response: ResponseTemplate,
    timeout_s: u32,
) -> LlmError {
    Mock::given(method("POST"))
        .and(path(CHAT))
        .respond_with(response)
        .mount(server)
        .await;
    let c = classifier(&server.uri(), timeout_s);
    match c.classify(&[item("a")], &CancellationToken::new()).await {
        Ok(r) => panic!("expected an error, got {r:?}"),
        Err(e) => e,
    }
}

#[tokio::test]
async fn cancellation_drops_the_request_within_a_second() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(CHAT))
        .respond_with(completion(&json!([])).set_delay(Duration::from_secs(30)))
        .mount(&server)
        .await;
    let c = classifier(&server.uri(), 60);
    let cancel = CancellationToken::new();
    let batch = [item("a")];
    let started = Instant::now();
    let (r, ()) = tokio::join!(c.classify(&batch, &cancel), async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel.cancel();
    });
    assert_eq!(r, Err(LlmError::Cancelled));
    assert!(started.elapsed() < Duration::from_secs(1));

    let r = c.classify(&[item("a")], &cancel).await;
    assert_eq!(r, Err(LlmError::Cancelled));
    assert_eq!(requests(&server).await.len(), 1);
}

#[tokio::test]
async fn empty_and_oversized_batches_send_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(completion(&json!([])))
        .expect(0)
        .mount(&server)
        .await;
    let c = classifier(&server.uri(), 30);
    let cancel = CancellationToken::new();
    assert_eq!(c.classify(&[], &cancel).await, Ok(Vec::new()));
    let nine: Vec<ClassifyItem> = (0..9).map(|i| item(&i.to_string())).collect();
    assert!(matches!(
        c.classify(&nine, &cancel).await,
        Err(LlmError::BadResponse(_))
    ));
}

#[tokio::test]
async fn closed_port_is_unreachable() {
    let c = classifier(&closed_endpoint(), 5);
    let r = c.classify(&[item("a")], &CancellationToken::new()).await;
    assert!(matches!(r, Err(LlmError::Unreachable(_))), "{r:?}");
    assert!(matches!(c.health().await, Err(LlmError::Unreachable(_))));
}

async fn models_server(response: ResponseTemplate) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(response)
        .mount(&server)
        .await;
    server
}

fn model_list(ids: &[&str]) -> ResponseTemplate {
    let data: Vec<Value> = ids
        .iter()
        .map(|id| json!({ "id": id, "object": "model", "owned_by": "organization_owner" }))
        .collect();
    ResponseTemplate::new(200).set_body_json(json!({ "object": "list", "data": data }))
}

#[tokio::test]
async fn health_reports_model_availability() {
    let server = models_server(model_list(&["text-embedding-nomic", "Qwen2.5-7B-Instruct"])).await;
    let report = match classifier(&server.uri(), 30).health().await {
        Ok(r) => r,
        Err(e) => panic!("health: {e}"),
    };
    assert!(report.model_available);
    assert_eq!(report.server_version, None);
    assert!(report.latency_ms < 10_000);

    let server = models_server(model_list(&["llama-3.2-3b-instruct"])).await;
    let report = match classifier(&server.uri(), 30).health().await {
        Ok(r) => r,
        Err(e) => panic!("health: {e}"),
    };
    assert!(!report.model_available);
}

#[tokio::test]
async fn health_errors() {
    let server = models_server(ResponseTemplate::new(401)).await;
    assert_eq!(
        classifier(&server.uri(), 30).health().await,
        Err(LlmError::Auth)
    );

    // Some other web server: 404 everywhere.
    let server = MockServer::start().await;
    let r = classifier(&server.uri(), 30).health().await;
    assert!(matches!(r, Err(LlmError::Unreachable(_))), "{r:?}");

    let server = models_server(ResponseTemplate::new(200).set_body_string("<html></html>")).await;
    let r = classifier(&server.uri(), 30).health().await;
    assert!(matches!(r, Err(LlmError::Unreachable(_))), "{r:?}");
}
