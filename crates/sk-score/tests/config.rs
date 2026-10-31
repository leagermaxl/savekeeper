//! Partial `scoring` overrides from `savekeeper.config.json` (SPEC-09 §4.6, §4.10).
//!
//! The config file is written into `target/` (`CARGO_TARGET_TMPDIR`), not into
//! the system temp folder.

// Helpers outside `#[test]` functions are not covered by `allow-unwrap-in-tests`.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use sk_core::config::Config;
use sk_core::model::Category;
use sk_score::{default_weights, validate_config, ScoringConfig, BAD_CONFIG_KEY};

fn load(name: &str, json: &str) -> ScoringConfig {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("sk-score-config");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, json).unwrap();
    Config::load_or_default(&path).config.scoring
}

#[test]
fn partial_override_keeps_other_defaults() {
    let scoring = load(
        "partial.json",
        r#"{
            "schema_version": 1,
            "scoring": {
                "select_threshold": 0.5,
                "category_weights": {
                    "game_save": { "authored": 0.8 },
                    "cache": { "size_tolerant": false }
                }
            }
        }"#,
    );
    let (cfg, issues) = validate_config(&scoring);
    assert!(issues.is_empty(), "{issues:?}");

    let defaults = ScoringConfig::default();
    assert_eq!(cfg.select_threshold, 0.5);
    assert_eq!(
        cfg.unknown_select_threshold,
        defaults.unknown_select_threshold
    );
    assert_eq!(cfg.max_default_item_bytes, defaults.max_default_item_bytes);

    let table = default_weights();
    let game_save = cfg.category_weights[&Category::GameSave];
    assert_eq!(game_save.authored, 0.8);
    assert_eq!(game_save.irreplaceability, 0.95);
    assert!(!game_save.size_tolerant);
    let cache = cfg.category_weights[&Category::Cache];
    assert!(!cache.size_tolerant);
    assert_eq!(cache.irreplaceability, 0.0);
    assert_eq!(cfg.category_weights.len(), table.len());
    for (category, weights) in &table {
        if !matches!(category, Category::GameSave | Category::Cache) {
            assert_eq!(&cfg.category_weights[category], weights, "{category:?}");
        }
    }
}

#[test]
fn invalid_override_falls_back_with_issue() {
    let scoring = load(
        "invalid.json",
        r#"{
            "schema_version": 1,
            "scoring": {
                "installed_bonus": 3.0,
                "category_weights": { "unknown": { "irreplaceability": -1.0 } }
            }
        }"#,
    );
    let (cfg, issues) = validate_config(&scoring);
    assert_eq!(cfg, ScoringConfig::default());
    assert_eq!(issues.len(), 2);
    assert!(issues.iter().all(|i| i.message_key == BAD_CONFIG_KEY));
}
