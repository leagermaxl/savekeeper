//! Validation of the `scoring` config section (SPEC-09 §4.10).

use std::collections::BTreeMap;

use sk_core::config::{default_weights, ScoringConfig};
use sk_core::model::{IssueSeverity, ScanIssue};

/// `ScanIssue::source` of the issues produced by `sk-score`.
pub const ISSUE_SOURCE: &str = "scoring";

/// i18n key of an invalid `scoring` value replaced by its default.
pub const BAD_CONFIG_KEY: &str = "scoring.bad_config";

/// Returns `cfg` with every invalid value replaced by its default (SPEC-09 §4.10).
///
/// Weights, thresholds, penalties and the bonus must be finite and lie in
/// `[0, 1]`. Each replaced value gives one `Warning` `scoring.bad_config` with
/// `message_args { field, value }`; `field` is the config path, for example
/// `scoring.category_weights.game_save.authored`. Categories missing from
/// `category_weights` are filled from the defaults without an issue (a partial
/// table is valid). Byte limits are `u64` and cannot be negative.
pub fn validate_config(cfg: &ScoringConfig) -> (ScoringConfig, Vec<ScanIssue>) {
    let defaults = ScoringConfig::default();
    let mut out = cfg.clone();
    let mut issues = Vec::new();

    let mut check = |field: &str, value: &mut f32, default: f32| {
        if !is_unit(*value) {
            issues.push(bad_config(field, *value));
            *value = default;
        }
    };

    check(
        "scoring.select_threshold",
        &mut out.select_threshold,
        defaults.select_threshold,
    );
    check(
        "scoring.unknown_select_threshold",
        &mut out.unknown_select_threshold,
        defaults.unknown_select_threshold,
    );
    check(
        "scoring.size_penalty_cap",
        &mut out.size_penalty_cap,
        defaults.size_penalty_cap,
    );
    check(
        "scoring.cloud_synced_penalty",
        &mut out.cloud_synced_penalty,
        defaults.cloud_synced_penalty,
    );
    check(
        "scoring.installed_bonus",
        &mut out.installed_bonus,
        defaults.installed_bonus,
    );
    check(
        "scoring.uninstalled_penalty",
        &mut out.uninstalled_penalty,
        defaults.uninstalled_penalty,
    );

    for (category, default) in default_weights() {
        let weights = out.category_weights.entry(category).or_insert(default);
        let name = category_name(category);
        check(
            &format!("scoring.category_weights.{name}.irreplaceability"),
            &mut weights.irreplaceability,
            default.irreplaceability,
        );
        check(
            &format!("scoring.category_weights.{name}.authored"),
            &mut weights.authored,
            default.authored,
        );
    }

    (out, issues)
}

/// Finite and within `[0, 1]`.
fn is_unit(value: f32) -> bool {
    value.is_finite() && (0.0..=1.0).contains(&value)
}

fn bad_config(field: &str, value: f32) -> ScanIssue {
    ScanIssue {
        severity: IssueSeverity::Warning,
        source: ISSUE_SOURCE.to_owned(),
        path: None,
        message_key: BAD_CONFIG_KEY.to_owned(),
        message_args: BTreeMap::from([
            ("field".to_owned(), field.to_owned()),
            ("value".to_owned(), value.to_string()),
        ]),
    }
}

/// The serde name of a category, as written in the config.
fn category_name(category: sk_core::model::Category) -> &'static str {
    use sk_core::model::Category as C;
    match category {
        C::GameSave => "game_save",
        C::GameConfig => "game_config",
        C::AppConfig => "app_config",
        C::AppData => "app_data",
        C::UserFiles => "user_files",
        C::DevEnvironment => "dev_environment",
        C::Credentials => "credentials",
        C::SystemSettings => "system_settings",
        C::Reinstallable => "reinstallable",
        C::Cache => "cache",
        C::Unknown => "unknown",
    }
}
