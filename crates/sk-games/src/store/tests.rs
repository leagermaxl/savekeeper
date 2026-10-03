//! `ManifestStore` against a mock HTTP server (SPEC-05 §6).

use std::collections::HashMap;
use std::fs::File;

use wiremock::matchers::{header, method, path};
use wiremock::{Match, Mock, MockServer, Request, ResponseTemplate};

use super::*;

const SAMPLE: &str = include_str!("../../../../fixtures/samples/ludusavi/manifest-sample.yaml");
const MINI: &str = include_str!("../../../../fixtures/samples/ludusavi/manifest-mini.yaml");
const SNAPSHOT_DATE: &str = "2000-01-02";

/// Matches requests without `If-None-Match`.
struct Unconditional;

impl Match for Unconditional {
    fn matches(&self, request: &Request) -> bool {
        !request.headers.contains_key("if-none-match")
    }
}

fn ok<T, E: std::fmt::Display>(r: Result<T, E>) -> T {
    r.unwrap_or_else(|e| panic!("{e}"))
}

fn games(yaml: &str) -> HashMap<String, crate::GameEntry> {
    ok(Manifest::parse(yaml.as_bytes(), ManifestSource::Cache)).games
}

fn tiny_snapshot() -> Embedded {
    let zst = ok(zstd::bulk::compress(b"Embedded Game: {}\n", 3));
    Embedded {
        zst: Box::leak(zst.into_boxed_slice()),
        date: SNAPSHOT_DATE,
    }
}

fn embedded_source() -> ManifestSource {
    ManifestSource::Embedded {
        snapshot_date: SNAPSHOT_DATE.to_owned(),
    }
}

fn store_with(server: &str, data: &Path, interval_hours: u32, auto_update: bool) -> ManifestStore {
    let mut cfg = Config::default();
    cfg.games.manifest_url = format!("{server}/manifest.yaml");
    cfg.games.update_interval_hours = interval_hours;
    cfg.games.auto_update = auto_update;
    ManifestStore::new(&cfg, data)
        .with_timeout(Duration::from_millis(500))
        .with_embedded(tiny_snapshot())
}

fn store(server: &str, data: &Path, interval_hours: u32) -> ManifestStore {
    store_with(server, data, interval_hours, true)
}

fn yaml_200(body: &str, etag: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("ETag", etag)
        .set_body_string(body)
}

/// Puts a previous download into `<data>/cache`.
fn seed_cache(data: &Path, yaml: &str, etag: &str) {
    let dir = data.join("cache");
    ok(fs::create_dir_all(&dir));
    ok(fs::write(dir.join(YAML), yaml));
    ok(fs::write(dir.join(ETAG), etag));
}

fn read(data: &Path, name: &str) -> String {
    ok(fs::read_to_string(data.join("cache").join(name)))
}

fn issue_keys(store: &ManifestStore) -> Vec<(IssueSeverity, String, String)> {
    (store.take_issues().into_iter())
        .map(|i| (i.severity, i.message_key, i.message_args["reason"].clone()))
        .collect()
}

async fn load(store: &ManifestStore, allow_network: bool) -> Arc<Manifest> {
    ok(store.load(allow_network, &CancellationToken::new()).await)
}

#[tokio::test]
async fn download_200_with_etag_is_cached_and_reused() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/manifest.yaml"))
        .and(Unconditional)
        .respond_with(yaml_200(MINI, "\"v1\""))
        .expect(1)
        .mount(&server)
        .await;
    let tmp = ok(tempfile::tempdir());
    let store = store(&server.uri(), tmp.path(), 168);

    let m = load(&store, true).await;
    assert_eq!(m.meta.source, ManifestSource::Downloaded);
    assert_eq!(m.meta.etag.as_deref(), Some("\"v1\""));
    assert!(m.meta.fetched_at.is_some());
    assert_eq!(m.games, games(MINI));
    assert_eq!(m.meta.games, m.games.len());
    assert_eq!(read(tmp.path(), YAML), MINI);
    assert_eq!(read(tmp.path(), ETAG), "\"v1\"");
    assert!(tmp.path().join("cache").join(INDEX).is_file());
    assert!(!tmp.path().join("cache").join(YAML_TMP).exists());
    assert!(issue_keys(&store).is_empty());

    // Within the update interval: no request, the cache (through the index).
    let m = load(&store, true).await;
    assert_eq!(m.meta.source, ManifestSource::Cache);
    assert_eq!(m.meta.etag.as_deref(), Some("\"v1\""));
    assert!(m.meta.fetched_at.is_some());
    assert_eq!(m.games, games(MINI));
}

