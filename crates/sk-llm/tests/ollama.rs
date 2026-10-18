//! Ollama provider and local auto-detection against a local `wiremock`
//! server (SPEC-08 §6.2). No real network: every endpoint is on 127.0.0.1.

use std::net::TcpListener;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sk_core::config::{LocalLlmConfig, LocalLlmKind};
use sk_core::model::{Category, FolderSummary};
use sk_core::template::PathTemplate;
use sk_core::CancellationToken;
use sk_llm::{
    detect_local_providers_at, Classifier, ClassifyItem, LlmError, OllamaClassifier, SYSTEM_PROMPT,
};
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const MODEL: &str = "qwen2.5:7b-instruct";

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

fn config(endpoint: &str, timeout_s: u32) -> LocalLlmConfig {
    LocalLlmConfig {
        endpoint: endpoint.to_owned(),
        model: MODEL.to_owned(),
        timeout_s,
        ..LocalLlmConfig::default()
    }
}

fn classifier(endpoint: &str, timeout_s: u32) -> OllamaClassifier {
    match OllamaClassifier::new(&config(endpoint, timeout_s)) {
        Ok(c) => c,
        Err(e) => panic!("classifier: {e}"),
    }
}

/// An `/api/chat` answer whose `message.content` is `results` as JSON text.
fn chat_answer(results: &Value) -> ResponseTemplate {
    let content = json!({ "results": results }).to_string();
    ResponseTemplate::new(200).set_body_json(json!({
        "model": MODEL,
        "message": { "role": "assistant", "content": content },
        "done": true,
        "done_reason": "stop"
    }))
}

fn result(id: &str, category: &str) -> Value {
    json!({
        "id": id, "category": category, "app_guess": "  OBS Studio ",
        "importance": 1.7, "confidence": 0.9, "reason": "Settings of OBS.",
        "suggested_include": ["*.json"], "suggested_exclude": ["../x", "logs/**"]
    })
}

/// A localhost port with nothing listening on it.
fn closed_endpoint() -> String {
    let port = match TcpListener::bind("127.0.0.1:0").and_then(|l| l.local_addr()) {
        Ok(addr) => addr.port(),
        Err(e) => panic!("bind: {e}"),
    };
    format!("http://127.0.0.1:{port}")
}

async fn mount_chat(server: &MockServer, response: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(response)
        .mount(server)
        .await;
}

async fn classify_err(server: &MockServer, response: ResponseTemplate) -> LlmError {
    mount_chat(server, response).await;
    let c = classifier(&server.uri(), 30);
    match c.classify(&[item("a")], &CancellationToken::new()).await {
        Ok(r) => panic!("expected an error, got {r:?}"),
        Err(e) => e,
    }
}

