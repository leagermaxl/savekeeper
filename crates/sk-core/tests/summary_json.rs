//! JSON contract of `FolderSummary` (SPEC-02 §4, §8): round-trip and snapshots.

// Helpers outside `#[test]` functions are not covered by `allow-unwrap-in-tests`.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;

use sk_core::model::{ChildStat, ExtStat, FolderSummary, Marker};
use sk_core::template::PathTemplate;
use time::macros::datetime;

fn summary() -> FolderSummary {
    FolderSummary {
        path: PathTemplate::parse(r"{LOCALAPPDATA}\SomeApp").unwrap(),
        total_bytes: 734_003_200,
        file_count: 1_204,
        dir_count: 87,
        max_depth: 6,
        newest_mtime: Some(datetime!(2026-09-30 18:02:11 UTC)),
        oldest_mtime: Some(datetime!(2023-01-15 09:00:00 UTC)),
        ext_histogram: vec![
            ExtStat {
                ext: "json".to_owned(),
                count: 412,
                bytes: 3_145_728,
            },
            ExtStat {
                ext: String::new(),
                count: 9,
                bytes: 4_096,
            },
        ],
        sample_names: vec![
            "settings.json".to_owned(),
            r"profiles\<redacted>\state.db".to_owned(),
        ],
        top_children: vec![ChildStat {
            name: "Cache".to_owned(),
            bytes: 600_000_000,
            files: 900,
        }],
        markers: vec![Marker::ElectronApp, Marker::ConfigLike],
        truncated: false,
    }
}

#[test]
fn summary_round_trips() {
    let s = summary();
    let json = serde_json::to_string(&s).unwrap();
    assert_eq!(serde_json::from_str::<FolderSummary>(&json).unwrap(), s);

    let empty = FolderSummary {
        newest_mtime: None,
        oldest_mtime: None,
        ext_histogram: vec![],
        sample_names: vec![],
        top_children: vec![],
        markers: vec![],
        truncated: true,
        ..summary()
    };
    let json = serde_json::to_string(&empty).unwrap();
    assert_eq!(serde_json::from_str::<FolderSummary>(&json).unwrap(), empty);
}

#[test]
fn snapshot_summary() {
    insta::assert_json_snapshot!("folder_summary", summary());
}

#[test]
fn marker_names() {
    let names: Vec<String> = Marker::ALL
        .iter()
        .map(|m| {
            serde_json::to_value(m)
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(
        names.iter().collect::<BTreeSet<_>>().len(),
        Marker::ALL.len()
    );
    for m in Marker::ALL {
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(serde_json::from_str::<Marker>(&json).unwrap(), m);
    }
    insta::assert_json_snapshot!("marker_names", names);
}

#[test]
fn invalid_template_in_summary_is_rejected() {
    let mut json = serde_json::to_value(summary()).unwrap();
    json["path"] = serde_json::json!(r"{NOPE}\x");
    assert!(serde_json::from_value::<FolderSummary>(json).is_err());
}
