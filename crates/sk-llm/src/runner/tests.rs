//! `ClassifyRunner` with `MockClassifier` (SPEC-08 §6.2): selection, order,
//! cache, dedupe, progress, privacy, preview, setup.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use sk_core::env::Environment;
use sk_core::events::{Event, ScanPhase};
use sk_core::model::{Category, FolderSummary, ScanIssue};
use sk_core::template::PathTemplate;
use sk_core::CancellationToken;
use tempfile::TempDir;
use time::{Duration, OffsetDateTime};

use super::*;
use crate::mock::{MockClassifier, MockFailure};
use crate::{CloudLlmKind, ContentPeek, EnvStore, LocalLlmKind};

/// A fresh temporary folder under `<workspace>/target/tmp`.
pub(super) fn temp_dir() -> TempDir {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
    fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new()
        .prefix("sk-llm-runner-")
        .tempdir_in(&base)
        .unwrap()
}

pub(super) fn template(s: &str) -> PathTemplate {
    PathTemplate::parse(s).unwrap()
}

/// An item at `{APPDATA}\<name>`, changed `age_days` ago.
pub(super) fn item_sized(id: &str, name: &str, bytes: u64, age_days: i64) -> ClassifyItem {
    ClassifyItem {
        id: id.to_owned(),
        summary: FolderSummary {
            path: template(&format!(r"{{APPDATA}}\{name}")),
            total_bytes: bytes,
            file_count: 3,
            dir_count: 1,
            max_depth: 1,
            newest_mtime: Some(OffsetDateTime::now_utc() - Duration::days(age_days)),
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

pub(super) fn item(id: &str, name: &str) -> ClassifyItem {
    item_sized(id, name, 1024, 1)
}

pub(super) fn answer(app: &str) -> Classification {
    Classification {
        id: "ignored".to_owned(),
        category: Category::AppConfig,
        app_guess: Some(app.to_owned()),
        importance: 0.6,
        confidence: 0.9,
        reason: "Settings.".to_owned(),
        suggested_include: Vec::new(),
        suggested_exclude: Vec::new(),
        from_cache: false,
    }
}

/// A mock that answers every item of `items` (by path).
pub(super) fn mock_for(items: &[ClassifyItem]) -> MockClassifier {
    items.iter().fold(MockClassifier::new(), |m, i| {
        m.with_response(i.summary.path.clone(), answer(i.summary.path.as_str()))
    })
}

pub(super) fn local_cfg() -> LlmConfig {
    LlmConfig {
        mode: LlmMode::Local,
        ..LlmConfig::default()
    }
}

pub(super) fn runner(mock: &Arc<MockClassifier>, dir: &TempDir, cfg: LlmConfig) -> ClassifyRunner {
    let cache = LlmCache::open(&LlmCache::file_path(dir.path())).unwrap();
    let classifier: Arc<dyn Classifier> = mock.clone();
    ClassifyRunner::new(classifier, cache, cfg)
}

pub(super) async fn run(
    runner: &ClassifyRunner,
    items: Vec<ClassifyItem>,
) -> (RunOutput, Vec<Event>) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let out = runner.run(items, &tx, &CancellationToken::new()).await;
    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }
    (out, events)
}

pub(super) fn find<'a>(out: &'a RunOutput, key: &str) -> Option<&'a ScanIssue> {
    out.issues.iter().find(|i| i.message_key == key)
}

pub(super) fn arg(issue: Option<&ScanIssue>, name: &str) -> Option<String> {
    issue.and_then(|i| i.message_args.get(name).cloned())
}

fn ids(out: &RunOutput) -> Vec<&str> {
    out.results.iter().map(|r| r.id.as_str()).collect()
}