#[tokio::test]
async fn not_modified_keeps_the_cache() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(header("If-None-Match", "\"v1\""))
        .respond_with(ResponseTemplate::new(304))
        .expect(1)
        .mount(&server)
        .await;
    let tmp = ok(tempfile::tempdir());
    seed_cache(tmp.path(), MINI, "\"v1\"");
    let etag_file = File::options()
        .write(true)
        .open(tmp.path().join("cache").join(ETAG));
    let old = SystemTime::now() - Duration::from_secs(3600);
    ok(ok(etag_file).set_modified(old));

    let m = load(&store(&server.uri(), tmp.path(), 0), true).await;
    assert_eq!(m.meta.source, ManifestSource::Cache);
    assert_eq!(m.games, games(MINI));
    assert_eq!(read(tmp.path(), ETAG), "\"v1\"");
    // The check time was recorded: a store with a 1 h interval does not ask again.
    let modified = ok(ok(fs::metadata(tmp.path().join("cache").join(ETAG))).modified());
    assert!(modified > old);
    let m = load(&store(&server.uri(), tmp.path(), 1), true).await;
    assert_eq!(m.meta.source, ManifestSource::Cache);
}

#[tokio::test]
async fn server_error_falls_back_to_cache_then_snapshot() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .expect(2)
        .mount(&server)
        .await;
    let tmp = ok(tempfile::tempdir());
    let store = store(&server.uri(), tmp.path(), 0);

    let m = load(&store, true).await;
    assert_eq!(m.meta.source, embedded_source());
    assert_eq!(m.meta.etag, None);
    assert_eq!(m.meta.fetched_at, None);
    assert!(m.games.contains_key("Embedded Game"));
    let offline = (
        IssueSeverity::Info,
        "issue.games.manifest_offline".into(),
        "HTTP 500".into(),
    );
    assert_eq!(issue_keys(&store), vec![offline.clone()]);
    assert!(!tmp.path().join("cache").join(YAML).exists());

    seed_cache(tmp.path(), MINI, "\"v1\"");
    let m = load(&store, true).await;
    assert_eq!(m.meta.source, ManifestSource::Cache);
    assert_eq!(m.games, games(MINI));
    assert_eq!(issue_keys(&store), vec![offline]);
}

#[tokio::test]
async fn broken_or_empty_download_keeps_the_old_cache() {
    for body in [
        "just a string",
        "Game:\n  steam:\n    id: x\n",
        "",
        "# truncated\n",
    ] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(yaml_200(body, "\"v2\""))
            .expect(1)
            .mount(&server)
            .await;
        let tmp = ok(tempfile::tempdir());
        seed_cache(tmp.path(), MINI, "\"v1\"");
        let store = store(&server.uri(), tmp.path(), 0);

        let m = load(&store, true).await;
        assert_eq!(m.meta.source, ManifestSource::Cache, "{body:?}");
        assert_eq!(m.games, games(MINI));
        assert_eq!(read(tmp.path(), YAML), MINI);
        assert_eq!(read(tmp.path(), ETAG), "\"v1\"");
        let issues = issue_keys(&store);
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert_eq!(issues[0].0, IssueSeverity::Warning);
        assert_eq!(issues[0].1, "issue.games.manifest_invalid");
    }
}

