//! `savekeeper.config.json` loading rules and the data folder (SPEC-01 §4.8, §6).

// Helpers outside `#[test]` functions are not covered by `allow-unwrap-in-tests`.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use sk_core::config::{Config, ConfigError, ConfigWarning, DataDir, LocalLlmKind};
use sk_core::model::{Category, LlmMode};

/// The schema of SPEC-01 §4.8.2 without comments.
const SPEC_EXAMPLE: &str = r#"{
  "schema_version": 1,
  "ui": { "language": "auto", "theme": "system" },
  "scan": { "extra_roots": [], "exclude_globs": [], "follow_symlinks": false, "max_depth": 32, "ignored_templates": [] },
  "games": { "manifest_url": "https://raw.githubusercontent.com/mtkennerly/ludusavi-manifest/master/data/manifest.yaml",
             "auto_update": true, "update_interval_hours": 168 },
  "system": { "disabled_exporters": [], "winget_timeout_s": 180 },
  "heuristics": {
    "enabled": { "unk": true, "usr": true, "git": true, "junk": true, "web": true },
    "unknown_threshold": 0.6,
    "usr": { "min_weight": 20, "max_candidates": 200, "scan_all_fixed_drives": true },
    "git": { "max_depth": 6, "max_repos": 200 },
    "thresholds": { "save_ext_ratio": 0.2, "config_max_bytes": 5242880, "sqlite_recent_days": 90 }
  },
  "llm": {
    "mode": "off",
    "local": { "kind": "ollama", "endpoint": "http://127.0.0.1:11434", "model": "qwen2.5:7b-instruct",
               "timeout_s": 120, "max_batch": 8, "max_concurrency": 1 },
    "cloud": { "kind": "anthropic", "model": "claude-haiku-4-5",
               "api_key_source": "credential_manager", "api_key_env": "ANTHROPIC_API_KEY",
               "timeout_s": 60, "max_batch": 20, "max_concurrency": 4 },
    "max_items_per_scan": 300,
    "allow_content_peek": false,
    "allow_content_peek_cloud": false,
    "confirm_cloud_each_scan": true,
    "min_confidence_to_apply": 0.5
  },
  "scoring": {
    "select_threshold": 0.40, "unknown_select_threshold": 0.30,
    "max_default_item_bytes": 2147483648, "unknown_max_default_bytes": 209715200
  },
  "backup": { "format": "zip", "compression_level": 6, "encrypt": false, "verify": true },
  "updates": { "check": false, "interval_days": 7 }
}"#;

fn write(dir: &Path, text: &str) -> PathBuf {
    let path = dir.join("savekeeper.config.json");
    std::fs::write(&path, text).unwrap();
    path
}

#[test]
fn spec_example_is_the_default() {
    let dir = tempfile::tempdir().unwrap();
    let loaded = Config::load_or_default(&write(dir.path(), SPEC_EXAMPLE));
    assert_eq!(loaded.warnings, []);
    assert_eq!(loaded.config, Config::default());
}

#[test]
fn snapshot_default_config() {
    // insta cannot write enum map keys (category_weights); serde_json can. Going
    // through a string keeps f32 values short (0.4, not 0.4000000059604645).
    let text = serde_json::to_string(&Config::default()).unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    insta::assert_json_snapshot!("default_config", json);
}

#[test]
fn missing_file_is_created_with_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sub").join("savekeeper.config.json");
    let loaded = Config::load_or_default(&path);
    assert_eq!(loaded.warnings, [ConfigWarning::Created]);
    assert_eq!(loaded.config, Config::default());
    let again = Config::load_or_default(&path);
    assert_eq!(again.warnings, []);
    assert_eq!(again.config, Config::default());
}

#[test]
fn partial_config_keeps_other_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        dir.path(),
        r#"{ "llm": { "mode": "local", "local": { "kind": "openai_compat" } } }"#,
    );
    let loaded = Config::load_or_default(&path);
    assert_eq!(loaded.warnings, []);
    let c = loaded.config;
    assert_eq!(c.llm.mode, LlmMode::Local);
    assert_eq!(c.llm.local.kind, LocalLlmKind::OpenaiCompat);
    assert_eq!(c.llm.local.model, "qwen2.5:7b-instruct");
    assert_eq!(c.scan, Config::default().scan);
    assert_eq!(c.schema_version, Config::SCHEMA_VERSION);
}