#[tokio::test]
async fn classifies_in_batches_then_second_run_is_all_cache() {
    let dir = temp_dir();
    let items = vec![item("c", "Gamma"), item("a", "Alpha"), item("b", "Beta")];
    let mock = Arc::new(mock_for(&items).with_max_batch(2));
    let (out, _) = run(&runner(&mock, &dir, local_cfg()), items.clone()).await;
    assert_eq!(ids(&out), ["c", "a", "b"], "results in input order");
    assert!(out.results.iter().all(|r| !r.from_cache));
    assert!(out.issues.is_empty(), "{:?}", out.issues);
    // Sorted by path, chunked by max_batch.
    assert_eq!(mock.calls(), [vec!["a", "b"], vec!["c"]]);

    // A new runner over the same file: 100% from the cache, 0 requests.
    let again = Arc::new(mock_for(&items).with_max_batch(2));
    let second = runner(&again, &dir, local_cfg());
    let (out, _) = run(&second, items.clone()).await;
    assert_eq!(again.call_count(), 0);
    assert_eq!(ids(&out), ["c", "a", "b"]);
    assert!(out.results.iter().all(|r| r.from_cache));
    assert_eq!(second.preview(&items).item_count, 0);
}

#[tokio::test]
async fn request_order_is_deterministic() {
    let items = vec![
        item("d", "Delta"),
        item("b", "Beta"),
        item("e", "Echo"),
        item("a", "Alpha"),
        item("c", "Charlie"),
    ];
    let mut seen = Vec::new();
    for shuffled in [items.clone(), items.iter().rev().cloned().collect()] {
        let dir = temp_dir();
        let mock = Arc::new(mock_for(&items).with_max_batch(2));
        run(&runner(&mock, &dir, local_cfg()), shuffled).await;
        seen.push(mock.calls());
    }
    assert_eq!(seen[0], [vec!["a", "b"], vec!["c", "d"], vec!["e"]]);
    assert_eq!(seen[0], seen[1]);
}

#[tokio::test]
async fn max_items_per_scan_keeps_the_biggest_recent_folders() {
    let dir = temp_dir();
    let gib = 1024 * 1024 * 1024;
    let items = vec![
        item_sized("small", "Small", 10, 1),
        item_sized("big-old", "BigOld", 10 * gib, 3650),
        item_sized("big-new", "BigNew", 10 * gib, 1),
        // Counted as 50 GB, halved by age: still ahead of `big-new`.
        item_sized("huge-old", "HugeOld", 900 * gib, 400),
        item_sized("mid-new", "MidNew", gib, 0),
    ];
    let mock = Arc::new(mock_for(&items));
    let cfg = LlmConfig {
        max_items_per_scan: 2,
        ..local_cfg()
    };
    let (out, _) = run(&runner(&mock, &dir, cfg), items).await;
    assert_eq!(mock.calls(), [vec!["big-new", "huge-old"]]);
    let limit = find(&out, "llm.limit_reached");
    assert_eq!(arg(limit, "skipped").as_deref(), Some("3"));
    assert_eq!(arg(limit, "limit").as_deref(), Some("2"));
}

#[test]
fn priority_caps_size_and_decays_with_age() {
    let now = OffsetDateTime::now_utc();
    let gib = 1024 * 1024 * 1024;
    let new = item_sized("a", "A", 50 * gib, 0).summary;
    let huge = item_sized("b", "B", 500 * gib, 0).summary;
    let year = item_sized("c", "C", 50 * gib, 365).summary;
    let p = |s: &FolderSummary| selection_priority(s, now);
    assert!((p(&new) - p(&huge)).abs() < 1e3);
    assert!((p(&year) / p(&new) - 0.5).abs() < 0.01);
    let mut none = new.clone();
    none.newest_mtime = None;
    assert!(p(&none).abs() < f64::EPSILON);
}

#[tokio::test]
async fn same_summary_is_one_request_with_the_answer_for_every_id() {
    let dir = temp_dir();
    let items = vec![item("x", "Same"), item("y", "Same"), item("z", "Other")];
    let mock = Arc::new(mock_for(&items));
    let (out, _) = run(&runner(&mock, &dir, local_cfg()), items).await;
    assert_eq!(mock.calls(), [vec!["z", "x"]]);
    assert_eq!(ids(&out), ["x", "y", "z"]);
    assert_eq!(out.results[0].app_guess, out.results[1].app_guess);
}

