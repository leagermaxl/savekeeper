use sk_core::config::{CloudLlmKind, LocalLlmConfig};

use super::*;
use crate::mock::{MockClassifier, MockFailure};
use crate::{EnvStore, API_KEY_NAME};

fn no_secrets() -> EnvStore {
    EnvStore::new()
        .with_var(API_KEY_NAME, "KEY")
        .with_values(Vec::<(String, String)>::new())
}

#[tokio::test]
async fn healthy_mock_is_ok() {
    let mock = MockClassifier::new().with_model("m1");
    let h = classifier_health(LlmMode::Local, &mock).await;
    assert_eq!(h.status, LlmHealthStatus::Ok);
    assert_eq!(h.provider, Some("mock"));
    assert_eq!(h.model.as_deref(), Some("m1"));
    assert_eq!(h.is_local, Some(true));
    assert_eq!(h.server_version.as_deref(), Some("mock"));
    assert!(h.latency_ms.is_some());
    assert_eq!((h.error, h.pull_command), (None, None));
}

#[tokio::test]
async fn errors_map_to_statuses() {
    let cases = [
        (
            MockFailure::ModelNotFound,
            LlmHealthStatus::ModelMissing,
            false,
        ),
        (MockFailure::Unreachable, LlmHealthStatus::Unreachable, true),
        (MockFailure::Timeout, LlmHealthStatus::Unreachable, true),
        (MockFailure::Auth, LlmHealthStatus::AuthFailed, true),
        (MockFailure::Overloaded, LlmHealthStatus::Error, true),
        (MockFailure::RateLimited(None), LlmHealthStatus::Error, true),
    ];
    for (failure, status, has_error) in cases {
        let mock = MockClassifier::new().with_health_failure(failure.clone());
        let h = classifier_health(LlmMode::Cloud, &mock).await;
        assert_eq!(h.status, status, "{failure:?}");
        assert_eq!(h.error.is_some(), has_error, "{failure:?}");
        assert_eq!(h.latency_ms, None);
        assert_eq!(h.pull_command, None, "only Ollama gets the pull hint");
        assert_eq!(h.mode, LlmMode::Cloud);
    }
}

#[tokio::test]
async fn off_makes_no_classifier() {
    let cfg = LlmConfig {
        mode: LlmMode::Local,
        ..LlmConfig::default()
    };
    let h = llm_health(&cfg, LlmMode::Off, &no_secrets()).await;
    assert_eq!(h, LlmHealth::empty(LlmMode::Off, LlmHealthStatus::Off));
}

#[tokio::test]
async fn cloud_without_a_key_is_no_api_key() {
    let h = llm_health(&LlmConfig::default(), LlmMode::Cloud, &no_secrets()).await;
    assert_eq!(
        h,
        LlmHealth::empty(LlmMode::Cloud, LlmHealthStatus::NoApiKey)
    );
}

#[tokio::test]
async fn invalid_configs_are_unreachable_without_a_request() {
    let cfg = LlmConfig {
        local: LocalLlmConfig {
            endpoint: "not a url".to_owned(),
            ..LocalLlmConfig::default()
        },
        ..LlmConfig::default()
    };
    let h = llm_health(&cfg, LlmMode::Local, &no_secrets()).await;
    assert_eq!(h.status, LlmHealthStatus::Unreachable);
    assert_eq!(h.provider, None);
    assert!(h.error.is_some_and(|e| e.contains("invalid endpoint URL")));

    let mut cfg = LlmConfig::default();
    cfg.cloud.kind = CloudLlmKind::OpenaiCompat;
    let h = llm_health(&cfg, LlmMode::Cloud, &no_secrets()).await;
    assert_eq!(h.status, LlmHealthStatus::Unreachable);
}

#[test]
fn health_serializes_snake_case() {
    let h = LlmHealth::empty(LlmMode::Cloud, LlmHealthStatus::NoApiKey);
    let value = serde_json::to_value(&h).unwrap_or_default();
    assert_eq!(value["mode"], "cloud");
    assert_eq!(value["status"], "no_api_key");
    assert!(value["pull_command"].is_null());
}