#[tokio::test]
async fn update_reports_each_outcome() {
    let server = MockServer::start().await;
    let tmp = ok(tempfile::tempdir());
    let store = store(&server.uri(), tmp.path(), 168);

    // No cache and 500: the snapshot is the fallback.
    let failing = Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount_as_scoped(&server)
        .await;
    let failed = UpdateOutcome::Failed {
        reason: "HTTP 500".into(),
        fallback: embedded_source(),
    };
    assert_eq!(ok(store.update(false).await), failed);
    drop(failing);

    let full = Mock::given(method("GET"))
        .and(Unconditional)
        .respond_with(yaml_200(MINI, "\"v1\""))
        .mount_as_scoped(&server)
        .await;
    let expected = UpdateOutcome::Updated {
        games: games(MINI).len(),
    };
    assert_eq!(ok(store.update(false).await), expected);
    // `force` downloads again although the etag is current.
    assert_eq!(ok(store.update(true).await), expected);
    drop(full);

    let _not_modified = Mock::given(method("GET"))
        .and(header("If-None-Match", "\"v1\""))
        .respond_with(ResponseTemplate::new(304))
        .mount_as_scoped(&server)
        .await;
    assert_eq!(ok(store.update(false).await), UpdateOutcome::NotModified);
    let failed = UpdateOutcome::Failed {
        reason: "HTTP 404".into(),
        fallback: ManifestSource::Cache,
    };
    // Forced, the request has no If-None-Match: no mock matches (404).
    assert_eq!(ok(store.update(true).await), failed);
}

#[tokio::test]
async fn offline_mode_and_disabled_auto_update_make_no_request() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(yaml_200(MINI, "\"v1\""))
        .expect(0)
        .mount(&server)
        .await;
    let tmp = ok(tempfile::tempdir());

    let store1 = store(&server.uri(), tmp.path(), 0);
    let m = load(&store1, false).await;
    assert_eq!(m.meta.source, embedded_source());
    let store2 = store_with(&server.uri(), tmp.path(), 0, false);
    let m = load(&store2, true).await;
    assert_eq!(m.meta.source, embedded_source());
    assert!(issue_keys(&store1).is_empty() && issue_keys(&store2).is_empty());
    // The snapshot was indexed; the next load reads the index.
    assert!(tmp.path().join("cache").join(INDEX).is_file());
    assert_eq!(
        load(&store1, false).await.games,
        games("Embedded Game: {}\n")
    );
}

#[tokio::test]
async fn stalled_server_times_out_to_the_snapshot() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(yaml_200(MINI, "\"v1\"").set_delay(Duration::from_secs(5)))
        .mount(&server)
        .await;
    let tmp = ok(tempfile::tempdir());
    let store = store(&server.uri(), tmp.path(), 0).with_timeout(Duration::from_millis(200));

    let m = load(&store, true).await;
    assert_eq!(m.meta.source, embedded_source());
    let offline = (
        IssueSeverity::Info,
        "issue.games.manifest_offline".into(),
        "timeout".into(),
    );
    assert_eq!(issue_keys(&store), vec![offline]);
}

#[tokio::test]
async fn cancel_stops_the_load() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(yaml_200(MINI, "\"v1\"").set_delay(Duration::from_secs(5)))
        .mount(&server)
        .await;
    let tmp = ok(tempfile::tempdir());
    let store = store(&server.uri(), tmp.path(), 0).with_timeout(Duration::from_secs(30));

    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        store.load(true, &cancel).await,
        Err(GamesError::Cancelled)
    ));

    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        trigger.cancel();
    });
    assert!(matches!(
        store.load(true, &cancel).await,
        Err(GamesError::Cancelled)
    ));
    assert!(!tmp.path().join("cache").join(YAML).exists());
}