#[tokio::test]
async fn progress_counts_items_of_the_classify_phase() {
    let dir = temp_dir();
    let items = vec![item("a", "Alpha"), item("b", "Beta"), item("c", "Gamma")];
    let mock = Arc::new(mock_for(&items).with_max_batch(1));
    let (_, events) = run(&runner(&mock, &dir, local_cfg()), items).await;
    let progress: Vec<(u64, Option<u64>, Option<String>)> = events
        .into_iter()
        .filter_map(|e| match e {
            Event::Progress {
                phase: ScanPhase::Classify,
                done,
                total,
                current,
            } => Some((done, total, current)),
            _ => None,
        })
        .collect();
    let done: Vec<u64> = progress.iter().map(|p| p.0).collect();
    assert_eq!(done, [0, 1, 2, 3]);
    assert!(progress.iter().all(|p| p.1 == Some(3)));
    assert_eq!(progress[1].2.as_deref(), Some(r"{APPDATA}\Alpha"));
}

#[tokio::test]
async fn privacy_check_blocks_items_with_the_user_name() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    let mut leaky = item("leak", "Leaky");
    leaky.summary.sample_names = vec![format!("{}/notes.txt", env.user_name)];
    let items = vec![leaky, item("ok", "Fine")];
    let mock = Arc::new(mock_for(&items));
    let (out, _) = run(&runner(&mock, &dir, local_cfg()).with_env(env), items).await;
    assert_eq!(mock.calls(), [vec!["ok"]]);
    let blocked = find(&out, "llm.privacy_blocked");
    assert_eq!(
        blocked.and_then(|i| i.path.clone()).as_deref(),
        Some(r"{APPDATA}\Leaky")
    );
    assert_eq!(
        arg(blocked, "reason").as_deref(),
        Some("contains the user name")
    );
}

#[tokio::test]
async fn without_env_paths_are_still_blocked() {
    let dir = temp_dir();
    let mut leaky = item("leak", "Leaky");
    leaky.summary.sample_names = vec![r"C:\Users\someone\a.txt".to_owned()];
    let mock = Arc::new(mock_for(std::slice::from_ref(&leaky)));
    let (out, _) = run(&runner(&mock, &dir, local_cfg()), vec![leaky]).await;
    assert_eq!(mock.call_count(), 0);
    assert!(find(&out, "llm.privacy_blocked").is_some());
}

#[test]
fn peek_is_sent_only_when_allowed_for_the_kind_of_provider() {
    let dir = temp_dir();
    let mut it = item("a", "Alpha");
    it.peek = vec![ContentPeek {
        file: "a.ini".to_owned(),
        head: "x=1".to_owned(),
    }];
    let has_peek = |runner: &ClassifyRunner| {
        runner.preview(std::slice::from_ref(&it)).sample[0]
            .get("peek")
            .is_some()
    };
    let cloud = Arc::new(MockClassifier::new().with_local(false));
    let local = Arc::new(MockClassifier::new());
    let both = LlmConfig {
        allow_content_peek: true,
        allow_content_peek_cloud: false,
        ..local_cfg()
    };
    assert!(has_peek(&runner(&local, &dir, both.clone())));
    // Decided by `is_local`, not by the config section.
    assert!(!has_peek(&runner(&cloud, &dir, both.clone())));
    let cloud_ok = LlmConfig {
        allow_content_peek_cloud: true,
        ..both
    };
    assert!(has_peek(&runner(&cloud, &dir, cloud_ok)));
    assert!(!has_peek(&runner(&local, &dir, local_cfg())));
}

#[tokio::test]
async fn health_failures_skip_the_phase() {
    let cases = [
        (MockFailure::Unreachable, "llm.unreachable"),
        (MockFailure::ModelNotFound, "llm.model_missing"),
        (MockFailure::Auth, "llm.auth_failed"),
        (MockFailure::Timeout, "llm.unreachable"),
    ];
    for (failure, key) in cases {
        let dir = temp_dir();
        let items = vec![item("a", "Alpha")];
        let mock = Arc::new(mock_for(&items).with_health_failure(failure));
        let (out, _) = run(&runner(&mock, &dir, local_cfg()), items).await;
        assert_eq!(mock.call_count(), 0, "{key}");
        assert!(find(&out, key).is_some(), "{key}: {:?}", out.issues);
        assert!(out.results.is_empty());
    }
}

