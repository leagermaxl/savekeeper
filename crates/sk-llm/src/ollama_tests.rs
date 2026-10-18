use serde_json::{json, Value};
use sk_core::model::{ExtStat, FolderSummary, Marker};
use sk_core::template::PathTemplate;

use super::*;

fn summary() -> FolderSummary {
    let path = match PathTemplate::parse(r"{APPDATA}\App") {
        Ok(t) => t,
        Err(e) => panic!("bad template: {e}"),
    };
    FolderSummary {
        path,
        total_bytes: 2048,
        file_count: 3,
        dir_count: 1,
        max_depth: 1,
        newest_mtime: None,
        oldest_mtime: None,
        ext_histogram: vec![ExtStat {
            ext: "json".to_owned(),
            count: 3,
            bytes: 2048,
        }],
        sample_names: vec!["settings.json".to_owned()],
        top_children: Vec::new(),
        markers: vec![Marker::ConfigLike],
        truncated: false,
    }
}

fn item(id: &str) -> ClassifyItem {
    ClassifyItem {
        id: id.to_owned(),
        summary: summary(),
        peek: Vec::new(),
        hint: Some("electron".to_owned()),
    }
}

fn classifier(cfg: &LocalLlmConfig) -> OllamaClassifier {
    match OllamaClassifier::new(cfg) {
        Ok(c) => c,
        Err(e) => panic!("classifier: {e}"),
    }
}

#[test]
fn request_body_matches_the_spec() {
    let c = classifier(&LocalLlmConfig::default());
    let mut body = match c.request_body(&[item("f1"), item("f2")]) {
        Ok(b) => b,
        Err(e) => panic!("body: {e}"),
    };
    // The prompt and schema are checked here and replaced in the snapshot.
    assert_eq!(body["messages"][0]["content"], json!(SYSTEM_PROMPT));
    let schema: Value = serde_json::from_str(RESPONSE_SCHEMA).unwrap_or(Value::Null);
    assert_eq!(body["format"], schema);
    body["messages"][0]["content"] = json!("<SYSTEM_PROMPT>");
    body["format"] = json!("<RESPONSE_SCHEMA>");
    insta::assert_json_snapshot!("ollama_request_body", body);
}

#[test]
fn info_comes_from_the_config() {
    let cfg = LocalLlmConfig {
        model: " llama3.2 ".to_owned(),
        max_batch: 0,
        max_concurrency: 3,
        ..LocalLlmConfig::default()
    };
    let info = classifier(&cfg).info();
    assert_eq!(info.provider, "ollama");
    assert_eq!(info.model, "llama3.2");
    assert!(info.is_local);
    assert_eq!(info.max_batch, 1);
    assert_eq!(info.max_concurrency, 3);

    let default = classifier(&LocalLlmConfig::default()).info();
    assert_eq!((default.max_batch, default.max_concurrency), (8, 1));
}

#[test]
fn invalid_endpoint_is_unreachable() {
    let cfg = LocalLlmConfig {
        endpoint: "127.0.0.1 11434".to_owned(),
        ..LocalLlmConfig::default()
    };
    assert!(matches!(
        OllamaClassifier::new(&cfg),
        Err(LlmError::Unreachable(_))
    ));
}

#[test]
fn model_names_default_to_the_latest_tag() {
    assert!(same_model("llama3.2", "llama3.2:latest"));
    assert!(same_model("Qwen2.5:7B-Instruct", "qwen2.5:7b-instruct"));
    assert!(!same_model("qwen2.5:7b-instruct", "qwen2.5:3b-instruct"));
    assert!(!same_model("qwen2.5", "qwen2.5:7b-instruct"));
}

#[test]
fn tag_names_prefer_name_over_model() {
    let tags: TagsResponse = match serde_json::from_value(json!({
        "models": [
            {"name": "a:1", "model": "a:1"},
            {"model": "b:2"},
            {"name": "", "model": ""},
            {"size": 1}
        ]
    })) {
        Ok(t) => t,
        Err(e) => panic!("tags: {e}"),
    };
    assert_eq!(tag_names(tags), vec!["a:1".to_owned(), "b:2".to_owned()]);
}
