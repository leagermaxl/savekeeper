use std::collections::BTreeMap;

use sk_core::model::{
    Category, Evidence, EvidenceSource, Finding, FindingId, IssueSeverity, RegHive, Sensitivity,
    Target,
};

use crate::sanitize::{add_fallback_evidence, sanitize};
use crate::{ISSUE_SOURCE, NO_EVIDENCE_KEY};

fn finding(evidence: Vec<Evidence>) -> Finding {
    let target = Target::Registry {
        hive: RegHive::Hkcu,
        key: r"Software\Test".to_owned(),
        recursive: true,
    };
    Finding {
        id: FindingId::for_target(&target),
        target,
        category: Category::Unknown,
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

fn ev(confidence: f32, importance: Option<f32>) -> Evidence {
    Evidence {
        source: EvidenceSource::Llm {
            provider: "p".to_owned(),
            model: "m".to_owned(),
        },
        message_key: "evidence.llm".to_owned(),
        message_args: BTreeMap::new(),
        confidence,
        importance,
    }
}

#[test]
fn fallback_evidence_matches_spec_5() {
    let mut f = finding(vec![]);
    let mut issues = Vec::new();
    add_fallback_evidence(&mut f, &mut issues);

    assert_eq!(f.evidence.len(), 1);
    let e = &f.evidence[0];
    assert_eq!(
        e.source,
        EvidenceSource::Heuristic {
            heuristic_id: "scoring-fallback".to_owned()
        }
    );
    assert_eq!(e.confidence, 0.3);

    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].severity, IssueSeverity::Warning);
    assert_eq!(issues[0].source, ISSUE_SOURCE);
    assert_eq!(issues[0].message_key, NO_EVIDENCE_KEY);
    assert_eq!(issues[0].message_args["finding_id"], f.id.as_str());
    assert!(issues[0].path.is_none());
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "has no evidence")]
fn missing_evidence_trips_debug_assert() {
    let mut issues = Vec::new();
    sanitize(&mut [finding(vec![])], &mut issues);
}

#[cfg(not(debug_assertions))]
#[test]
fn missing_evidence_gets_fallback_in_release() {
    let mut findings = [finding(vec![])];
    let mut issues = Vec::new();
    sanitize(&mut findings, &mut issues);
    assert_eq!(findings[0].evidence.len(), 1);
    assert_eq!(issues.len(), 1);
}

#[test]
fn non_finite_numbers_become_zero_with_one_issue_each() {
    let mut findings = [finding(vec![
        ev(f32::INFINITY, Some(f32::NAN)),
        ev(0.5, Some(0.4)),
        ev(f32::NEG_INFINITY, None),
    ])];
    let mut issues = Vec::new();
    sanitize(&mut findings, &mut issues);

    let e = &findings[0].evidence;
    assert_eq!((e[0].confidence, e[0].importance), (0.0, Some(0.0)));
    assert_eq!((e[1].confidence, e[1].importance), (0.5, Some(0.4)));
    assert_eq!((e[2].confidence, e[2].importance), (0.0, None));

    let got: Vec<(&str, &str)> = issues
        .iter()
        .map(|i| {
            (
                i.message_args["field"].as_str(),
                i.message_args["value"].as_str(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            ("confidence", "inf"),
            ("importance", "NaN"),
            ("confidence", "-inf")
        ]
    );
    assert!(issues
        .iter()
        .all(|i| i.message_key == crate::BAD_NUMBER_KEY));
}

#[test]
fn finite_input_is_untouched() {
    let original = [finding(vec![ev(0.9, Some(0.2))])];
    let mut findings = original.clone();
    let mut issues = Vec::new();
    sanitize(&mut findings, &mut issues);
    assert_eq!(findings, original);
    assert!(issues.is_empty());
}
