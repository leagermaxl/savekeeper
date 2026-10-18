//! `ClassifyRunner` error paths with `MockClassifier` (SPEC-08 §4.6, §4.12,
//! §5, §6.2): retries, backoff, halving, refusal, missing ids, provider
//! failure, cancellation, phase budget. Backoff tests run on paused time.

use std::sync::Arc;
use std::time::Duration;

use sk_core::events::Event;
use sk_core::CancellationToken;

use super::tests::{arg, find, item, local_cfg, mock_for, run, runner, temp_dir};
use super::*;
use crate::mock::MockFailure;

fn three() -> Vec<ClassifyItem> {
    vec![item("a", "Alpha"), item("b", "Beta"), item("c", "Gamma")]
}

#[tokio::test(start_paused = true)]
async fn rate_limit_waits_for_retry_after() {
    let dir = temp_dir();
    let mock = Arc::new(mock_for(&three()));
    mock.fail_next(MockFailure::RateLimited(Some(Duration::from_secs(5))));
    let start = tokio::time::Instant::now();
    let (out, _) = run(&runner(&mock, &dir, local_cfg()), three()).await;
    let waited = start.elapsed();
    assert_eq!(mock.call_count(), 2);
    assert_eq!(out.results.len(), 3);
    assert!(out.issues.is_empty(), "{:?}", out.issues);
    assert!(waited >= Duration::from_secs(5) && waited < Duration::from_secs(6));
}

#[tokio::test(start_paused = true)]
async fn retry_after_is_capped_at_a_minute() {
    let dir = temp_dir();
    let mock = Arc::new(mock_for(&three()));
    mock.fail_next(MockFailure::RateLimited(Some(Duration::from_secs(3600))));
    let start = tokio::time::Instant::now();
    let (out, _) = run(&runner(&mock, &dir, local_cfg()), three()).await;
    let waited = start.elapsed();
    assert_eq!(out.results.len(), 3);
    assert!(waited >= MAX_RETRY_AFTER && waited < MAX_RETRY_AFTER + Duration::from_secs(1));
}

#[tokio::test(start_paused = true)]
async fn overload_backs_off_exponentially_then_gives_up() {
    let dir = temp_dir();
    let mock = Arc::new(mock_for(&three()));
    for _ in 0..=MAX_RETRIES {
        mock.fail_next(MockFailure::Overloaded);
    }
    let start = tokio::time::Instant::now();
    let (out, _) = run(&runner(&mock, &dir, local_cfg()), three()).await;
    let waited = start.elapsed().as_secs_f64();
    // 1 + 2 + 4 s, ±20%.
    assert!((5.6..=8.4).contains(&waited), "{waited}");
    assert_eq!(mock.call_count(), 4);
    assert!(out.results.is_empty());
    let failed = find(&out, "llm.request_failed");
    assert_eq!(arg(failed, "count").as_deref(), Some("3"));
    assert_eq!(arg(failed, "error").as_deref(), Some("overloaded"));
}

#[tokio::test(start_paused = true)]
async fn transient_errors_recover() {
    let dir = temp_dir();
    let mock = Arc::new(mock_for(&three()));
    mock.fail_next(MockFailure::Timeout);
    mock.fail_next(MockFailure::Unreachable);
    let (out, _) = run(&runner(&mock, &dir, local_cfg()), three()).await;
    assert_eq!(mock.call_count(), 3);
    assert_eq!(out.results.len(), 3);
}

#[tokio::test]
async fn invalid_json_is_retried_once() {
    let dir = temp_dir();
    let mock = Arc::new(mock_for(&three()));
    mock.fail_next(MockFailure::InvalidJson);
    let (out, _) = run(&runner(&mock, &dir, local_cfg()), three()).await;
    assert_eq!((mock.call_count(), out.results.len()), (2, 3));

    let dir = temp_dir();
    let mock = Arc::new(mock_for(&three()));
    mock.fail_next(MockFailure::InvalidJson);
    mock.fail_next(MockFailure::InvalidJson);
    let (out, _) = run(&runner(&mock, &dir, local_cfg()), three()).await;
    assert_eq!((mock.call_count(), out.results.len()), (2, 0));
    assert_eq!(
        arg(find(&out, "llm.bad_json"), "count").as_deref(),
        Some("3")
    );
}

