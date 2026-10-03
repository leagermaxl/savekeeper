//! `ManifestStore` with the real embedded snapshot (SPEC-05 FR-05-02, NFR-05-01),
//! or without one when the feature `embedded-manifest` is off (FR-05-11).
//!
//! The index of the full manifest is several MB, so it is written under
//! `target/` (`CARGO_TARGET_TMPDIR`), not into the system temp folder.

use sk_core::config::Config;
use sk_core::CancellationToken;
use sk_games::{ManifestSource, ManifestStore};

#[cfg(feature = "embedded-manifest")]
#[tokio::test]
async fn real_snapshot_round_trips_through_the_index() {
    let tmp = tempfile::Builder::new()
        .prefix("manifest-store-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .unwrap_or_else(|e| panic!("{e}"));
    let store = ManifestStore::new(&Config::default(), tmp.path());
    let cancel = CancellationToken::new();

    let first = store
        .load(false, &cancel)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let ManifestSource::Embedded { snapshot_date } = &first.meta.source else {
        panic!("{:?}", first.meta.source);
    };
    assert_eq!(snapshot_date.len(), 10, "{snapshot_date}");
    assert!(first.meta.games > 10_000, "{} games", first.meta.games);
    let index = tmp.path().join("cache").join("ludusavi-index.bin");
    assert!(index.is_file());

    // The second load reads the index instead of the YAML.
    let second = store
        .load(false, &cancel)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(second.games, first.games);
    assert_eq!(second.meta, first.meta);
}

/// Without the snapshot, an offline load with no cache has no manifest.
#[cfg(not(feature = "embedded-manifest"))]
#[tokio::test]
async fn without_the_feature_there_is_no_snapshot() {
    let tmp = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let store = ManifestStore::new(&Config::default(), tmp.path());
    let result = store.load(false, &CancellationToken::new()).await;
    let Err(sk_games::GamesError::Io(e)) = result else {
        panic!("{result:?}");
    };
    assert_eq!(e.kind(), std::io::ErrorKind::NotFound, "{e}");
    assert!(!tmp.path().join("cache").exists());
}

/// The cache works the same with and without the snapshot.
#[tokio::test]
async fn the_cache_is_used_with_or_without_the_snapshot() {
    let tmp = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let cache = tmp.path().join("cache");
    std::fs::create_dir_all(&cache).unwrap_or_else(|e| panic!("{e}"));
    let mini = include_str!("../../../fixtures/samples/ludusavi/manifest-mini.yaml");
    std::fs::write(cache.join("ludusavi-manifest.yaml"), mini).unwrap_or_else(|e| panic!("{e}"));
    let store = ManifestStore::new(&Config::default(), tmp.path());
    let m = store
        .load(false, &CancellationToken::new())
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(m.meta.source, ManifestSource::Cache);
    assert_eq!(m.meta.games, 20);
}
