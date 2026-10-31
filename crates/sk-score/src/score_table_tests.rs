//! The example table of SPEC-09 §6.1: `value` (T-09-05) and `selected`
//! (T-09-06) columns.

use std::collections::BTreeMap;
use std::path::Path;

use sk_core::config::ScoringConfig;
use sk_core::env::Environment;
use sk_core::model::{
    AppKind, AppRef, Category, Evidence, EvidenceSource, Finding, FindingId, RegHive, Sensitivity,
    Target, TargetStats,
};
use time::macros::datetime;
use time::{Duration, OffsetDateTime};

use crate::select::select_reporting;
use crate::{default_selection, score, SelectionDecision, SENSITIVE_SELECTED_KEY};

pub(crate) const MIB: u64 = 1024 * 1024;
pub(crate) const GIB: u64 = 1024 * MIB;

/// `now` of the table.
pub(crate) const NOW: OffsetDateTime = datetime!(2026-09-28 12:00 UTC);

pub(crate) fn env() -> Environment {
    let mut env = Environment::fake(Path::new("/fake"));
    env.is_elevated = false;
    env
}

pub(crate) fn finding(category: Category, evidence: Vec<Evidence>) -> Finding {
    let target = Target::Registry {
        hive: RegHive::Hkcu,
        key: r"Software\Test".to_owned(),
        recursive: true,
    };
    Finding {
        id: FindingId::for_target(&target),
        target,
        category,
        app: None,
        title: "test".to_owned(),
        evidence,
        stats: None,
        sensitivity: Sensitivity::None,
        score: None,
        default_selected: false,
        requires_elevation: false,
        tags: vec![],
        children: vec![],
        notes_key: None,
    }
}

pub(crate) fn ev(source: EvidenceSource, confidence: f32) -> Evidence {
    Evidence {
        source,
        message_key: "evidence.test".to_owned(),
        message_args: BTreeMap::new(),
        confidence,
        importance: None,
    }
}

pub(crate) fn rule() -> EvidenceSource {
    EvidenceSource::Rule {
        rule_id: "r".to_owned(),
    }
}

pub(crate) fn heuristic() -> EvidenceSource {
    EvidenceSource::Heuristic {
        heuristic_id: "h".to_owned(),
    }
}

pub(crate) fn llm() -> EvidenceSource {
    EvidenceSource::Llm {
        provider: "p".to_owned(),
        model: "m".to_owned(),
    }
}

pub(crate) fn ludusavi() -> EvidenceSource {
    EvidenceSource::Ludusavi {
        game: "g".to_owned(),
        manifest_version: "1".to_owned(),
    }
}

pub(crate) fn system() -> EvidenceSource {
    EvidenceSource::System {
        exporter_id: "s".to_owned(),
    }
}

pub(crate) fn stats(bytes: u64, days_ago: i64) -> TargetStats {
    TargetStats {
        total_bytes: bytes,
        file_count: 1,
        dir_count: 0,
        newest_mtime: Some(NOW - Duration::days(days_ago)),
        oldest_mtime: None,
        locked_files: 0,
        cloud_only_bytes: 0,
        cloud_only_files: 0,
        largest_file_bytes: None,
        truncated: false,
    }
}

pub(crate) fn app(installed: bool) -> AppRef {
    AppRef {
        id: "app".to_owned(),
        name: "App".to_owned(),
        kind: AppKind::Application,
        source_ids: BTreeMap::new(),
        installed: Some(installed),
        process_names: vec![],
    }
}

/// Expected `selected / tag` column: `None` is selected, `Some(tag)` is not
/// selected with that reason tag.
type Expected = Option<&'static str>;

