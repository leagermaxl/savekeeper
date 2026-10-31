//! Components, rounding and edge cases of `score()` (SPEC-09 §4.5, §5).

use sk_core::config::ScoringConfig;
use sk_core::model::{Category, Evidence, IssueSeverity};
use time::Duration;

use crate::score::score_reporting;
use crate::score_table_tests::{app, env, ev, finding, heuristic, llm, rule, stats, GIB, MIB, NOW};
use crate::{score, BAD_NUMBER_KEY, ISSUE_SOURCE, VIA_ARG};

fn llm_ev(confidence: f32, importance: Option<f32>) -> Evidence {
    let mut e = ev(llm(), confidence);
    e.importance = importance;
    e
}

fn llm_str(confidence: f32, importance: &str) -> Evidence {
    let mut e = ev(llm(), confidence);
    e.message_args
        .insert("importance".to_owned(), importance.to_owned());
    e
}

fn authored(evidence: Vec<Evidence>) -> f32 {
    authored_with_issues(evidence).0
}

fn authored_with_issues(evidence: Vec<Evidence>) -> (f32, Vec<sk_core::model::ScanIssue>) {
    let f = finding(Category::AppData, evidence);
    let mut issues = Vec::new();
    let s = score_reporting(&f, &env(), NOW, &ScoringConfig::default(), &mut issues);
    (s.components["user_authored"], issues)
}

#[test]
fn components_hold_every_term() {
    let f = finding(Category::AppConfig, vec![ev(rule(), 0.95)]);
    let s = score(&f, &env(), NOW, &ScoringConfig::default());
    let keys: Vec<&str> = s.components.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "adjustments",
            "base",
            "confidence",
            "irreplaceability",
            "recency",
            "size_penalty",
            "user_authored",
            "value",
        ]
    );
    assert_eq!(s.components["value"], s.value);
    assert_eq!(s.components["confidence"], 0.95);
    assert_eq!(s.components["irreplaceability"], 0.65);
}

#[test]
fn values_are_rounded_to_three_decimals() {
    let mut f = finding(Category::UserFiles, vec![ev(heuristic(), 0.7)]);
    f.stats = Some(stats(40 * GIB, 60));
    let s = score(&f, &env(), NOW, &ScoringConfig::default());
    for (key, v) in &s.components {
        let scaled = f64::from(*v) * 1000.0;
        assert!((scaled - scaled.round()).abs() < 1e-3, "{key} = {v}");
    }
}

#[test]
fn recency_curve() {
    let r = |days: i64| {
        let mut f = finding(Category::AppData, vec![ev(rule(), 1.0)]);
        f.stats = Some(stats(MIB, days));
        score(&f, &env(), NOW, &ScoringConfig::default()).components["recency"]
    };
    assert_eq!(r(0), 1.0);
    assert_eq!(r(30), 1.0);
    assert_eq!(r(395), 0.558); // 0.3 + 0.7·e^-1
    assert!(r(31) < 1.0);
    // A modification time in the future: d = 0.
    assert_eq!(r(-10), 1.0);
}

#[test]
fn missing_stats_or_mtime_give_neutral_recency_and_no_penalty() {
    let f = finding(Category::AppData, vec![ev(rule(), 1.0)]);
    let s = score(&f, &env(), NOW, &ScoringConfig::default());
    assert_eq!(s.components["recency"], 0.5);
    assert_eq!(s.components["size_penalty"], 0.0);

    let mut f = finding(Category::AppData, vec![ev(rule(), 1.0)]);
    let mut st = stats(10 * GIB, 0);
    st.newest_mtime = None;
    f.stats = Some(st);
    let s = score(&f, &env(), NOW, &ScoringConfig::default());
    assert_eq!(s.components["recency"], 0.5);
    assert_eq!(s.components["size_penalty"], 0.4);
}

#[test]
fn size_penalty_scale_cap_and_tolerance() {
    let s = |category, bytes| {
        let mut f = finding(category, vec![ev(rule(), 1.0)]);
        f.stats = Some(stats(bytes, 1));
        score(&f, &env(), NOW, &ScoringConfig::default()).components["size_penalty"]
    };
    assert_eq!(s(Category::AppData, 100 * MIB), 0.0);
    assert_eq!(s(Category::AppData, 200 * MIB), 0.1);
    assert_eq!(s(Category::AppData, 800 * MIB), 0.3);
    assert_eq!(s(Category::AppData, 100 * GIB), 0.4);
    assert_eq!(s(Category::UserFiles, 800 * MIB), 0.15);
    assert_eq!(s(Category::UserFiles, 100 * GIB), 0.2);
}

