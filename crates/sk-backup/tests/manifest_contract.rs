//! Contract of `manifest.json` v1 (SPEC-10 §4.4, T-10-01): the §4.4 example round-trips
//! through the Rust types, matches the JSON Schema, and both are snapshotted.
//!
//! The schema is also published as `specs/schemas/backup-manifest-v1.schema.json`
//! (the contract for SPEC-13). Regenerate it with `SK_UPDATE_SCHEMAS=1 cargo test -p sk-backup`.

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod common;

use std::path::PathBuf;

use serde_json::{json, Value};
use sk_backup::{manifest_json_schema, BackupManifest, BackupStatus, FindingStatus};

use common::schema_check::validate;

/// The §4.4 example with concrete values (elisions filled in).
fn example() -> Value {
    json!({
        "format": "savekeeper-backup",
        "schema_version": 1,
        "status": "complete_with_issues",
        "backup_id": "7b1c1f4e-2d3a-4b5c-8d9e-0f1a2b3c4d5e",
        "scan_id": "a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d",
        "app_version": "0.1.0+g1a2b3c4",
        "created_at": "2026-09-28T14:30:12Z",
        "finished_at": "2026-09-28T14:41:57Z",
        "container": { "format": "zip", "encrypted": false, "compression_level": 6 },
        "environment": {
            "os": {
                "product": "Windows 11 Pro", "display_version": "24H2", "build": "26100.2033",
                "arch": "x86_64", "ui_language": "ru-RU"
            },
            "machine_name": "DESKTOP-01",
            "known_folders": {
                "DOCUMENTS": "{ONEDRIVE}\\Документы",
                "SAVED_GAMES": "{HOME}\\Saved Games"
            },
            "drives": [{
                "letter": "D", "kind": "fixed", "fs": "NTFS", "label": "Data",
                "volume_serial": 305419896
            }],
            "launchers": [{ "id": "steam", "root": "{PROGRAMFILES_X86}\\Steam", "game_count": 12 }],
            "is_elevated": false
        },
        "findings": [
            {
                "id": "3fa2c9e01b7d44aa",
                "title": "ELDEN RING — сохранения",
                "category": "game_save",
                "app": {
                    "id": "elden-ring", "name": "ELDEN RING", "kind": "game",
                    "source_ids": { "steam": "1245620" }, "installed": true,
                    "process_names": ["eldenring.exe"]
                },
                "target": {
                    "kind": "file_set", "root": "{APPDATA}\\EldenRing", "include": [], "exclude": []
                },
                "sensitivity": "none",
                "archive_dir": "files/3fa2c9e01b7d44aa",
                "stats": { "total_bytes": 28311552, "file_count": 4 },
                "status": "complete"
            },
            {
                "id": "5c1e0a9d3b2f7e61",
                "title": "PuTTY — сессии",
                "category": "app_config",
                "target": {
                    "kind": "registry", "hive": "hkcu", "key": "Software\\SimonTatham\\PuTTY",
                    "recursive": true
                },
                "sensitivity": "low",
                "archive_path": "registry/5c1e0a9d3b2f7e61.reg",
                "status": "complete"
            },
            {
                "id": "e0c47a1f9b8d2233",
                "title": "winget",
                "category": "system_settings",
                "target": { "kind": "system_export", "exporter_id": "winget", "params": {} },
                "sensitivity": "none",
                "status": "complete"
            },
            {
                "id": "77aa00bb11cc22dd",
                "title": "Google Chrome — профиль",
                "category": "app_data",
                "target": {
                    "kind": "file", "path": "{LOCALAPPDATA}\\Google\\Chrome\\User Data\\Default\\History"
                },
                "sensitivity": "high",
                "archive_dir": "files/77aa00bb11cc22dd",
                "stats": { "total_bytes": 0, "file_count": 0 },
                "status": "partial"
            }
        ],
        "entries": [
            {
                "finding_id": "3fa2c9e01b7d44aa",
                "kind": "file",
                "source": "{APPDATA}\\EldenRing\\76561198000000000\\ER0000.sl2",
                "archive_path": "files/3fa2c9e01b7d44aa/76561198000000000/ER0000.sl2",
                "original_name": null,
                "size": 28311552,
                "mtime": "2026-09-27T22:10:04.123Z",
                "ctime": "2025-03-01T10:00:00Z",
                "attributes": ["archive"],
                "blake3": "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262",
                "flags": []
            },
            {
                "finding_id": "3fa2c9e01b7d44aa",
                "kind": "file",
                "source": "{APPDATA}\\EldenRing\\CON",
                "archive_path": "files/3fa2c9e01b7d44aa/CON~sk",
                "original_name": "CON",
                "size": 0,
                "mtime": "2026-09-27T22:10:04Z",
                "ctime": null,
                "attributes": ["hidden", "archive"],
                "blake3": "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262",
                "flags": ["name_escaped", "changed_during_read"]
            },
            {
                "finding_id": "3fa2c9e01b7d44aa",
                "kind": "symlink",
                "source": "{APPDATA}\\EldenRing\\latest",
                "archive_path": "files/3fa2c9e01b7d44aa/latest",
                "original_name": null,
                "size": 0,
                "mtime": null,
                "ctime": null,
                "attributes": [],
                "blake3": null,
                "flags": [],
                "link_target": "76561198000000000"
            }
        ],
        "exports": [{
            "finding_id": "e0c47a1f9b8d2233",
            "exporter_id": "winget",
            "params": {},
            "files": [{
                "archive_path": "system/winget/winget.json", "size": 18211,
                "blake3": "d74981efa70a0c880b8d8c1985d075dbcbf679b99a5f9914e5aaf96b831a9e24"
            }],
            "status": "complete",
            "elevated": false
        }],
        "locked": [{
            "finding_id": "77aa00bb11cc22dd",
            "source": "{LOCALAPPDATA}\\Google\\Chrome\\User Data\\Default\\History",
            "processes": [{ "name": "chrome.exe", "pid": 1234, "app_name": "Google Chrome" }]
        }],
        "errors": [
            {
                "finding_id": "3fa2c9e01b7d44aa", "source": "{HOME}\\Saved Games\\x.sav",
                "code": "access_denied", "message": "Access is denied. (os error 5)"
            },
            { "finding_id": null, "source": null, "code": "cloud_only", "message": "" }
        ],
        "totals": {
            "files": 18234, "bytes": 3_221_225_472_u64, "archive_bytes": 1987654321, "locked": 1, "errors": 2
        },
        "verification": {
            "performed": true, "ok": false, "checked_files": 18234,
            "mismatches": [{
                "archive_path": "files/3fa2c9e01b7d44aa/CON~sk",
                "expected_blake3": "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262",
                "actual_blake3": null
            }]
        }
    })
}

