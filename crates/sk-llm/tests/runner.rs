//! `ClassifyRunner` over real providers against a local `wiremock` server
//! (SPEC-08 §6.2, §8): 429, `max_tokens` halving, refusal, privacy of every
//! sent request, a second run fully from the cache. No real network.

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};
use sk_core::config::{CloudLlmConfig, LocalLlmConfig};
use sk_core::env::Environment;
use sk_core::model::FolderSummary;
use sk_core::template::PathTemplate;
use sk_core::CancellationToken;
use sk_llm::{
    AnthropicClassifier, Classifier, ClassifyItem, ClassifyRunner, LlmCache, LlmConfig, LlmMode,
    OllamaClassifier, RunOutput,
};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const HAIKU: &str = "claude-haiku-4-5";

/// `unwrap` for test setup (the lints forbid `unwrap` outside unit tests).
trait Must<T> {
    fn must(self) -> T;
}

impl<T, E: std::fmt::Debug> Must<T> for Result<T, E> {
    fn must(self) -> T {
        match self {
            Ok(v) => v,
            Err(e) => panic!("{e:?}"),
        }
    }
}

impl<T> Must<T> for Option<T> {
    fn must(self) -> T {
        match self {
            Some(v) => v,
            None => panic!("missing value"),
        }
    }
}

fn temp_dir() -> TempDir {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
    fs::create_dir_all(&base).must();
    tempfile::Builder::new()
        .prefix("sk-llm-runner-it-")
        .tempdir_in(&base)
        .must()
}