#[test]
fn zero_free_size_gives_the_cap() {
    let cfg = ScoringConfig {
        size_free_bytes: 0,
        ..ScoringConfig::default()
    };
    let mut f = finding(Category::AppData, vec![ev(rule(), 1.0)]);
    f.stats = Some(stats(1, 1));
    assert_eq!(score(&f, &env(), NOW, &cfg).components["size_penalty"], 0.4);
}

#[test]
fn adjustments() {
    let adj = |category, installed: Option<bool>, cloud: bool| {
        let mut f = finding(category, vec![ev(rule(), 1.0)]);
        f.app = installed.map(app);
        if cloud {
            f.tags.push("cloud-synced".to_owned());
        }
        score(&f, &env(), NOW, &ScoringConfig::default()).components["adjustments"]
    };
    assert_eq!(adj(Category::GameSave, Some(true), false), 0.05);
    assert_eq!(adj(Category::AppConfig, Some(false), false), -0.1);
    assert_eq!(adj(Category::AppData, Some(false), false), -0.1);
    assert_eq!(adj(Category::GameConfig, Some(false), false), -0.1);
    // The uninstalled penalty applies only to app categories.
    assert_eq!(adj(Category::GameSave, Some(false), false), 0.0);
    assert_eq!(adj(Category::UserFiles, None, true), -0.15);
    assert_eq!(adj(Category::AppConfig, Some(false), true), -0.25);
}

#[test]
fn value_is_clamped_at_zero() {
    let mut f = finding(Category::Cache, vec![ev(heuristic(), 0.0)]);
    f.stats = Some(stats(100 * GIB, 1000));
    f.tags.push("cloud-synced".to_owned());
    let s = score(&f, &env(), NOW, &ScoringConfig::default());
    assert_eq!(s.value, 0.0);
}

#[test]
fn unsaved_git_tags_make_authored_one() {
    for tag in ["git-unsaved", "git-no-remote"] {
        let mut f = finding(Category::DevEnvironment, vec![llm_ev(0.9, Some(0.1))]);
        f.tags.push(tag.to_owned());
        let s = score(&f, &env(), NOW, &ScoringConfig::default());
        assert_eq!(s.components["user_authored"], 1.0, "{tag}");
    }
}

#[test]
fn llm_importance_is_used_when_the_category_comes_from_the_llm() {
    assert_eq!(authored(vec![llm_ev(0.8, Some(0.9))]), 0.9);
    // Heuristic candidates are what the LLM classifies.
    assert_eq!(
        authored(vec![ev(heuristic(), 0.0), llm_ev(0.8, Some(0.9))]),
        0.9
    );
    // The string form of SPEC-08 §9.
    assert_eq!(authored(vec![llm_str(0.8, "0.2")]), 0.2);
    // Out of range is clamped.
    assert_eq!(authored(vec![llm_ev(0.8, Some(1.7))]), 1.0);
    // The most confident LLM evidence wins.
    assert_eq!(
        authored(vec![llm_ev(0.4, Some(0.1)), llm_ev(0.9, Some(0.7))]),
        0.7
    );
}

#[test]
fn llm_importance_is_ignored_otherwise() {
    // Default `authored` of AppData.
    assert_eq!(authored(vec![ev(heuristic(), 0.9)]), 0.5);
    // A rule decides the category, the LLM cannot override it.
    assert_eq!(authored(vec![ev(rule(), 0.9), llm_ev(0.8, Some(0.9))]), 0.5);
    // LLM evidence without importance.
    assert_eq!(authored(vec![llm_ev(0.8, None)]), 0.5);
    // Evidence taken over from a nested child.
    let mut via = llm_ev(0.8, Some(0.9));
    via.message_args
        .insert(VIA_ARG.to_owned(), "Child".to_owned());
    assert_eq!(authored(vec![ev(heuristic(), 0.9), via]), 0.5);
}