#[test]
fn example_round_trips_through_the_types() {
    let manifest: BackupManifest = serde_json::from_value(example()).unwrap();
    assert_eq!(manifest.format, BackupManifest::FORMAT);
    assert_eq!(manifest.schema_version, BackupManifest::SCHEMA_VERSION);
    assert_eq!(manifest.status, BackupStatus::CompleteWithIssues);
    assert_eq!(manifest.findings[3].status, FindingStatus::Partial);
    let written = serde_json::to_value(&manifest).unwrap();
    assert_eq!(written, example());
    // Through `Value`: insta's own serializer rejects the enum keys of `known_folders`.
    insta::assert_json_snapshot!("manifest_example", written);
}

#[test]
fn example_matches_the_schema() {
    let errors = validate(&manifest_json_schema(), &example());
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn schema_rejects_broken_manifests() {
    let schema = manifest_json_schema();
    let mut missing = example();
    missing.as_object_mut().unwrap().remove("entries");
    let mut bad_enum = example();
    bad_enum["findings"][0]["category"] = json!("games");
    let mut unknown = example();
    unknown["environment"]["os"]["user_name"] = json!("max");
    let mut bad_id = example();
    bad_id["entries"][0]["finding_id"] = json!(42);
    for broken in [missing, bad_enum, unknown, bad_id] {
        assert!(!validate(&schema, &broken).is_empty(), "{broken}");
    }
}

#[test]
fn schema_snapshot() {
    insta::assert_json_snapshot!("manifest_schema", manifest_json_schema());
}

#[test]
fn published_schema_file_is_up_to_date() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/schemas/backup-manifest-v1.schema.json");
    let generated = serde_json::to_string_pretty(&manifest_json_schema()).unwrap() + "\n";
    if std::env::var_os("SK_UPDATE_SCHEMAS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &generated).unwrap();
    }
    let published = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        published.replace("\r\n", "\n") == generated,
        "{} is stale; run `SK_UPDATE_SCHEMAS=1 cargo test -p sk-backup`",
        path.display()
    );
}
