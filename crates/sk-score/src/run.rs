//! The whole Score phase: merge, score, select, sort and count (SPEC-09 §4.1).

use sk_core::config::ScoringConfig;
use sk_core::env::Environment;
use sk_core::model::{Finding, ScanIssue, Target, Totals};
use time::OffsetDateTime;

use crate::config::validate_config;
use crate::issues::compare_issues;
use crate::merge::merge_by_id_reporting;
use crate::nested::merge_nested;
use crate::order::sort_findings;
use crate::score::score_reporting;
use crate::select::select_reporting;
use crate::totals::totals;

/// Tag of a `FileSet` without `stats`: Measure did not run for it (cancelled
/// or failed), so recency is neutral and the size rules do not apply
/// (SPEC-09 §5).
pub const UNMEASURED_TAG: &str = "unmeasured";

/// Input of [`run`].
#[derive(Debug, Clone)]
pub struct ScoreInput<'a> {
    /// Raw findings of all collectors.
    pub findings: Vec<Finding>,
    /// Environment of the scan.
    pub env: &'a Environment,
    /// The scan clock, passed in by the engine, so the result is deterministic.
    pub now: OffsetDateTime,
}

/// Output of [`run`].
#[derive(Debug, Clone, PartialEq)]
pub struct ScoreOutput {
    /// Merged, scored and selected findings in the order of §4.8.
    pub findings: Vec<Finding>,
    /// Totals over the top-level findings, selected by `default_selected`.
    pub totals: Totals,
    /// Issues of the phase, in a deterministic order.
    pub issues: Vec<ScanIssue>,
}

/// Runs the Score phase (SPEC-09): validates `cfg` (§4.10), merges findings
/// by id (§4.3, after the input checks of §5) and by nesting (§4.4), tags
/// `FileSet`s without stats `unmeasured` (§5), scores every finding (§4.5),
/// applies the default selection (§4.7), sorts the result (§4.8) and counts
/// [`Totals`] by `default_selected` (§4.9).
///
/// Issues come in phase order: `scoring.bad_config`, then the merge issues
/// (`scoring.no_evidence`, `scoring.bad_number`, `scoring.category_conflict`),
/// then the `scoring.bad_number` issues of scoring, then
/// `scoring.sensitive_selected`; within each group they are sorted, so the
/// output does not depend on the input order (NFR-09-02).
pub fn run(input: ScoreInput<'_>, cfg: &ScoringConfig) -> ScoreOutput {
    let ScoreInput { findings, env, now } = input;
    let (cfg, mut issues) = validate_config(cfg);

    let mut findings = merge_by_id_reporting(findings, &mut issues);
    tag_unmeasured(&mut findings);
    merge_nested(&mut findings, env);

    let start = issues.len();
    for f in &mut findings {
        f.score = Some(score_reporting(f, env, now, &cfg, &mut issues));
    }
    issues[start..].sort_by(compare_issues);

    select_reporting(&mut findings, env, &cfg, &mut issues);
    sort_findings(&mut findings);
    let totals = totals(&findings, |f| f.default_selected);

    ScoreOutput {
        findings,
        totals,
        issues,
    }
}

/// Adds the `unmeasured` tag (kept sorted, without duplicates) to every
/// `FileSet` without `stats` (SPEC-09 §5).
fn tag_unmeasured(findings: &mut [Finding]) {
    for f in findings {
        let unmeasured = matches!(f.target, Target::FileSet { .. }) && f.stats.is_none();
        if unmeasured && !f.tags.iter().any(|t| t == UNMEASURED_TAG) {
            f.tags.push(UNMEASURED_TAG.to_owned());
            f.tags.sort();
        }
    }
}
