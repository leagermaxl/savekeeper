//! Ollama `"think": false` and its fallback (SPEC-08 §4.2.1) against a local
//! `wiremock` server. No real network: every endpoint is on 127.0.0.1.

use serde_json::{json, Value};
use sk_core::config::LocalLlmConfig;
use sk_core::model::{Category, FolderSummary};
use sk_core::template::PathTemplate;
use sk_core::CancellationToken;
use sk_llm::{Classifier, ClassifyItem, LlmError, OllamaClassifier};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const MODEL: &str = "qwen3.5:9b";

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

fn classifier(endpoint: &str) -> OllamaClassifier {
    let cfg = LocalLlmConfig {
        endpoint: endpoint.to_owned(),
        model: MODEL.to_owned(),
        timeout_s: 30,
        ..LocalLlmConfig::default()
    };
    match OllamaClassifier::new(&cfg) {
        Ok(c) => c,
        Err(e) => panic!("classifier: {e}"),
    }
}

/// An `/api/chat` answer for item `id`, with a `thinking` field the client must ignore.
fn chat_answer(id: &str) -> ResponseTemplate {
    let content = json!({ "results": [{
        "id": id, "category": "app_config", "app_guess": "OBS Studio",
        "importance": 0.8, "confidence": 0.9, "reason": "Settings.",
        "suggested_include": [], "suggested_exclude": []
    }]})
    .to_string();
    ResponseTemplate::new(200).set_body_json(json!({
        "model": MODEL,
        "message": { "role": "assistant", "content": content, "thinking": "Let me think..." },
        "done": true
    }))
}

fn json_body(request: &Request) -> Value {
    serde_json::from_slice(&request.body).unwrap_or(Value::Null)
}

fn has_think(request: &Request) -> bool {
    json_body(request).get("think").is_some()
}

async fn requests(server: &MockServer) -> Vec<Request> {
    server.received_requests().await.unwrap_or_default()
}

/// A server that rejects `think` with `rejection` and answers requests without it.
async fn server_rejecting_think(rejection: ResponseTemplate) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .and(has_think)
        .respond_with(rejection)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .and(|r: &Request| !has_think(r))
        .respond_with(chat_answer("a"))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn think_false_is_sent_and_thinking_in_the_answer_is_ignored() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(chat_answer("a"))
        .expect(1)
        .mount(&server)
        .await;
    let c = classifier(&server.uri());
    let results = match c.classify(&[item("a")], &CancellationToken::new()).await {
        Ok(r) => r,
        Err(e) => panic!("classify: {e}"),
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].category, Category::AppConfig);
    assert_eq!(
        json_body(&requests(&server).await[0])["think"],
        json!(false)
    );
    assert!(!c.uses_no_think_fallback());
}

#[tokio::test]
async fn rejected_think_is_dropped_for_the_session_and_its_clones() {
    let rejection = ResponseTemplate::new(400)
        .set_body_json(json!({ "error": "\"llama3.2\" does not support thinking" }));
    let server = server_rejecting_think(rejection).await;
    let c = classifier(&server.uri());
    let clone = c.clone();
    let cancel = CancellationToken::new();

    let first = c.classify(&[item("a")], &cancel).await;
    assert!(matches!(&first, Ok(r) if r.len() == 1), "{first:?}");
    assert!(c.uses_no_think_fallback());
    assert!(clone.uses_no_think_fallback(), "clones share the flag");

    let second = clone.classify(&[item("a")], &cancel).await;
    assert!(matches!(&second, Ok(r) if r.len() == 1), "{second:?}");
    let sent: Vec<bool> = requests(&server).await.iter().map(has_think).collect();
    assert_eq!(
        sent,
        vec![true, false, false],
        "one fallback, then no think"
    );
}

#[tokio::test]
async fn other_400_errors_keep_think() {
    let server = server_rejecting_think(
        ResponseTemplate::new(400).set_body_json(json!({ "error": "invalid format schema" })),
    )
    .await;
    let c = classifier(&server.uri());
    let err = c.classify(&[item("a")], &CancellationToken::new()).await;
    assert!(
        matches!(&err, Err(LlmError::BadResponse(m)) if m.starts_with("HTTP 400")),
        "{err:?}"
    );
    assert_eq!(requests(&server).await.len(), 1);
    assert!(!c.uses_no_think_fallback());
}

#[tokio::test]
async fn fallback_that_fails_too_is_an_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(
            ResponseTemplate::new(400).set_body_json(json!({ "error": "think: bad request" })),
        )
        .mount(&server)
        .await;
    let c = classifier(&server.uri());
    let err = c.classify(&[item("a")], &CancellationToken::new()).await;
    assert!(
        matches!(&err, Err(LlmError::BadResponse(m)) if m.starts_with("HTTP 400")),
        "{err:?}"
    );
    let sent: Vec<bool> = requests(&server).await.iter().map(has_think).collect();
    assert_eq!(sent, vec![true, false], "one fallback, no loop");
}

#[tokio::test]
async fn missing_model_after_the_fallback_is_model_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .and(has_think)
        .respond_with(
            ResponseTemplate::new(400).set_body_json(json!({ "error": "think is not supported" })),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .and(|r: &Request| !has_think(r))
        .respond_with(
            ResponseTemplate::new(404)
                .set_body_json(json!({ "error": format!("model \"{MODEL}\" not found") })),
        )
        .mount(&server)
        .await;
    let c = classifier(&server.uri());
    let err = c.classify(&[item("a")], &CancellationToken::new()).await;
    assert!(matches!(err, Err(LlmError::ModelNotFound(_))), "{err:?}");
    assert_eq!(requests(&server).await.len(), 2);
}
