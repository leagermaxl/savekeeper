//! «Check connection» and recommendations for the settings screen
//! (SPEC-08 §4.11, T-08-13) against a local `wiremock` Ollama server.
//! No real network: every endpoint is on 127.0.0.1.

use std::net::TcpListener;

use serde_json::json;
use sk_core::config::{LlmConfig, LocalLlmConfig, LocalLlmKind};
use sk_core::model::LlmMode;
use sk_llm::{
    detect_local_providers_at, llm_health, recommendations, EnvStore, LlmHealthStatus,
    DETECT_TIMEOUT,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn ollama_server(models: &[&str]) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/version"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "version": "0.40.6" })))
        .mount(&server)
        .await;
    let models: Vec<_> = models.iter().map(|m| json!({ "name": m })).collect();
    Mock::given(method("GET"))
        .and(path("/api/tags"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "models": models })))
        .mount(&server)
        .await;
    server
}

fn local(endpoint: &str, model: &str) -> LlmConfig {
    LlmConfig {
        local: LocalLlmConfig {
            endpoint: endpoint.to_owned(),
            model: model.to_owned(),
            ..LocalLlmConfig::default()
        },
        ..LlmConfig::default()
    }
}

#[tokio::test]
async fn installed_model_is_ok_with_the_server_version() {
    let server = ollama_server(&["qwen3.5:latest", "qwen3.5:9b"]).await;
    let h = llm_health(
        &local(&server.uri(), "qwen3.5:9b"),
        LlmMode::Local,
        &EnvStore::new(),
    )
    .await;
    assert_eq!(h.status, LlmHealthStatus::Ok, "{h:?}");
    assert_eq!(h.provider, Some("ollama"));
    assert_eq!(h.server_version.as_deref(), Some("0.40.6"));
    assert_eq!(h.is_local, Some(true));
    assert!(h.latency_ms.is_some());
    assert_eq!(h.pull_command, None);
}

#[tokio::test]
async fn missing_ollama_model_gets_the_pull_command() {
    let server = ollama_server(&["llama3.2:3b"]).await;
    let h = llm_health(
        &local(&server.uri(), "qwen3.5:4b"),
        LlmMode::Local,
        &EnvStore::new(),
    )
    .await;
    assert_eq!(h.status, LlmHealthStatus::ModelMissing);
    assert_eq!(h.pull_command.as_deref(), Some("ollama pull qwen3.5:4b"));
    assert_eq!(h.error, None);
}

#[tokio::test]
async fn closed_port_is_unreachable() {
    let port = match TcpListener::bind("127.0.0.1:0").and_then(|l| l.local_addr()) {
        Ok(addr) => addr.port(),
        Err(e) => panic!("bind: {e}"),
    };
    let endpoint = format!("http://127.0.0.1:{port}");
    let h = llm_health(&local(&endpoint, "m"), LlmMode::Local, &EnvStore::new()).await;
    assert_eq!(h.status, LlmHealthStatus::Unreachable);
    let error = h.error.unwrap_or_default();
    assert!(!error.is_empty());
    assert!(!error.contains("127.0.0.1"), "no URL in messages: {error}");
}

#[tokio::test]
async fn detected_models_drive_the_recommendations() {
    let server = ollama_server(&["qwen3.5:9b"]).await;
    let uri = server.uri();
    let found =
        detect_local_providers_at(&[(LocalLlmKind::Ollama, uri.as_str())], DETECT_TIMEOUT).await;
    assert_eq!(found.len(), 1);
    let recs = recommendations(Some(&found[0].models));
    let state = |name: &str| {
        recs.iter()
            .find(|r| r.model.model.as_deref() == Some(name))
            .map(|r| (r.installed, r.pull_command.clone()))
    };
    assert_eq!(state("qwen3.5:9b"), Some((Some(true), None)));
    assert_eq!(
        state("qwen2.5:7b-instruct"),
        Some((
            Some(false),
            Some("ollama pull qwen2.5:7b-instruct".to_owned())
        ))
    );
}
