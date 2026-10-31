//! Input checks before merging (SPEC-09 §5): findings without evidence and
//! non-finite confidence or importance.

use std::collections::BTreeMap;

use sk_core::model::{Evidence, EvidenceSource, Finding, ScanIssue};

use crate::issues::{target_path, warning, BAD_NUMBER_KEY, NO_EVIDENCE_KEY};

/// `heuristic_id` of the synthetic evidence of a finding without evidence.
pub(crate) const FALLBACK_HEURISTIC_ID: &str = "scoring-fallback";
/// `message_key` of the synthetic evidence.
pub(crate) const FALLBACK_MESSAGE_KEY: &str = "evidence.scoring_fallback";
/// Confidence of the synthetic evidence.
pub(crate) const FALLBACK_CONFIDENCE: f32 = 0.3;

/// Repairs every finding in place and reports what was repaired.
///
/// A finding without evidence trips a `debug_assert!`; in release it gets the
/// synthetic `Heuristic { "scoring-fallback" }` evidence with confidence 0.3
/// and a `scoring.no_evidence` issue. A NaN or infinite `confidence` or
/// `importance` becomes 0 with a `scoring.bad_number` issue.
pub(crate) fn sanitize(findings: &mut [Finding], issues: &mut Vec<ScanIssue>) {
    for finding in findings {
        debug_assert!(
            !finding.evidence.is_empty(),
            "finding {} has no evidence (principle P3)",
            finding.id.as_str()
        );
        if finding.evidence.is_empty() {
            add_fallback_evidence(finding, issues);
        }
        fix_numbers(finding, issues);
    }
}

/// Gives a finding without evidence the synthetic fallback evidence.
pub(crate) fn add_fallback_evidence(finding: &mut Finding, issues: &mut Vec<ScanIssue>) {
    finding.evidence.push(Evidence {
        source: EvidenceSource::Heuristic {
            heuristic_id: FALLBACK_HEURISTIC_ID.to_owned(),
        },
        message_key: FALLBACK_MESSAGE_KEY.to_owned(),
        message_args: BTreeMap::new(),
        confidence: FALLBACK_CONFIDENCE,
        importance: None,
    });
    issues.push(warning(
        NO_EVIDENCE_KEY,
        target_path(&finding.target),
        &[("finding_id", finding.id.as_str().to_owned())],
    ));
}

/// Replaces non-finite `confidence` and `importance` values with 0.
fn fix_numbers(finding: &mut Finding, issues: &mut Vec<ScanIssue>) {
    let path = target_path(&finding.target);
    let id = finding.id.as_str().to_owned();
    let mut report = |field: &str, value: f32| {
        issues.push(warning(
            BAD_NUMBER_KEY,
            path.clone(),
            &[
                ("finding_id", id.clone()),
                ("field", field.to_owned()),
                ("value", value.to_string()),
            ],
        ));
    };
    for evidence in &mut finding.evidence {
        if !evidence.confidence.is_finite() {
            report("confidence", evidence.confidence);
            evidence.confidence = 0.0;
        }
        if let Some(importance) = evidence.importance {
            if !importance.is_finite() {
                report("importance", importance);
                evidence.importance = Some(0.0);
            }
        }
    }
}
