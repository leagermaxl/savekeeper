//! Rule order, reason tags and the `sensitive_selected` issue of the default
//! selection (SPEC-09 §4.7, FR-09-05..08).

use sk_core::config::ScoringConfig;
use sk_core::model::{Category, Finding, IssueSeverity, Score, Sensitivity};

use crate::score_table_tests::{env, ev, finding, heuristic, rule, stats, GIB, MIB};
use crate::select::select_reporting;
use crate::{default_selection, SelectionDecision, ISSUE_SOURCE, SENSITIVE_SELECTED_KEY};

/// A finding with a given score value.
fn scored(category: Category, value: f32) -> Finding {
    let mut f = finding(category, vec![ev(rule(), 1.0)]);
    f.score = Some(Score {
        value,
        components: Default::default(),
    });
    f
}

fn decide(f: &Finding) -> SelectionDecision {
    default_selection(f, &env(), &ScoringConfig::default())
}

fn reason(f: &Finding) -> Option<&'static str> {
    let d = decide(f);
    assert_eq!(d.selected, d.reason_tag.is_none(), "{d:?}");
    d.reason_tag
}

#[test]
fn nested_wins_over_every_other_rule() {
    let mut f = scored(Category::Cache, 0.0);
    f.tags = vec!["alternative".to_owned(), "nested".to_owned()];
    f.requires_elevation = true;
    assert_eq!(reason(&f), Some("nested"));
}

#[test]
fn alternative_comes_before_restorable() {
    let mut f = scored(Category::Reinstallable, 0.9);
    f.tags = vec!["alternative".to_owned()];
    assert_eq!(reason(&f), Some("alternative"));
}

#[test]
fn cache_and_reinstallable_are_never_selected() {
    for category in [Category::Cache, Category::Reinstallable] {
        let f = scored(category, 1.0);
        assert_eq!(reason(&f), Some("restorable"), "{category:?}");
    }
}

#[test]
fn elevation_is_needed_only_when_not_elevated() {
    let mut f = scored(Category::SystemSettings, 0.9);
    f.requires_elevation = true;
    assert_eq!(reason(&f), Some("needs_elevation"));

    let mut elevated = env();
    elevated.is_elevated = true;
    let d = default_selection(&f, &elevated, &ScoringConfig::default());
    assert!(d.selected, "{d:?}");
}

#[test]
fn elevation_comes_before_size() {
    let mut f = scored(Category::UserFiles, 0.9);
    f.requires_elevation = true;
    f.stats = Some(stats(10 * GIB, 1));
    assert_eq!(reason(&f), Some("needs_elevation"));
}

#[test]
fn size_limit_is_exclusive() {
    let cfg = ScoringConfig::default();
    let mut f = scored(Category::UserFiles, 0.9);
    f.stats = Some(stats(cfg.max_default_item_bytes, 1));
    assert_eq!(reason(&f), None);
    f.stats = Some(stats(cfg.max_default_item_bytes + 1, 1));
    assert_eq!(reason(&f), Some("too_large_default"));
}

#[test]
fn too_large_applies_to_unknown_before_threshold() {
    let mut f = scored(Category::Unknown, 0.0);
    f.stats = Some(stats(3 * GIB, 1));
    assert_eq!(reason(&f), Some("too_large_default"));
}

#[test]
fn unknown_uses_its_own_threshold_and_size_limit() {
    let cfg = ScoringConfig::default();
    // Below the general threshold, at the Unknown threshold: selected.
    let f = scored(Category::Unknown, cfg.unknown_select_threshold);
    assert_eq!(reason(&f), None);

    let f = scored(Category::Unknown, 0.299);
    assert_eq!(reason(&f), Some("below_threshold"));

    let mut f = scored(Category::Unknown, 0.9);
    f.stats = Some(stats(cfg.unknown_max_default_bytes, 1));
    assert_eq!(reason(&f), None);
    f.stats = Some(stats(cfg.unknown_max_default_bytes + 1, 1));
    assert_eq!(reason(&f), Some("below_threshold"));
}

#[test]
fn general_threshold_is_inclusive() {
    let cfg = ScoringConfig::default();
    let f = scored(Category::AppConfig, cfg.select_threshold);
    assert_eq!(reason(&f), None);
    let f = scored(Category::AppConfig, 0.399);
    assert_eq!(reason(&f), Some("below_threshold"));
    // The Unknown threshold does not apply to other categories.
    let f = scored(Category::AppConfig, 0.35);
    assert_eq!(reason(&f), Some("below_threshold"));
}