fn item(id: &str, name: &str) -> ClassifyItem {
    ClassifyItem {
        id: id.to_owned(),
        summary: FolderSummary {
            path: PathTemplate::parse(&format!(r"{{APPDATA}}\{name}")).must(),
            total_bytes: 4096,
            file_count: 2,
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

fn env(dir: &TempDir) -> Environment {
    let mut env = Environment::fake(dir.path());
    env.user_name = "maxl".to_owned();
    env.machine_name = "MAXPC".to_owned();
    env
}

/// The item ids of a request body (`messages[0].content` or Ollama's user message).
fn sent_ids(request: &Request) -> Vec<String> {
    let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
    let user = body["messages"]
        .as_array()
        .and_then(|m| m.iter().find(|m| m["role"] == "user"))
        .and_then(|m| m["content"].as_str())
        .unwrap_or("{}");
    let items: Value = serde_json::from_str(user).unwrap_or(Value::Null);
    items["items"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|i| i["id"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn results_json(ids: &[String]) -> String {
    let results: Vec<Value> = ids
        .iter()
        .map(|id| {
            json!({
                "id": id, "category": "app_config", "app_guess": "App",
                "importance": 0.5, "confidence": 0.9, "reason": "Settings.",
                "suggested_include": [], "suggested_exclude": []
            })
        })
        .collect();
    json!({ "results": results }).to_string()
}

fn anthropic_message(text: &str, stop_reason: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "id": "msg_01", "type": "message", "role": "assistant", "model": HAIKU,
        "content": [{ "type": "text", "text": text }],
        "stop_reason": stop_reason, "stop_sequence": null,
        "usage": { "input_tokens": 1000, "output_tokens": 100,
                   "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0 }
    }))
}

async fn run(runner: &ClassifyRunner, items: Vec<ClassifyItem>) -> RunOutput {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    runner.run(items, &tx, &CancellationToken::new()).await
}

fn keys(out: &RunOutput) -> Vec<&str> {
    out.issues.iter().map(|i| i.message_key.as_str()).collect()
}

#[tokio::test]
async fn anthropic_retries_halves_refuses_and_never_leaks() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/v1/models/{HAIKU}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": HAIKU })))
        .mount(&server)
        .await;
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(move |request: &Request| {
            let ids = sent_ids(request);
            if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                return ResponseTemplate::new(429).insert_header("retry-after", "0");
            }
            if ids.len() > 1 {
                return anthropic_message("{\"results\":[", "max_tokens");
            }
            if ids.iter().any(|id| id == "refuse") {
                return anthropic_message("", "refusal");
            }
            anthropic_message(&results_json(&ids), "end_turn")
        })
        .mount(&server)
        .await;

    let dir = temp_dir();
    let cfg = LlmConfig {
        mode: LlmMode::Cloud,
        ..LlmConfig::default()
    };
    let classifier = AnthropicClassifier::new(&CloudLlmConfig::default(), "sk-ant-test-key")
        .and_then(|c| c.with_endpoint(&server.uri()))
        .must();
    let classifier: Arc<dyn Classifier> = Arc::new(classifier);
    let cache = LlmCache::open(&LlmCache::file_path(dir.path())).must();
    let runner = ClassifyRunner::new(classifier, cache, cfg).with_env(env(&dir));
    let mut leaky = item("leak", "Leaky");
    leaky.summary.sample_names = vec!["Maxl/notes.txt".to_owned()];
    let items = vec![
        item("a", "Alpha"),
        item("b", "Beta"),
        item("c", "Charlie"),
        item("refuse", "Delta"),
        leaky,
    ];
    let out = run(&runner, items).await;

    let ids: Vec<&str> = out.results.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["a", "b", "c"]);
    assert_eq!(keys(&out), ["llm.privacy_blocked", "llm.refusal"]);
    let received = server.received_requests().await.must();
    let posted: Vec<Vec<String>> = received
        .iter()
        .filter(|r| r.method.as_str() == "POST")
        .map(sent_ids)
        .collect();
    // 429 → the same batch again → max_tokens → halves, down to single items.
    let expected: [&[&str]; 8] = [
        &["a", "b", "c", "refuse"],
        &["a", "b", "c", "refuse"],
        &["a", "b"],
        &["a"],
        &["b"],
        &["c", "refuse"],
        &["c"],
        &["refuse"],
    ];
    assert_eq!(posted, expected, "{posted:?}");
    for request in &received {
        let body = String::from_utf8_lossy(&request.body).to_lowercase();
        assert!(!body.contains("maxl") && !body.contains("maxpc"), "{body}");
    }
    // Every 2xx answer is paid, truncated and refused ones too (Haiku 4.5 prices).
    assert_eq!(out.usage.requests, 7);
    assert!((out.usage.cost_usd - 7.0 * (0.001 + 0.0005)).abs() < 1e-9);
}

#[tokio::test]
async fn ollama_second_run_makes_no_request() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/version"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "version": "0.6.0" })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/tags"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({ "models": [{ "name": "qwen2.5:7b-instruct", "model": "qwen2.5:7b-instruct" }] }),
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(|request: &Request| {
            let content = results_json(&sent_ids(request));
            ResponseTemplate::new(200).set_body_json(json!({
                "model": "qwen2.5:7b-instruct", "done": true,
                "message": { "role": "assistant", "content": content }
            }))
        })
        .mount(&server)
        .await;

    let dir = temp_dir();
    let local = LocalLlmConfig {
        endpoint: server.uri(),
        ..LocalLlmConfig::default()
    };
    let cfg = LlmConfig {
        mode: LlmMode::Local,
        local: local.clone(),
        ..LlmConfig::default()
    };
    let items: Vec<ClassifyItem> = (0..10)
        .map(|i| item(&format!("i{i}"), &format!("App{i}")))
        .collect();
    let new_runner = || {
        let classifier: Arc<dyn Classifier> = Arc::new(OllamaClassifier::new(&local).must());
        let cache = LlmCache::open(&LlmCache::file_path(dir.path())).must();
        ClassifyRunner::new(classifier, cache, cfg.clone()).with_env(env(&dir))
    };
    let out = run(&new_runner(), items.clone()).await;
    assert_eq!(out.results.len(), 10);
    assert!(out.issues.is_empty(), "{:?}", out.issues);
    let first = server.received_requests().await.must().len();
    // Health (2) + two batches of at most 8.
    assert_eq!(first, 4);

    let out = run(&new_runner(), items).await;
    assert_eq!(out.results.len(), 10);
    assert!(out.results.iter().all(|r| r.from_cache));
    assert_eq!(server.received_requests().await.must().len(), first);
}