#[tokio::test]
async fn classify_sends_the_spec_body_and_normalizes_the_answer() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .and(header("content-type", "application/json"))
        .and(body_partial_json(json!({
            "model": MODEL,
            "stream": false,
            "options": { "temperature": 0, "num_ctx": 8192, "seed": 42 },
            "keep_alive": "10m",
            "format": { "type": "object", "required": ["results"] }
        })))
        .respond_with(chat_answer(&json!([
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
    assert!(!out[1].from_cache);

    // The messages are the system prompt and the compact items JSON.
    let requests = server.received_requests().await.unwrap_or_default();
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap_or(Value::Null);
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["messages"][0]["content"], SYSTEM_PROMPT);
    assert_eq!(body["messages"][1]["role"], "user");
    let user: Value = serde_json::from_str(body["messages"][1]["content"].as_str().unwrap_or(""))
        .unwrap_or(Value::Null);
    assert_eq!(user["items"].as_array().map(Vec::len), Some(3));
}

#[tokio::test]
async fn http_errors_map_to_llm_errors() {
    let not_found = ResponseTemplate::new(404).set_body_json(
        json!({"error": "model \"qwen2.5:7b-instruct\" not found, try pulling it first"}),
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
    let auth = ResponseTemplate::new(401);
    assert_eq!(
        classify_err(&MockServer::start().await, auth).await,
        LlmError::Auth
    );
    let bad = ResponseTemplate::new(400).set_body_json(json!({"error": "invalid format"}));
    assert_eq!(
        classify_err(&MockServer::start().await, bad).await,
        LlmError::BadResponse("HTTP 400: invalid format".to_owned())
    );
    let plain_404 = ResponseTemplate::new(404).set_body_string("404 page not found");
    assert!(matches!(
        classify_err(&MockServer::start().await, plain_404).await,
        LlmError::BadResponse(_)
    ));
}

#[tokio::test]
async fn invalid_answers_are_bad_responses() {
    let not_json = ResponseTemplate::new(200).set_body_json(json!({
        "message": { "role": "assistant", "content": "Sure! Here is {\"results\": [" }
    }));
    assert!(matches!(
        classify_err(&MockServer::start().await, not_json).await,
        LlmError::BadResponse(_)
    ));
    let no_message = ResponseTemplate::new(200).set_body_json(json!({ "done": true }));
    assert!(matches!(
        classify_err(&MockServer::start().await, no_message).await,
        LlmError::BadResponse(_)
    ));
    let not_chat = ResponseTemplate::new(200).set_body_string("<html></html>");
    assert!(matches!(
        classify_err(&MockServer::start().await, not_chat).await,
        LlmError::BadResponse(_)
    ));
}

#[tokio::test]
async fn slow_answer_is_a_timeout() {
    let server = MockServer::start().await;
    mount_chat(
        &server,
        chat_answer(&json!([])).set_delay(Duration::from_secs(5)),
    )
    .await;
    let c = classifier(&server.uri(), 1);
    let started = Instant::now();
    let r = c.classify(&[item("a")], &CancellationToken::new()).await;
    assert_eq!(r, Err(LlmError::Timeout));
    assert!(started.elapsed() < Duration::from_secs(4));
}

#[tokio::test]
async fn cancellation_drops_the_request_within_a_second() {
    let server = MockServer::start().await;
    mount_chat(
        &server,
        chat_answer(&json!([])).set_delay(Duration::from_secs(30)),
    )
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

    // An already cancelled token sends nothing.
    let r = c.classify(&[item("a")], &cancel).await;
    assert_eq!(r, Err(LlmError::Cancelled));
    assert_eq!(server.received_requests().await.map(|r| r.len()), Some(1));
}

#[tokio::test]
async fn empty_and_oversized_batches_send_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(chat_answer(&json!([])))
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

async fn ollama_server(models: &[&str]) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/version"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"version": "0.12.6"})))
        .mount(&server)
        .await;
    let models: Vec<Value> = models
        .iter()
        .map(|m| json!({"name": m, "model": m, "size": 4_683_087_332_u64}))
        .collect();
    Mock::given(method("GET"))
        .and(path("/api/tags"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "models": models })))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn health_reports_version_and_model_availability() {
    let server = ollama_server(&["llama3.2:latest", MODEL]).await;
    let report = match classifier(&server.uri(), 30).health().await {
        Ok(r) => r,
        Err(e) => panic!("health: {e}"),
    };
    assert!(report.model_available);
    assert_eq!(report.server_version.as_deref(), Some("0.12.6"));
    assert!(report.latency_ms < 10_000);
}

#[tokio::test]
async fn health_with_missing_model_is_ok_but_not_available() {
    let server = ollama_server(&["llama3.2:latest"]).await;
    let report = match classifier(&server.uri(), 30).health().await {
        Ok(r) => r,
        Err(e) => panic!("health: {e}"),
    };
    assert!(!report.model_available);
}

#[tokio::test]
async fn health_of_a_non_ollama_server_is_unreachable() {
    let server = MockServer::start().await;
    let r = classifier(&server.uri(), 30).health().await;
    assert!(matches!(r, Err(LlmError::Unreachable(_))), "{r:?}");
}

#[tokio::test]
async fn detection_finds_ollama_and_openai_compatible_servers() {
    let ollama = ollama_server(&[MODEL]).await;
    let lm_studio = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "object": "list",
            "data": [{"id": "qwen2.5-7b-instruct", "object": "model"}]
        })))
        .mount(&lm_studio)
        .await;
    // Answers 404 everywhere: some web server, not an LLM.
    let other = MockServer::start().await;
    let closed = closed_endpoint();
    let (o, l, x) = (ollama.uri(), lm_studio.uri(), other.uri());
    let candidates = [
        (LocalLlmKind::Ollama, o.as_str()),
        (LocalLlmKind::OpenaiCompat, closed.as_str()),
        (LocalLlmKind::OpenaiCompat, x.as_str()),
        (LocalLlmKind::Ollama, l.as_str()),
        (LocalLlmKind::OpenaiCompat, l.as_str()),
    ];
    let found = detect_local_providers_at(&candidates, Duration::from_secs(2)).await;
    assert_eq!(found.len(), 2, "{found:?}");
    assert_eq!(found[0].kind, LocalLlmKind::Ollama);
    assert_eq!(found[0].endpoint, o);
    assert_eq!(found[0].models, [MODEL]);
    assert_eq!(found[0].server_version.as_deref(), Some("0.12.6"));
    assert_eq!(found[1].kind, LocalLlmKind::OpenaiCompat);
    assert_eq!(found[1].endpoint, l);
    assert_eq!(found[1].models, ["qwen2.5-7b-instruct"]);
    assert_eq!(found[1].server_version, None);
}