#[test]
fn bad_importance_counts_as_zero_with_an_issue() {
    for bad in [
        llm_str(0.8, "NaN"),
        llm_str(0.8, "inf"),
        llm_str(0.8, "high"),
        llm_ev(0.8, Some(f32::NAN)),
    ] {
        let (a, issues) = authored_with_issues(vec![bad]);
        assert_eq!(a, 0.0);
        assert_eq!(issues.len(), 1);
        let issue = &issues[0];
        assert_eq!(issue.severity, IssueSeverity::Warning);
        assert_eq!(issue.source, ISSUE_SOURCE);
        assert_eq!(issue.message_key, BAD_NUMBER_KEY);
        assert_eq!(issue.message_args["field"], "importance");
    }
    let (_, issues) = authored_with_issues(vec![llm_str(0.8, "0.5")]);
    assert!(issues.is_empty());
}

#[test]
fn non_finite_confidence_counts_as_zero() {
    let f = finding(Category::AppData, vec![ev(rule(), f32::NAN)]);
    let s = score(&f, &env(), NOW, &ScoringConfig::default());
    assert_eq!(s.components["confidence"], 0.0);
    assert!((0.0..=1.0).contains(&s.value));
}

#[test]
fn deterministic_for_the_same_now() {
    let mut f = finding(
        Category::GameSave,
        vec![llm_ev(0.5, Some(0.4)), ev(heuristic(), 0.7)],
    );
    let mut st = stats(3 * GIB, 0);
    st.newest_mtime = Some(NOW - Duration::days(400) - Duration::hours(7));
    f.stats = Some(st);
    let cfg = ScoringConfig::default();
    let a = score(&f, &env(), NOW, &cfg);
    f.evidence.reverse();
    let b = score(&f, &env(), NOW, &cfg);
    assert_eq!(a, b);
}

fn confidence_of(category: Category, evidence: Vec<Evidence>) -> f32 {
    let f = finding(category, evidence);
    score(&f, &env(), NOW, &ScoringConfig::default()).components["confidence"]
}

#[test]
fn confidence_comes_from_the_llm_verdict() {
    // Heuristic C=0 outranks Llm in §4.2 but no longer describes the category.
    assert_eq!(
        confidence_of(
            Category::AppData,
            vec![ev(heuristic(), 0.0), llm_ev(0.7, Some(0.5))]
        ),
        0.7
    );
    // An LLM answer that was not applied: the finding stayed Unknown.
    assert_eq!(
        confidence_of(
            Category::Unknown,
            vec![ev(heuristic(), 0.0), llm_ev(0.3, Some(0.3))]
        ),
        0.3
    );
    // Several verdicts: the highest confidence.
    assert_eq!(
        confidence_of(
            Category::AppData,
            vec![llm_ev(0.4, Some(0.9)), llm_ev(0.8, Some(0.1))]
        ),
        0.8
    );
    // The importance of the same evidence is used for A.
    let f = finding(
        Category::AppData,
        vec![llm_ev(0.4, Some(0.9)), llm_ev(0.8, Some(0.1))],
    );
    let s = score(&f, &env(), NOW, &ScoringConfig::default());
    assert_eq!(s.components["user_authored"], 0.1);
}

#[test]
fn unknown_without_the_llm_has_neutral_confidence() {
    assert_eq!(
        confidence_of(Category::Unknown, vec![ev(heuristic(), 0.0)]),
        0.5
    );
    assert_eq!(
        confidence_of(Category::Unknown, vec![ev(heuristic(), 0.9)]),
        0.5
    );
    // LLM evidence without importance does not make the category the LLM's.
    assert_eq!(
        confidence_of(
            Category::Unknown,
            vec![ev(heuristic(), 0.0), llm_ev(0.9, None)]
        ),
        0.5
    );
    // Nor does LLM evidence taken over from a nested child.
    let mut via = llm_ev(0.9, Some(0.9));
    via.message_args
        .insert(VIA_ARG.to_owned(), "Child".to_owned());
    assert_eq!(
        confidence_of(Category::Unknown, vec![ev(heuristic(), 0.0), via]),
        0.5
    );
}

#[test]
fn otherwise_confidence_is_the_authority() {
    // A rule outranks the LLM: §4.2 applies.
    assert_eq!(
        confidence_of(
            Category::AppConfig,
            vec![ev(rule(), 0.9), llm_ev(0.3, Some(0.3))]
        ),
        0.9
    );
    // An Unknown category not from the LLM stays neutral, whatever the source.
    assert_eq!(
        confidence_of(
            Category::Unknown,
            vec![ev(rule(), 0.9), llm_ev(0.3, Some(0.3))]
        ),
        0.5
    );
    assert_eq!(
        confidence_of(Category::AppData, vec![ev(heuristic(), 0.6)]),
        0.6
    );
}
