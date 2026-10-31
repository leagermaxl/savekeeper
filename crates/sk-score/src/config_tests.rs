use sk_core::model::{Category, IssueSeverity};

use super::*;

const MIB: u64 = 1024 * 1024;

#[test]
fn defaults_match_spec_4_1() {
    let cfg = ScoringConfig::default();
    assert_eq!(cfg.select_threshold, 0.40);
    assert_eq!(cfg.unknown_select_threshold, 0.30);
    assert_eq!(cfg.unknown_max_default_bytes, 200 * MIB);
    assert_eq!(cfg.max_default_item_bytes, 2048 * MIB);
    assert_eq!(cfg.size_free_bytes, 100 * MIB);
    assert_eq!(cfg.size_penalty_cap, 0.40);
    assert_eq!(cfg.cloud_synced_penalty, 0.15);
    assert_eq!(cfg.installed_bonus, 0.05);
    assert_eq!(cfg.uninstalled_penalty, 0.10);
}

#[test]
fn default_weights_match_spec_4_6() {
    let expected = [
        (Category::Credentials, 1.00, 1.00, false),
        (Category::GameSave, 0.95, 0.90, false),
        (Category::UserFiles, 0.85, 0.90, true),
        (Category::DevEnvironment, 0.80, 0.60, false),
        (Category::AppData, 0.70, 0.50, false),
        (Category::AppConfig, 0.65, 0.50, false),
        (Category::SystemSettings, 0.60, 0.40, false),
        (Category::GameConfig, 0.55, 0.40, false),
        (Category::Unknown, 0.35, 0.30, false),
        (Category::Reinstallable, 0.05, 0.00, true),
        (Category::Cache, 0.00, 0.00, true),
    ];
    let weights = ScoringConfig::default().category_weights;
    assert_eq!(weights.len(), expected.len());
    for (category, irreplaceability, authored, size_tolerant) in expected {
        let w = weights[&category];
        assert_eq!(w.irreplaceability, irreplaceability, "{category:?}");
        assert_eq!(w.authored, authored, "{category:?}");
        assert_eq!(w.size_tolerant, size_tolerant, "{category:?}");
    }
    assert_eq!(weights, default_weights());
}

#[test]
fn valid_config_is_unchanged() {
    let cfg = ScoringConfig::default();
    let (out, issues) = validate_config(&cfg);
    assert_eq!(out, cfg);
    assert!(issues.is_empty());
}

#[test]
fn valid_overrides_are_kept() {
    let mut cfg = ScoringConfig {
        select_threshold: 0.0,
        installed_bonus: 1.0,
        max_default_item_bytes: 0,
        ..ScoringConfig::default()
    };
    if let Some(w) = cfg.category_weights.get_mut(&Category::Cache) {
        w.irreplaceability = 0.2;
        w.size_tolerant = false;
    }
    let (out, issues) = validate_config(&cfg);
    assert_eq!(out, cfg);
    assert!(issues.is_empty());
}

#[test]
fn out_of_range_and_non_finite_values_fall_back_to_defaults() {
    let mut cfg = ScoringConfig {
        select_threshold: 1.5,
        unknown_select_threshold: -0.1,
        size_penalty_cap: f32::NAN,
        cloud_synced_penalty: f32::INFINITY,
        installed_bonus: 0.07,
        ..ScoringConfig::default()
    };
    if let Some(w) = cfg.category_weights.get_mut(&Category::GameSave) {
        w.authored = 2.0;
        w.irreplaceability = 0.5;
    }
    let (out, issues) = validate_config(&cfg);

    let defaults = ScoringConfig::default();
    assert_eq!(out.select_threshold, defaults.select_threshold);
    assert_eq!(
        out.unknown_select_threshold,
        defaults.unknown_select_threshold
    );
    assert_eq!(out.size_penalty_cap, defaults.size_penalty_cap);
    assert_eq!(out.cloud_synced_penalty, defaults.cloud_synced_penalty);
    assert_eq!(out.installed_bonus, 0.07);
    let game_save = out.category_weights[&Category::GameSave];
    assert_eq!(game_save.authored, 0.90);
    assert_eq!(game_save.irreplaceability, 0.5);

    let fields: Vec<&str> = issues
        .iter()
        .map(|i| i.message_args["field"].as_str())
        .collect();
    assert_eq!(
        fields,
        [
            "scoring.select_threshold",
            "scoring.unknown_select_threshold",
            "scoring.size_penalty_cap",
            "scoring.cloud_synced_penalty",
            "scoring.category_weights.game_save.authored",
        ]
    );
    for issue in &issues {
        assert_eq!(issue.severity, IssueSeverity::Warning);
        assert_eq!(issue.source, ISSUE_SOURCE);
        assert_eq!(issue.message_key, BAD_CONFIG_KEY);
        assert!(issue.path.is_none());
    }
    assert_eq!(issues[0].message_args["value"], "1.5");
    assert_eq!(issues[2].message_args["value"], "NaN");
}

#[test]
fn missing_categories_are_filled_without_issues() {
    let mut cfg = ScoringConfig::default();
    cfg.category_weights.remove(&Category::Unknown);
    let (out, issues) = validate_config(&cfg);
    assert_eq!(out.category_weights, default_weights());
    assert!(issues.is_empty());
}