#[tokio::test]
async fn max_tokens_halves_the_batch_down_to_one_item() {
    let dir = temp_dir();
    let items = vec![
        item("a", "Alpha"),
        item("b", "Beta"),
        item("c", "Charlie"),
        item("d", "Delta"),
    ];
    let mock = Arc::new(mock_for(&items).with_truncate_above(1));
    let (out, _) = run(&runner(&mock, &dir, local_cfg()), items).await;
    assert_eq!(
        mock.calls(),
        [
            vec!["a", "b", "c", "d"],
            vec!["a", "b"],
            vec!["a"],
            vec!["b"],
            vec!["c", "d"],
            vec!["c"],
            vec!["d"],
        ]
    );
    assert_eq!(out.results.len(), 4);
    assert!(out.issues.is_empty(), "{:?}", out.issues);
}

#[tokio::test]
async fn max_tokens_for_one_item_is_an_issue() {
    let dir = temp_dir();
    let items = vec![item("a", "Alpha")];
    let mock = Arc::new(mock_for(&items).with_truncate_above(0));
    let (out, _) = run(&runner(&mock, &dir, local_cfg()), items).await;
    assert_eq!(mock.call_count(), 1);
    assert!(find(&out, "llm.truncated").is_some());
}

#[tokio::test]
async fn refusal_leaves_items_unknown_without_retry() {
    let dir = temp_dir();
    let mock = Arc::new(mock_for(&three()));
    mock.fail_next(MockFailure::Refusal);
    let (out, _) = run(&runner(&mock, &dir, local_cfg()), three()).await;
    assert_eq!(mock.call_count(), 1);
    assert!(out.results.is_empty());
    assert_eq!(
        arg(find(&out, "llm.refusal"), "count").as_deref(),
        Some("3")
    );
    // Nothing was cached: the next run asks again.
    let again = Arc::new(mock_for(&three()));
    let (out, _) = run(&runner(&again, &dir, local_cfg()), three()).await;
    assert_eq!((again.call_count(), out.results.len()), (1, 3));
}

#[tokio::test]
async fn missing_ids_are_asked_again_in_a_mini_batch() {
    let dir = temp_dir();
    let items = three();
    // No answer for `b`.
    let mock = Arc::new(mock_for(&[items[0].clone(), items[2].clone()]));
    let (out, _) = run(&runner(&mock, &dir, local_cfg()), items).await;
    assert_eq!(mock.calls(), [vec!["a", "b", "c"], vec!["b"]]);
    assert_eq!(out.results.len(), 2);
    assert_eq!(
        arg(find(&out, "llm.missing_result"), "count").as_deref(),
        Some("1")
    );
}

#[tokio::test]
async fn three_failed_batches_in_a_row_stop_the_phase() {
    let dir = temp_dir();
    let items: Vec<ClassifyItem> = (0..6)
        .map(|i| item(&format!("i{i}"), &format!("App{i}")))
        .collect();
    let mock = Arc::new(mock_for(&items).with_max_batch(1));
    for _ in 0..6 {
        mock.fail_next(MockFailure::Auth);
    }
    let (out, _) = run(&runner(&mock, &dir, local_cfg()), items).await;
    assert_eq!(mock.call_count(), 3, "Auth is not retried");
    assert!(find(&out, "llm.provider_failed").is_some());
    assert_eq!(
        arg(find(&out, "llm.auth_failed"), "count").as_deref(),
        Some("3")
    );
}