/// One row: the finding, the expected `value` and the expected selection.
fn table() -> Vec<(u8, Finding, f32, Expected)> {
    let mut rows = Vec::new();

    let mut f = finding(Category::GameSave, vec![ev(ludusavi(), 1.0)]);
    f.stats = Some(stats(50 * MIB, 3));
    f.app = Some(app(true));
    rows.push((1, f, 1.00, None));

    let mut f = finding(Category::AppConfig, vec![ev(rule(), 0.95)]);
    f.stats = Some(stats(2 * MIB, 10));
    f.app = Some(app(true));
    rows.push((2, f, 0.72, None));

    let mut f = finding(Category::AppConfig, vec![ev(heuristic(), 0.6)]);
    f.stats = Some(stats(MIB, 1095));
    f.app = Some(app(false));
    rows.push((3, f, 0.34, Some("below_threshold")));

    let mut f = finding(Category::Cache, vec![ev(heuristic(), 0.95)]);
    f.stats = Some(stats(800 * MIB, 1));
    rows.push((4, f, 0.05, Some("restorable")));

    let mut f = finding(Category::UserFiles, vec![ev(heuristic(), 0.7)]);
    f.stats = Some(stats(40 * GIB, 60));
    rows.push((5, f, 0.55, Some("too_large_default")));

    let mut f = finding(Category::DevEnvironment, vec![ev(heuristic(), 0.95)]);
    f.tags = vec!["git-unsaved".to_owned()];
    f.stats = Some(stats(300 * MIB, 1));
    rows.push((6, f, 0.71, None));

    let mut llm_ev = ev(llm(), 0.3);
    llm_ev.importance = Some(0.3);
    let mut f = finding(Category::Unknown, vec![llm_ev]);
    f.stats = Some(stats(80 * MIB, 20));
    rows.push((7, f, 0.304, None));

    let mut f = finding(Category::Credentials, vec![ev(heuristic(), 1.0)]);
    f.stats = Some(stats(2 * MIB, 100));
    rows.push((8, f, 0.98, None));

    let mut f = finding(Category::SystemSettings, vec![ev(system(), 1.0)]);
    f.sensitivity = Sensitivity::High;
    rows.push((9, f, 0.53, None));

    let mut f = finding(Category::UserFiles, vec![ev(rule(), 1.0)]);
    f.tags = vec!["cloud-synced".to_owned()];
    f.stats = Some(stats(5 * GIB, 1));
    rows.push((10, f, 0.54, Some("too_large_default")));

    let mut f = finding(Category::Reinstallable, vec![ev(heuristic(), 0.85)]);
    f.tags = vec!["git-clean".to_owned()];
    rows.push((11, f, 0.12, Some("restorable")));

    let mut f = finding(Category::SystemSettings, vec![ev(system(), 1.0)]);
    f.requires_elevation = true;
    rows.push((12, f, 0.53, Some("needs_elevation")));

    let mut llm_ev = ev(llm(), 0.3);
    llm_ev.importance = Some(0.3);
    let mut f = finding(Category::Unknown, vec![ev(heuristic(), 0.0), llm_ev]);
    f.stats = Some(stats(80 * MIB, 20));
    rows.push((13, f, 0.304, None));

    let mut f = finding(Category::Unknown, vec![ev(heuristic(), 0.0)]);
    f.stats = Some(stats(80 * MIB, 20));
    rows.push((14, f, 0.35, None));

    rows
}

#[test]
fn table_has_all_rows() {
    let rows: Vec<u8> = table().into_iter().map(|(row, _, _, _)| row).collect();
    assert_eq!(rows, (1..=14).collect::<Vec<u8>>());
}

#[test]
fn table_6_1_values() {
    let cfg = ScoringConfig::default();
    let env = env();
    for (row, f, expected, _) in table() {
        let s = score(&f, &env, NOW, &cfg);
        if row == 7 || row == 13 {
            // Boundary case: exact value, no tolerance.
            assert_eq!(s.value, 0.304, "row {row}: {s:?}");
        } else {
            assert!(
                (s.value - expected).abs() <= 0.02,
                "row {row}: value {} expected {expected} ({s:?})",
                s.value
            );
        }
        assert!((0.0..=1.0).contains(&s.value), "row {row}");
    }
}

#[test]
fn row_1_is_clamped_to_one() {
    let (_, f, _, _) = table().remove(0);
    let s = score(&f, &env(), NOW, &ScoringConfig::default());
    // 0.9475 + 0.05 = 0.9975 (f32 weights may round it either way).
    assert!((s.value - 0.9975).abs() <= 0.001, "{s:?}");
    assert!((s.components["base"] - 0.9475).abs() <= 0.001, "{s:?}");
    assert_eq!(s.components["adjustments"], 0.05);
}

#[test]
fn table_6_1_selected() {
    let cfg = ScoringConfig::default();
    let env = env();
    let mut findings = Vec::new();
    for (row, mut f, _, expected) in table() {
        f.score = Some(score(&f, &env, NOW, &cfg));
        let decision = default_selection(&f, &env, &cfg);
        assert_eq!(
            decision,
            SelectionDecision {
                selected: expected.is_none(),
                reason_tag: expected,
            },
            "row {row}: {:?}",
            f.score
        );
        findings.push((row, f, expected));
    }

    // Rows 8 (Credentials) and 9 (Sensitivity::High) are selected and
    // sensitive: one `sensitive_selected` issue for the scan, count 2.
    let mut rows: Vec<Finding> = findings.iter().map(|(_, f, _)| f.clone()).collect();
    let mut issues = Vec::new();
    select_reporting(&mut rows, &env, &cfg, &mut issues);
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert_eq!(issues[0].message_key, SENSITIVE_SELECTED_KEY);
    assert_eq!(issues[0].message_args["count"], "2");

    for ((row, _, expected), f) in findings.iter().zip(&rows) {
        assert_eq!(f.default_selected, expected.is_none(), "row {row}");
        if let Some(tag) = expected {
            assert!(f.tags.iter().any(|t| t == tag), "row {row}: {:?}", f.tags);
        }
    }
}