#[test]
fn category_weights_are_merged_over_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        dir.path(),
        r#"{ "scoring": { "category_weights": { "game_save": { "authored": 0.5 }, "cache": { "size_tolerant": false } } } }"#,
    );
    let w = Config::load_or_default(&path)
        .config
        .scoring
        .category_weights;
    assert_eq!(w.len(), 11);
    assert_eq!(w[&Category::GameSave].irreplaceability, 0.95);
    assert_eq!(w[&Category::GameSave].authored, 0.5);
    assert!(!w[&Category::Cache].size_tolerant);
    assert_eq!(w[&Category::Credentials].irreplaceability, 1.0);
}

#[test]
fn unknown_fields_are_reported_and_kept() {
    let dir = tempfile::tempdir().unwrap();
    let text =
        r#"{ "extra": true, "llm": { "locl": {}, "mode": "off" }, "ui": { "theme": "dark" } }"#;
    let path = write(dir.path(), text);
    let loaded = Config::load_or_default(&path);
    assert_eq!(
        loaded.warnings,
        [
            ConfigWarning::UnknownField("extra".to_owned()),
            ConfigWarning::UnknownField("llm.locl".to_owned()),
        ]
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
}

#[test]
fn broken_file_goes_to_bak() {
    for text in [
        "{ not json",
        r#"{ "scan": { "max_depth": "deep" } }"#,
        r#"{ "schema_version": 0 }"#,
        "[]",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), text);
        let loaded = Config::load_or_default(&path);
        let bak = dir.path().join("savekeeper.config.json.bak");
        match loaded.warnings.as_slice() {
            [ConfigWarning::Corrupt {
                backup: Some(b), ..
            }] => assert_eq!(b, &bak),
            other => panic!("{text}: {other:?}"),
        }
        assert_eq!(loaded.config, Config::default());
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), text);
        assert_eq!(
            Config::load_or_default(&path).warnings,
            [],
            "{text}: defaults were written"
        );
    }
}

#[test]
fn newer_schema_keeps_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let text = r#"{ "schema_version": 2, "ui": { "theme": "dark" } }"#;
    let path = write(dir.path(), text);
    let loaded = Config::load_or_default(&path);
    assert_eq!(loaded.warnings, [ConfigWarning::NewerSchema(2)]);
    assert_eq!(loaded.config, Config::default());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
}

#[test]
fn save_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a").join("b.json");
    let mut config = Config::default();
    config.scan.extra_roots.push(PathBuf::from("D:/Projects"));
    config.backup.compression_level = 9;
    config.save(&path).unwrap();
    let loaded = Config::load_or_default(&path);
    assert_eq!(loaded.warnings, []);
    assert_eq!(loaded.config, config);
    assert!(!dir.path().join("a").join("b.json.tmp").exists());
}

#[test]
fn data_dir_is_portable_when_writable() {
    let exe = tempfile::tempdir().unwrap();
    let dd = DataDir::locate(exe.path(), None).unwrap();
    assert!(dd.portable);
    assert_eq!(dd.root, exe.path().join("savekeeper-data"));
    assert_eq!(dd.config_path, exe.path().join("savekeeper.config.json"));
    // The probe file is removed.
    assert_eq!(std::fs::read_dir(exe.path()).unwrap().count(), 0);
    dd.create_dirs().unwrap();
    for sub in ["logs", "cache", "rules.d", "scans"] {
        assert!(dd.root.join(sub).is_dir(), "{sub}");
    }
}

#[test]
fn data_dir_falls_back_to_local_app_data() {
    let exe = tempfile::tempdir().unwrap();
    let missing = exe.path().join("not-here");
    let local = tempfile::tempdir().unwrap();
    let dd = DataDir::locate(&missing, Some(local.path())).unwrap();
    assert!(!dd.portable);
    assert_eq!(
        dd.root,
        local.path().join("SaveKeeper").join("savekeeper-data")
    );
    assert_eq!(
        dd.config_path,
        local
            .path()
            .join("SaveKeeper")
            .join("savekeeper.config.json")
    );
    assert!(matches!(
        DataDir::locate(&missing, None),
        Err(ConfigError::NoDataDir)
    ));
}