#[tokio::test]
async fn a_good_batch_resets_the_failure_count() {
    let dir = temp_dir();
    let items: Vec<ClassifyItem> = (0..5)
        .map(|i| item(&format!("i{i}"), &format!("App{i}")))
        .collect();
    let mock = Arc::new(mock_for(&items).with_max_batch(1));
    for f in [
        MockFailure::Auth,
        MockFailure::Auth,
        MockFailure::ModelNotFound,
    ] {
        mock.fail_next(f);
    }
    let (out, _) = run(&runner(&mock, &dir, local_cfg()), items).await;
    assert_eq!(mock.call_count(), 5);
    assert!(find(&out, "llm.provider_failed").is_none());
    assert_eq!(out.results.len(), 2);
    assert!(find(&out, "llm.model_missing").is_some());
}

#[tokio::test]
async fn cancel_stops_within_a_second_and_keeps_finished_batches() {
    let dir = temp_dir();
    let items = three();
    let mock = Arc::new(
        mock_for(&items)
            .with_max_batch(1)
            .with_delay(Duration::from_millis(50)),
    );
    let r = runner(&mock, &dir, local_cfg());
    let cancel = CancellationToken::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let watcher = {
        let cancel = cancel.clone();
        async move {
            // Cancel as soon as the first batch is done.
            while let Some(e) = rx.recv().await {
                if matches!(e, Event::Progress { done, .. } if done >= 1) {
                    cancel.cancel();
                    break;
                }
            }
        }
    };
    let (out, ()) = tokio::join!(r.run(items.clone(), &tx, &cancel), watcher);
    assert_eq!(out.results.len(), 1, "partial output");
    drop(r);
    let cache = LlmCache::open(&LlmCache::file_path(dir.path())).unwrap();
    assert_eq!(cache.len(), 1, "finished batch is in the cache");

    // A long request is dropped at once.
    let slow = Arc::new(mock_for(&items).with_delay(Duration::from_secs(30)));
    let r = runner(&slow, &temp_dir(), local_cfg());
    let cancel = CancellationToken::new();
    let trigger = {
        let cancel = cancel.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel.cancel();
        }
    };
    let start = std::time::Instant::now();
    let (out, ()) = tokio::join!(r.run(items, &tx, &cancel), trigger);
    assert!(start.elapsed() < Duration::from_secs(1));
    assert!(out.results.is_empty());
}

#[tokio::test(start_paused = true)]
async fn phase_budget_stops_a_hanging_provider() {
    let dir = temp_dir();
    let mock = Arc::new(mock_for(&three()).with_delay(Duration::from_secs(3600)));
    let mut cfg = local_cfg();
    cfg.local.timeout_s = 1;
    let start = tokio::time::Instant::now();
    let (out, _) = run(&runner(&mock, &dir, cfg), three()).await;
    let waited = start.elapsed();
    assert!(waited >= MIN_PHASE_BUDGET && waited < MIN_PHASE_BUDGET + Duration::from_secs(1));
    assert_eq!(
        arg(find(&out, "llm.phase_timeout"), "budget_s").as_deref(),
        Some("60")
    );
}

#[tokio::test]
async fn cache_write_failure_is_an_issue() {
    let dir = temp_dir();
    let mock = Arc::new(mock_for(&three()));
    let r = runner(&mock, &dir, local_cfg());
    // The cache folder is a file: appending fails.
    std::fs::write(dir.path().join("cache"), b"").unwrap();
    let (out, _) = run(&r, three()).await;
    assert_eq!(out.results.len(), 3, "results are still returned");
    assert!(find(&out, "llm.cache_error").is_some(), "{:?}", out.issues);
}

#[test]
fn backoff_doubles_with_jitter() {
    for retry in 0..3u32 {
        let d = exec::retry_delay(&LlmError::Overloaded, retry).as_secs_f64();
        let base = f64::from(1u32 << retry);
        assert!((base * 0.8..=base * 1.2).contains(&d), "{retry}: {d}");
    }
    let none = exec::retry_delay(&LlmError::RateLimited(None), 0).as_secs_f64();
    assert!((0.8..=1.2).contains(&none));
}
