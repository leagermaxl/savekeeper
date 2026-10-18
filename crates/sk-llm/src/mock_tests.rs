use std::sync::Arc;
use std::time::{Duration, Instant};

use sk_core::model::{Category, FolderSummary};
use sk_core::template::PathTemplate;
use sk_core::CancellationToken;

use super::{MockClassifier, MockFailure};
use crate::{Classification, Classifier, ClassifyItem, LlmError};

fn template(s: &str) -> PathTemplate {
    match PathTemplate::parse(s) {
        Ok(t) => t,
        Err(e) => panic!("bad template {s}: {e}"),
    }
}

fn item(id: &str, path: &str) -> ClassifyItem {
    ClassifyItem {
        id: id.to_owned(),
        summary: FolderSummary {
            path: template(path),
            total_bytes: 1024,
            file_count: 3,
            dir_count: 1,
            max_depth: 1,
            newest_mtime: None,
            oldest_mtime: None,
            ext_histogram: Vec::new(),
            sample_names: Vec::new(),
            top_children: Vec::new(),
            markers: Vec::new(),
            truncated: false,
        },
        peek: Vec::new(),
        hint: None,
    }
}

fn answer(category: Category, app: &str) -> Classification {
    Classification {
        id: "ignored".to_owned(),
        category,
        app_guess: Some(app.to_owned()),
        importance: 0.7,
        confidence: 0.9,
        reason: "Settings of the application.".to_owned(),
        suggested_include: Vec::new(),
        suggested_exclude: vec!["logs/**".to_owned()],
        from_cache: true,
    }
}

fn obs_mock() -> MockClassifier {
    MockClassifier::new()
        .with_response(
            template("{APPDATA}/obs-studio"),
            answer(Category::AppConfig, "OBS Studio"),
        )
        .with_response(
            template("{LOCALAPPDATA}/SomeCache"),
            answer(Category::Cache, "Some"),
        )
}

#[tokio::test]
async fn answers_from_table_by_path_with_item_id() {
    let mock = obs_mock();
    let items = [
        item("a", "{LOCALAPPDATA}/SomeCache"),
        item("b", "{APPDATA}/obs-studio"),
    ];
    let out = match mock.classify(&items, &CancellationToken::new()).await {
        Ok(out) => out,
        Err(e) => panic!("classify failed: {e}"),
    };
    assert_eq!(out.len(), 2);
    let b = out.iter().find(|c| c.id == "b");
    let b = b.map(|c| (c.category, c.app_guess.clone(), c.from_cache));
    assert_eq!(
        b,
        Some((Category::AppConfig, Some("OBS Studio".to_owned()), false))
    );
    let a = out.iter().find(|c| c.id == "a").map(|c| c.category);
    assert_eq!(a, Some(Category::Cache));
    assert_eq!(mock.calls(), vec![vec!["a".to_owned(), "b".to_owned()]]);
}

#[tokio::test]
async fn unknown_paths_are_left_out() {
    let mock = obs_mock();
    let items = [
        item("a", "{APPDATA}/obs-studio"),
        item("x", "{APPDATA}/Unrelated"),
    ];
    let out = mock.classify(&items, &CancellationToken::new()).await;
    let ids: Option<Vec<String>> = out.ok().map(|v| v.into_iter().map(|c| c.id).collect());
    assert_eq!(ids, Some(vec!["a".to_owned()]));
}

#[tokio::test]
async fn queued_failures_are_returned_in_order_then_success() {
    let mock = obs_mock();
    mock.fail_next(MockFailure::RateLimited(Some(Duration::from_secs(3))));
    mock.fail_next(MockFailure::Overloaded);
    mock.fail_next(MockFailure::InvalidJson);
    let items = [item("a", "{APPDATA}/obs-studio")];
    let cancel = CancellationToken::new();

    let first = mock.classify(&items, &cancel).await;
    assert_eq!(
        first,
        Err(LlmError::RateLimited(Some(Duration::from_secs(3))))
    );
    assert_eq!(
        mock.classify(&items, &cancel).await,
        Err(LlmError::Overloaded)
    );
    assert!(matches!(
        mock.classify(&items, &cancel).await,
        Err(LlmError::BadResponse(_))
    ));
    let ok = mock.classify(&items, &cancel).await;
    assert_eq!(ok.map(|v| v.len()), Ok(1));
    assert_eq!(mock.call_count(), 4);
}

