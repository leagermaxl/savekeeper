use reqwest::header::HeaderMap;
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

fn config() -> LocalLlmConfig {
    LocalLlmConfig {
        kind: sk_core::config::LocalLlmKind::OpenaiCompat,
        endpoint: "http://127.0.0.1:1234".to_owned(),
        model: "qwen2.5-7b-instruct".to_owned(),
        ..LocalLlmConfig::default()
    }
}

fn classifier(cfg: &LocalLlmConfig) -> OpenAiCompatClassifier {
    match OpenAiCompatClassifier::new(cfg) {
        Ok(c) => c,
        Err(e) => panic!("classifier: {e}"),
    }
}

fn schema() -> Value {
    serde_json::from_str(RESPONSE_SCHEMA).unwrap_or(Value::Null)
}

fn body(format: Format) -> Value {
    match classifier(&config()).request_body(&[item("f1"), item("f2")], format) {
        Ok(b) => b,
        Err(e) => panic!("body: {e}"),
    }
}

#[test]
fn json_schema_body_matches_the_spec() {
    let mut body = body(Format::JsonSchema);
    // The prompt and schema are checked here and replaced in the snapshot.
    assert_eq!(body["messages"][0]["content"], json!(SYSTEM_PROMPT));
    assert_eq!(body["response_format"]["json_schema"]["schema"], schema());
    body["messages"][0]["content"] = json!("<SYSTEM_PROMPT>");
    body["response_format"]["json_schema"]["schema"] = json!("<RESPONSE_SCHEMA>");
    insta::assert_json_snapshot!("openai_compat_request_body", body);
}

#[test]
fn json_object_body_puts_the_schema_into_the_prompt() {
    let body = body(Format::JsonObject);
    assert_eq!(body["response_format"], json!({ "type": "json_object" }));
    let system = body["messages"][0]["content"].as_str().unwrap_or("");
    let Some(schema_text) = system
        .strip_prefix(SYSTEM_PROMPT)
        .and_then(|rest| rest.strip_prefix(FALLBACK_SCHEMA_INTRO))
    else {
        panic!("unexpected system prompt: {system}");
    };
    let parsed: Value = serde_json::from_str(schema_text).unwrap_or(Value::Null);
    assert_eq!(parsed, schema());
    assert_eq!(body["temperature"], json!(0));
    assert_eq!(body["messages"][1]["role"], "user");
}

#[test]
fn info_comes_from_the_config() {
    let info = classifier(&config()).info();
    assert_eq!(info.provider, "openai_compat");
    assert_eq!(info.model, "qwen2.5-7b-instruct");
    assert!(info.is_local);
    assert_eq!((info.max_batch, info.max_concurrency), (8, 1));

    let remote = LocalLlmConfig {
        endpoint: "https://llm.example.org".to_owned(),
        max_batch: 0,
        max_concurrency: 0,
        ..config()
    };
    let info = classifier(&remote).info();
    assert!(!info.is_local, "a non-loopback endpoint is not local");
    assert_eq!((info.max_batch, info.max_concurrency), (1, 1));
}

#[test]
fn invalid_endpoint_is_unreachable() {
    let cfg = LocalLlmConfig {
        endpoint: "localhost:1234".to_owned(),
        ..config()
    };
    assert!(matches!(
        OpenAiCompatClassifier::new(&cfg),
        Err(LlmError::Unreachable(_))
    ));
}

#[test]
fn api_key_is_not_shown_by_debug() {
    let c = classifier(&config()).with_api_key("sk-very-secret-key-1234567890");
    let text = format!("{c:?}");
    assert!(!text.contains("very-secret"), "{text}");
    assert!(text.contains("<redacted>"));
    assert!(classifier(&config()).with_api_key("  ").api_key.is_none());
}

fn response(code: u16, body: &str) -> HttpResponse {
    HttpResponse {
        status: StatusCode::from_u16(code).unwrap_or(StatusCode::IM_A_TEAPOT),
        headers: HeaderMap::new(),
        body: body.as_bytes().to_vec(),
    }
}

#[test]
fn json_schema_rejection_needs_400_mentioning_response_format() {
    assert!(rejects_json_schema(&response(
        400,
        r#"{"error":{"message":"'response_format.type' must be 'json_object' or 'text'"}}"#
    )));
    assert!(rejects_json_schema(&response(
        400,
        "Unsupported RESPONSE_FORMAT"
    )));
    assert!(!rejects_json_schema(&response(400, r#"{"error":"bad"}"#)));
    assert!(!rejects_json_schema(&response(
        500,
        "response_format not supported"
    )));
}

#[test]
fn model_ids_match_case_insensitively_and_by_file_name() {
    assert!(model_matches("Qwen2.5-7B-Instruct", "qwen2.5-7b-instruct"));
    assert!(model_matches(
        "models/qwen2.5-7b-instruct.gguf",
        "qwen2.5-7b-instruct"
    ));
    assert!(model_matches(
        r"C:\models\qwen2.5-7b-instruct.gguf",
        "qwen2.5-7b-instruct"
    ));
    assert!(!model_matches("qwen2.5-3b-instruct", "qwen2.5-7b-instruct"));
    assert!(!model_matches("anything", ""));
}
