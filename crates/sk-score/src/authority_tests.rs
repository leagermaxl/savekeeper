use std::collections::BTreeMap;

use sk_core::model::{
    Category, Evidence, EvidenceSource, Finding, FindingId, RegHive, Sensitivity, Target,
};

use super::*;

fn evidence(source: EvidenceSource, confidence: f32) -> Evidence {
    Evidence {
        source,
        message_key: "test.evidence".to_owned(),
        message_args: BTreeMap::new(),
        confidence,
        importance: None,
    }
}

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

fn rule() -> EvidenceSource {
    EvidenceSource::Rule {
        rule_id: "r".to_owned(),
    }
}

fn heuristic() -> EvidenceSource {
    EvidenceSource::Heuristic {
        heuristic_id: "h".to_owned(),
    }
}

fn llm() -> EvidenceSource {
    EvidenceSource::Llm {
        provider: "ollama".to_owned(),
        model: "m".to_owned(),
    }
}

#[test]
fn priorities_match_spec_4_2() {
    let cases = [
        (EvidenceSource::User, 6),
        (rule(), 5),
        (
            EvidenceSource::Ludusavi {
                game: "g".to_owned(),
                manifest_version: "1".to_owned(),
            },
            4,
        ),
        (
            EvidenceSource::Launcher {
                launcher: "steam".to_owned(),
            },
            4,
        ),
        (
            EvidenceSource::System {
                exporter_id: "wifi".to_owned(),
            },
            3,
        ),
        (heuristic(), 2),
        (llm(), 1),
    ];
    for (source, priority) in cases {
        assert_eq!(source_priority(&source), priority, "{source:?}");
    }
}

#[test]
fn priority_beats_confidence() {
    let low_rule = evidence_authority(&evidence(rule(), 0.1));
    let sure_heuristic = evidence_authority(&evidence(heuristic(), 1.0));
    assert!(low_rule > sure_heuristic);
}

#[test]
fn confidence_breaks_priority_ties() {
    let a = evidence_authority(&evidence(heuristic(), 0.6));
    let b = evidence_authority(&evidence(heuristic(), 0.9));
    assert!(b > a);
    assert_eq!(a, evidence_authority(&evidence(heuristic(), 0.6)));
}

#[test]
fn authority_is_max_over_evidence() {
    let f = finding(vec![
        evidence(llm(), 1.0),
        evidence(rule(), 0.7),
        evidence(rule(), 0.9),
        evidence(heuristic(), 1.0),
    ]);
    assert_eq!(authority(&f), Authority::new(5, 0.9));
}

#[test]
fn authority_does_not_depend_on_evidence_order() {
    let mut ev = vec![
        evidence(heuristic(), 0.95),
        evidence(EvidenceSource::User, 0.2),
        evidence(llm(), 0.4),
    ];
    let a = authority(&finding(ev.clone()));
    ev.reverse();
    assert_eq!(authority(&finding(ev)), a);
    assert_eq!(a, Authority::new(6, 0.2));
}

#[test]
fn finding_without_evidence_has_lowest_authority() {
    let empty = authority(&finding(vec![]));
    assert_eq!(empty, Authority::NONE);
    assert!(empty < authority(&finding(vec![evidence(llm(), 0.0)])));
}

#[test]
fn non_finite_confidence_counts_as_zero() {
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let a = evidence_authority(&evidence(heuristic(), bad));
        assert_eq!(a, Authority::new(2, 0.0));
        assert!(a < evidence_authority(&evidence(heuristic(), 0.1)));
    }
    let f = finding(vec![evidence(rule(), f32::NAN), evidence(rule(), 0.5)]);
    assert_eq!(authority(&f), Authority::new(5, 0.5));
}

#[test]
fn negative_zero_equals_zero() {
    let a = Authority::new(2, -0.0);
    let b = Authority::new(2, 0.0);
    assert_eq!(a, b);
    assert_eq!(a.cmp(&b), std::cmp::Ordering::Equal);
}