#[test]
fn missing_stats_skip_size_rules() {
    let mut f = scored(Category::Unknown, 0.5);
    f.stats = None;
    assert_eq!(reason(&f), None);
}

#[test]
fn missing_score_counts_as_zero() {
    let mut f = finding(Category::GameSave, vec![ev(heuristic(), 1.0)]);
    f.score = None;
    assert_eq!(reason(&f), Some("below_threshold"));
}

#[test]
fn custom_thresholds_are_used() {
    let cfg = ScoringConfig {
        select_threshold: 0.9,
        max_default_item_bytes: MIB,
        ..ScoringConfig::default()
    };
    let f = scored(Category::AppConfig, 0.8);
    let d = default_selection(&f, &env(), &cfg);
    assert_eq!(d.reason_tag, Some("below_threshold"));

    let mut f = scored(Category::AppConfig, 0.95);
    f.stats = Some(stats(2 * MIB, 1));
    let d = default_selection(&f, &env(), &cfg);
    assert_eq!(d.reason_tag, Some("too_large_default"));
}

#[test]
fn reporting_sets_flag_and_adds_sorted_unique_tag() {
    let mut nested = scored(Category::AppConfig, 0.9);
    nested.tags = vec!["nested".to_owned(), "x".to_owned()];
    nested.default_selected = true;
    let mut low = scored(Category::AppConfig, 0.1);
    low.tags = vec!["zz".to_owned(), "a".to_owned()];
    let picked = scored(Category::AppConfig, 0.9);

    let mut findings = vec![nested, low, picked];
    let mut issues = Vec::new();
    select_reporting(
        &mut findings,
        &env(),
        &ScoringConfig::default(),
        &mut issues,
    );

    assert!(!findings[0].default_selected);
    assert_eq!(findings[0].tags, ["nested", "x"]);
    assert!(!findings[1].default_selected);
    assert_eq!(findings[1].tags, ["a", "below_threshold", "zz"]);
    assert!(findings[2].default_selected);
    assert!(findings[2].tags.is_empty());
    assert!(issues.is_empty(), "{issues:?}");
}

#[test]
fn reporting_is_idempotent_for_tags() {
    let mut findings = vec![scored(Category::Cache, 0.1)];
    let mut issues = Vec::new();
    let cfg = ScoringConfig::default();
    select_reporting(&mut findings, &env(), &cfg, &mut issues);
    select_reporting(&mut findings, &env(), &cfg, &mut issues);
    assert_eq!(findings[0].tags, ["restorable"]);
}

#[test]
fn one_sensitive_issue_per_scan_with_count() {
    let creds = scored(Category::Credentials, 0.9);
    let mut high = scored(Category::AppData, 0.9);
    high.sensitivity = Sensitivity::High;
    let mut low = scored(Category::AppData, 0.9);
    low.sensitivity = Sensitivity::Low;
    // Not selected: no warning for it.
    let mut unselected = scored(Category::Credentials, 0.9);
    unselected.requires_elevation = true;

    let mut findings = vec![creds, high, low, unselected];
    let mut issues = Vec::new();
    select_reporting(
        &mut findings,
        &env(),
        &ScoringConfig::default(),
        &mut issues,
    );

    assert_eq!(issues.len(), 1, "{issues:?}");
    let issue = &issues[0];
    assert_eq!(issue.severity, IssueSeverity::Warning);
    assert_eq!(issue.source, ISSUE_SOURCE);
    assert_eq!(issue.message_key, SENSITIVE_SELECTED_KEY);
    assert_eq!(issue.path, None);
    assert_eq!(issue.message_args.len(), 1);
    assert_eq!(issue.message_args["count"], "2");
}

#[test]
fn no_sensitive_issue_when_nothing_sensitive_is_selected() {
    let mut f = scored(Category::Credentials, 0.1);
    f.stats = Some(stats(MIB, 1));
    let mut findings = vec![f];
    let mut issues = Vec::new();
    select_reporting(
        &mut findings,
        &env(),
        &ScoringConfig::default(),
        &mut issues,
    );
    assert!(!findings[0].default_selected);
    assert!(issues.is_empty(), "{issues:?}");
}