#[tokio::test]
async fn other_failures_map_to_errors() {
    let mock = MockClassifier::new().with_model("qwen2.5:7b-instruct");
    let items = [item("a", "{APPDATA}/obs-studio")];
    let cancel = CancellationToken::new();
    for (failure, expected) in [
        (MockFailure::Auth, LlmError::Auth),
        (MockFailure::Timeout, LlmError::Timeout),
        (
            MockFailure::ModelNotFound,
            LlmError::ModelNotFound("qwen2.5:7b-instruct".to_owned()),
        ),
    ] {
        mock.fail_next(failure);
        assert_eq!(mock.classify(&items, &cancel).await, Err(expected));
    }
    mock.fail_next(MockFailure::Unreachable);
    assert!(matches!(
        mock.classify(&items, &cancel).await,
        Err(LlmError::Unreachable(_))
    ));
}

#[tokio::test]
async fn batch_over_max_batch_is_rejected() {
    let mock = obs_mock().with_max_batch(2);
    let items = [
        item("a", "{APPDATA}/obs-studio"),
        item("b", "{APPDATA}/obs-studio"),
        item("c", "{APPDATA}/obs-studio"),
    ];
    let out = mock.classify(&items, &CancellationToken::new()).await;
    assert!(matches!(out, Err(LlmError::BadResponse(_))));
    let out = mock.classify(&items[..2], &CancellationToken::new()).await;
    assert_eq!(out.map(|v| v.len()), Ok(2));
}

#[tokio::test]
async fn cancelled_before_start_makes_no_call() {
    let mock = obs_mock();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let out = mock
        .classify(&[item("a", "{APPDATA}/obs-studio")], &cancel)
        .await;
    assert_eq!(out, Err(LlmError::Cancelled));
    assert_eq!(mock.call_count(), 0);
}

#[tokio::test]
async fn cancel_interrupts_delay() {
    let mock = Arc::new(obs_mock().with_delay(Duration::from_secs(30)));
    let cancel = CancellationToken::new();
    let canceller = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        canceller.cancel();
    });
    let started = Instant::now();
    let out = mock
        .classify(&[item("a", "{APPDATA}/obs-studio")], &cancel)
        .await;
    assert_eq!(out, Err(LlmError::Cancelled));
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[tokio::test]
async fn delay_is_applied() {
    let mock = obs_mock().with_delay(Duration::from_millis(30));
    let started = Instant::now();
    let out = mock
        .classify(
            &[item("a", "{APPDATA}/obs-studio")],
            &CancellationToken::new(),
        )
        .await;
    assert_eq!(out.map(|v| v.len()), Ok(1));
    assert!(started.elapsed() >= Duration::from_millis(30));
}

#[tokio::test]
async fn info_and_health() {
    let mock = MockClassifier::new()
        .with_model("m")
        .with_local(false)
        .with_max_batch(20)
        .with_max_concurrency(4);
    let info = mock.info();
    assert_eq!(
        (info.provider, info.model.as_str(), info.is_local),
        ("mock", "m", false)
    );
    assert_eq!((info.max_batch, info.max_concurrency), (20, 4));
    assert_eq!(mock.health().await.map(|h| h.model_available), Ok(true));

    let down = MockClassifier::new().with_health_failure(MockFailure::Unreachable);
    assert!(matches!(down.health().await, Err(LlmError::Unreachable(_))));
}

#[test]
fn usable_as_trait_object() {
    let classifier: Arc<dyn Classifier> = Arc::new(MockClassifier::default());
    assert_eq!(classifier.info().provider, "mock");
}

#[tokio::test]
async fn truncation_refusal_and_usage() {
    let per_call = crate::UsageStats {
        requests: 1,
        input_tokens: 10,
        ..crate::UsageStats::default()
    };
    let mock = obs_mock()
        .with_truncate_above(1)
        .with_usage_per_call(per_call);
    let cancel = CancellationToken::new();
    let two = [
        item("a", "{LOCALAPPDATA}/SomeCache"),
        item("b", "{APPDATA}/obs-studio"),
    ];
    assert_eq!(mock.classify(&two, &cancel).await, Err(LlmError::MaxTokens));
    assert_eq!(
        mock.classify(&two[..1], &cancel).await.map(|v| v.len()),
        Ok(1)
    );
    mock.fail_next(MockFailure::Refusal);
    mock.fail_next(MockFailure::MaxTokens);
    assert_eq!(
        mock.classify(&two[..1], &cancel).await,
        Err(LlmError::Refusal)
    );
    assert_eq!(
        mock.classify(&two[..1], &cancel).await,
        Err(LlmError::MaxTokens)
    );
    assert_eq!((mock.usage().requests, mock.usage().input_tokens), (4, 40));
}