#[tokio::test]
async fn index_follows_the_cached_file() {
    let tmp = ok(tempfile::tempdir());
    seed_cache(tmp.path(), MINI, "\"v1\"");
    let store = store("http://127.0.0.1:9", tmp.path(), 168);
    assert_eq!(load(&store, false).await.games, games(MINI));
    let yaml = tmp.path().join("cache").join(YAML);
    let modified = ok(ok(fs::metadata(&yaml)).modified());

    // Same size and time: the index is trusted, the YAML is not parsed.
    ok(fs::write(&yaml, "x".repeat(MINI.len())));
    ok(ok(File::options().write(true).open(&yaml)).set_modified(modified));
    assert_eq!(load(&store, false).await.games, games(MINI));

    // Another file: the index is rebuilt from it.
    ok(fs::write(&yaml, SAMPLE));
    assert_eq!(load(&store, false).await.games, games(SAMPLE));
    // Another etag invalidates the index too (the YAML now is garbage).
    let modified = ok(ok(fs::metadata(&yaml)).modified());
    ok(fs::write(&yaml, "x".repeat(SAMPLE.len())));
    ok(ok(File::options().write(true).open(&yaml)).set_modified(modified));
    ok(fs::write(tmp.path().join("cache").join(ETAG), "\"v2\""));
    let m = load(&store, false).await;
    assert_eq!(m.meta.source, embedded_source());
    let issues = issue_keys(&store);
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert_eq!(issues[0].1, "issue.games.manifest_cache_failed");
}

#[tokio::test]
async fn unwritable_cache_keeps_the_download() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(yaml_200(MINI, "\"v1\""))
        .expect(2)
        .mount(&server)
        .await;
    let tmp = ok(tempfile::tempdir());
    // `<data>/cache` is a file: nothing can be written into it.
    ok(fs::write(tmp.path().join("cache"), "not a folder"));
    let store = store(&server.uri(), tmp.path(), 0);

    let m = load(&store, true).await;
    assert_eq!(m.meta.source, ManifestSource::Downloaded);
    assert_eq!(m.meta.etag.as_deref(), Some("\"v1\""));
    assert_eq!(m.games, games(MINI));
    let issues = issue_keys(&store);
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert_eq!(issues[0].0, IssueSeverity::Warning);
    assert_eq!(issues[0].1, "issue.games.manifest_cache_failed");

    assert!(matches!(store.update(false).await, Err(GamesError::Io(_))));
}

#[tokio::test]
async fn unsaved_index_is_a_cache_warning() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(yaml_200(MINI, "\"v1\""))
        .mount(&server)
        .await;
    let cache_failed = |store: &ManifestStore| {
        let issues = issue_keys(store);
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert_eq!(issues[0].0, IssueSeverity::Warning);
        assert_eq!(issues[0].1, "issue.games.manifest_cache_failed");
    };
    // `ludusavi-index.bin` is a folder: the index cannot be replaced.
    let blocked = || {
        let tmp = ok(tempfile::tempdir());
        ok(fs::create_dir_all(tmp.path().join("cache").join(INDEX)));
        tmp
    };

    let tmp = blocked();
    let offline = store(&server.uri(), tmp.path(), 0);
    assert_eq!(load(&offline, false).await.meta.source, embedded_source());
    cache_failed(&offline);

    seed_cache(tmp.path(), MINI, "\"v1\"");
    let m = load(&offline, false).await;
    assert_eq!(
        (m.meta.source.clone(), m.games.clone()),
        (ManifestSource::Cache, games(MINI))
    );
    cache_failed(&offline);

    let tmp = blocked();
    let store = store(&server.uri(), tmp.path(), 0);
    let m = load(&store, true).await;
    assert_eq!(m.meta.source, ManifestSource::Downloaded);
    assert_eq!(read(tmp.path(), YAML), MINI);
    cache_failed(&store);
    let expected = UpdateOutcome::Updated {
        games: games(MINI).len(),
    };
    assert_eq!(ok(store.update(false).await), expected);
    cache_failed(&store);
}