#[tokio::test]
async fn usage_is_the_difference_of_the_run() {
    let dir = temp_dir();
    let items = vec![item("a", "Alpha"), item("b", "Beta")];
    let per_call = UsageStats {
        requests: 1,
        input_tokens: 600,
        output_tokens: 120,
        ..UsageStats::default()
    };
    let mock = Arc::new(
        mock_for(&items)
            .with_max_batch(1)
            .with_usage_per_call(per_call),
    );
    let r = runner(&mock, &dir, local_cfg());
    let (out, _) = run(&r, items.clone()).await;
    assert_eq!((out.usage.requests, out.usage.input_tokens), (2, 1200));
    let (out, _) = run(&r, items).await;
    assert_eq!(out.usage, UsageStats::default(), "all from cache");
}

#[test]
fn from_config_builds_the_configured_provider() {
    let dir = temp_dir();
    let no_secrets = EnvStore::new().with_values(Vec::<(String, String)>::new());
    let off = LlmConfig::default();
    assert!(ClassifyRunner::from_config(&off, dir.path(), &no_secrets)
        .unwrap()
        .is_none());

    let local = ClassifyRunner::from_config(&local_cfg(), dir.path(), &no_secrets).unwrap();
    let local = local.unwrap();
    assert_eq!(local.info().provider, "ollama");
    assert!(!local.needs_cloud_confirmation());
    assert!(local.estimate_cost(10).is_none());
    let mut compat = local_cfg();
    compat.local.kind = LocalLlmKind::OpenaiCompat;
    compat.local.endpoint = "https://llm.example.org".to_owned();
    let compat = ClassifyRunner::from_config(&compat, dir.path(), &no_secrets).unwrap();
    // An external URL in the `local` section follows the cloud rules.
    assert!(compat.unwrap().needs_cloud_confirmation());

    let cloud = LlmConfig {
        mode: LlmMode::Cloud,
        ..LlmConfig::default()
    };
    assert_eq!(
        ClassifyRunner::from_config(&cloud, dir.path(), &no_secrets).err(),
        Some(LlmError::Auth)
    );
    let secrets = EnvStore::for_cloud(&cloud.cloud).with_values([("ANTHROPIC_API_KEY", "sk-test")]);
    let runner = ClassifyRunner::from_config(&cloud, dir.path(), &secrets).unwrap();
    let runner = runner.unwrap();
    assert!(runner.needs_cloud_confirmation());
    let estimate = runner.estimate_cost(300).unwrap();
    assert!((estimate.usd - 0.36).abs() < 1e-9);
    let preview = runner.preview(&[item("a", "Alpha")]);
    assert_eq!(preview.item_count, 1);
    let one = preview.est_cost_usd.unwrap();
    assert!((one - 0.0012).abs() < 1e-12, "{one}");

    let mut compat_cloud = cloud;
    compat_cloud.cloud.kind = CloudLlmKind::OpenaiCompat;
    assert!(matches!(
        ClassifyRunner::from_config(&compat_cloud, dir.path(), &secrets),
        Err(LlmError::Unreachable(_))
    ));
}

#[tokio::test]
async fn unreadable_cache_is_an_issue_not_a_failure() {
    let dir = temp_dir();
    // The cache file is a folder: it cannot be read.
    fs::create_dir_all(LlmCache::file_path(dir.path())).unwrap();
    let no_secrets = EnvStore::new().with_values(Vec::<(String, String)>::new());
    let runner = ClassifyRunner::from_config(&local_cfg(), dir.path(), &no_secrets).unwrap();
    let (out, _) = run(&runner.unwrap(), Vec::new()).await;
    assert!(find(&out, "llm.cache_error").is_some(), "{:?}", out.issues);
}
