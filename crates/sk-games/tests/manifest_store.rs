//! `ManifestStore` with the real embedded snapshot (SPEC-05 FR-05-02, NFR-05-01).
//!
//! The index of the full manifest is several MB, so it is written under
//! `target/` (`CARGO_TARGET_TMPDIR`), not into the system temp folder.

use sk_core::config::Config;
use sk_core::CancellationToken;
use sk_games::{ManifestSource, ManifestStore};

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
