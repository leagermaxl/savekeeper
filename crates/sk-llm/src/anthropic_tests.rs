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

fn config(model: &str) -> CloudLlmConfig {
    CloudLlmConfig {
        model: model.to_owned(),
        ..CloudLlmConfig::default()
    }
}

fn classifier(model: &str) -> AnthropicClassifier {
    match AnthropicClassifier::new(&config(model), "sk-ant-test-key-0123456789") {
        Ok(c) => c,
        Err(e) => panic!("classifier: {e}"),
    }
}

fn body(model: &str) -> Value {
    match classifier(model).request_body(&[item("f1"), item("f2")]) {
        Ok(b) => b,
        Err(e) => panic!("body: {e}"),
    }
}

fn schema() -> Value {
    serde_json::from_str(RESPONSE_SCHEMA).unwrap_or(Value::Null)
}

#[test]
fn haiku_body_matches_the_spec() {
    let mut body = body("claude-haiku-4-5");
    assert_eq!(body["system"][0]["text"], json!(SYSTEM_PROMPT));
    assert_eq!(body["output_config"]["format"]["schema"], schema());
    assert!(
        body["output_config"].get("effort").is_none(),
        "Haiku 4.5 rejects effort"
    );
    body["system"][0]["text"] = json!("<SYSTEM_PROMPT>");
    body["output_config"]["format"]["schema"] = json!("<RESPONSE_SCHEMA>");
    insta::assert_json_snapshot!("anthropic_request_body", body);
}

#[test]
fn sonnet_5_gets_low_effort() {
    let body = body("claude-sonnet-5");
    assert_eq!(body["output_config"]["effort"], json!("low"));
    assert_eq!(
        body["output_config"]["format"]["type"],
        json!("json_schema")
    );
    assert!(supports_effort("claude-sonnet-5-5"));
    assert!(!supports_effort("claude-haiku-4-5"));
    assert!(!supports_effort("claude-sonnet-4-6"));
}

/// Every object in the sent schema has `additionalProperties: false`, and no
/// numeric or string constraints are present (§4.2.3, §6.1).
fn check_schema(v: &Value, at: &str) {
    match v {
        Value::Object(map) => {
            for banned in [
                "minimum",
                "maximum",
                "exclusiveMinimum",
                "exclusiveMaximum",
                "maxLength",
                "minLength",
            ] {
                assert!(!map.contains_key(banned), "{banned} at {at}");
            }
            if map.get("type") == Some(&json!("object")) {
                assert_eq!(
                    map.get("additionalProperties"),
                    Some(&json!(false)),
                    "additionalProperties at {at}"
                );
            }
            for (k, child) in map {
                check_schema(child, &format!("{at}/{k}"));
            }
        }
        Value::Array(list) => {
            for (i, child) in list.iter().enumerate() {
                check_schema(child, &format!("{at}/{i}"));
            }
        }
        _ => {}
    }
}

#[test]
fn sent_schema_has_no_unsupported_constraints() {
    let body = body("claude-haiku-4-5");
    let sent = &body["output_config"]["format"]["schema"];
    assert!(sent.is_object());
    check_schema(sent, "");
}

#[test]
fn info_comes_from_the_config() {
    let info = classifier("claude-haiku-4-5").info();
    assert_eq!(info.provider, "anthropic");
    assert_eq!(info.model, "claude-haiku-4-5");
    assert!(!info.is_local);
    assert_eq!((info.max_batch, info.max_concurrency), (20, 4));
    let zero = CloudLlmConfig {
        max_batch: 0,
        max_concurrency: 0,
        ..config("claude-haiku-4-5")
    };
    let c = AnthropicClassifier::new(&zero, "k");
    let info = c.map(|c| c.info());
    assert!(matches!(
        info,
        Ok(ClassifierInfo {
            max_batch: 1,
            max_concurrency: 1,
            ..
        })
    ));
}

#[test]
fn api_key_is_required_and_not_shown_by_debug() {
    let c = classifier("claude-haiku-4-5");
    let text = format!("{c:?}");
    assert!(!text.contains("test-key"), "{text}");
    assert!(text.contains("REDACTED"), "{text}");
    let cfg = config("claude-haiku-4-5");
    assert!(matches!(
        AnthropicClassifier::new(&cfg, "  "),
        Err(LlmError::Auth)
    ));
    assert!(matches!(
        AnthropicClassifier::new(&cfg, "bad\nkey"),
        Err(LlmError::Auth)
    ));
}

#[test]
fn invalid_endpoint_is_unreachable() {
    assert!(matches!(
        classifier("claude-haiku-4-5").with_endpoint("api.anthropic.com"),
        Err(LlmError::Unreachable(_))
    ));
}

#[test]
fn stop_reasons_map_to_errors() {
    assert_eq!(check_stop_reason(Some("end_turn")), Ok(()));
    assert_eq!(
        check_stop_reason(Some("max_tokens")),
        Err(LlmError::MaxTokens)
    );
    assert_eq!(
        check_stop_reason(Some("model_context_window_exceeded")),
        Err(LlmError::MaxTokens)
    );
    assert_eq!(check_stop_reason(Some("refusal")), Err(LlmError::Refusal));
    assert!(matches!(
        check_stop_reason(Some("tool_use")),
        Err(LlmError::BadResponse(_))
    ));
    assert!(matches!(
        check_stop_reason(None),
        Err(LlmError::BadResponse(_))
    ));
}
