//! JSON form and defaults of the request, estimate and result types (SPEC-10 §4.1).

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::path::PathBuf;

use serde_json::json;
use sk_backup::{
    BackupEstimate, BackupFormat, BackupOptions, BackupTarget, EstimateBlocker, EstimateWarning,
    FsKind, FsLimitSubject, ManifestTarget, Passphrase, RetryPolicy,
};
use sk_core::model::{RegHive, Target};
use sk_core::template::PathTemplate;

#[test]
fn retry_policy_default_is_200_500_1000() {
    let policy = RetryPolicy::default();
    assert_eq!(policy.attempts, 3);
    assert_eq!(policy.delays_ms, [200, 500, 1000]);
}

#[test]
fn options_debug_hides_the_passphrase() {
    let options = BackupOptions {
        format: BackupFormat::Zip,
        compression_level: 6,
        encryption: Some(Passphrase::from("hunter2-secret")),
        verify: true,
        keep_partial_on_cancel: false,
        retry_locked: RetryPolicy::default(),
    };
    assert!(!format!("{options:?}").contains("hunter2-secret"));
}

#[test]
fn estimate_json_uses_kind_tags() {
    let finding = serde_json::from_value(json!("3fa2c9e01b7d44aa")).unwrap();
    let estimate = BackupEstimate {
        total_bytes: 10,
        file_count: 2,
        estimated_archive_bytes: 6,
        free_bytes: 100,
        fs_kind: FsKind::Fat32,
        largest_file_bytes: Some(8),
        blockers: vec![
            EstimateBlocker::InsufficientSpace {
                required_bytes: 52_428_807,
            },
            EstimateBlocker::FsLimit {
                subject: FsLimitSubject::File,
                bytes: 5_000_000_000,
                max_bytes: 4_294_967_295,
            },
            EstimateBlocker::TargetInsideSource { finding },
            EstimateBlocker::EncryptWithDir,
            EstimateBlocker::NotWritable,
        ],
        warnings: vec![
            EstimateWarning::Sensitive { count: 1 },
            EstimateWarning::LargeSelection,
            EstimateWarning::TargetOnSystemDrive,
            EstimateWarning::StalePartial {
                path: PathBuf::from("E:/b/x.zip.partial"),
            },
        ],
    };
    let value = serde_json::to_value(&estimate).unwrap();
    assert_eq!(value["fs_kind"], json!({ "kind": "fat32" }));
    assert_eq!(
        value["blockers"][0],
        json!({ "kind": "insufficient_space", "required_bytes": 52_428_807 })
    );
    assert_eq!(value["blockers"][1]["subject"], json!("file"));
    assert_eq!(
        value["blockers"][2],
        json!({ "kind": "target_inside_source", "finding": "3fa2c9e01b7d44aa" })
    );
    assert_eq!(value["blockers"][3], json!({ "kind": "encrypt_with_dir" }));
    assert_eq!(
        value["warnings"][0],
        json!({ "kind": "sensitive", "count": 1 })
    );
    assert_eq!(value["warnings"][3]["path"], json!("E:/b/x.zip.partial"));
    let back: BackupEstimate = serde_json::from_value(value).unwrap();
    assert_eq!(back, estimate);
}

#[test]
fn fs_kind_other_keeps_the_name() {
    let other = FsKind::Other("UDF".into());
    let value = serde_json::to_value(&other).unwrap();
    assert_eq!(value, json!({ "kind": "other", "name": "UDF" }));
    assert_eq!(serde_json::from_value::<FsKind>(value).unwrap(), other);
}

#[test]
fn target_json() {
    let target = BackupTarget {
        parent_dir: PathBuf::from("E:/Backups"),
        name: "SaveKeeper-PC".into(),
    };
    let value = serde_json::to_value(&target).unwrap();
    assert_eq!(
        value,
        json!({ "parent_dir": "E:/Backups", "name": "SaveKeeper-PC" })
    );
}

#[test]
fn manifest_target_drops_resolved_paths() {
    let root = PathTemplate::parse("{APPDATA}\\EldenRing").unwrap();
    let file_set = Target::FileSet {
        root: root.clone(),
        resolved: PathBuf::from("C:/Users/max/AppData/Roaming/EldenRing"),
        include: vec!["**/*.sl2".into()],
        exclude: vec![],
    };
    let manifest = ManifestTarget::from(&file_set);
    let value = serde_json::to_value(&manifest).unwrap();
    assert_eq!(
        value,
        json!({ "kind": "file_set", "root": "{APPDATA}\\EldenRing", "include": ["**/*.sl2"], "exclude": [] })
    );
    assert!(!value.to_string().contains("max"));

    let file = Target::File {
        path: root.clone(),
        resolved: PathBuf::from("C:/Users/max/x"),
    };
    assert_eq!(
        ManifestTarget::from(&file),
        ManifestTarget::File { path: root }
    );

    let registry = Target::Registry {
        hive: RegHive::Hkcu,
        key: "Software\\X".into(),
        recursive: false,
    };
    assert_eq!(
        serde_json::to_value(ManifestTarget::from(&registry)).unwrap(),
        json!({ "kind": "registry", "hive": "hkcu", "key": "Software\\X", "recursive": false })
    );

    let export = Target::SystemExport {
        exporter_id: "winget".into(),
        params: json!({ "a": 1 }),
    };
    assert_eq!(
        ManifestTarget::from(&export),
        ManifestTarget::SystemExport {
            exporter_id: "winget".into(),
            params: json!({ "a": 1 })
        }
    );
}
