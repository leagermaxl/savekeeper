//! Default selection of findings and reason tags (SPEC-09 §4.7).

use sk_core::config::ScoringConfig;
use sk_core::env::Environment;
use sk_core::model::{Category, Finding, ScanIssue, Sensitivity};

use crate::issues::{warning, SENSITIVE_SELECTED_KEY};
use crate::nested::NESTED_TAG;

/// Reason tag of an alternative way to save the parent's data, set by a
/// collector (e.g. `git-bundle`, SPEC-07 §4.4.4).
pub const ALTERNATIVE_TAG: &str = "alternative";
/// Reason tag of `Cache`/`Reinstallable` findings.
pub const RESTORABLE_TAG: &str = "restorable";
/// Reason tag of a finding that needs elevation the process does not have.
pub const NEEDS_ELEVATION_TAG: &str = "needs_elevation";
/// Reason tag of a finding larger than `max_default_item_bytes`.
pub const TOO_LARGE_DEFAULT_TAG: &str = "too_large_default";
/// Reason tag of a finding whose score is below the selection threshold.
pub const BELOW_THRESHOLD_TAG: &str = "below_threshold";

/// `message_args` key of the number of sensitive selected findings.
pub const COUNT_ARG: &str = "count";

/// Outcome of the default selection rules for one finding (SPEC-09 §4.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionDecision {
    /// Whether the finding is selected by default.
    pub selected: bool,
    /// Tag of the reason the finding is not selected; `None` when selected.
    pub reason_tag: Option<&'static str>,
}

impl SelectionDecision {
    const SELECTED: SelectionDecision = SelectionDecision {
        selected: true,
        reason_tag: None,
    };

    const fn rejected(tag: &'static str) -> SelectionDecision {
        SelectionDecision {
            selected: false,
            reason_tag: Some(tag),
        }
    }
}

/// Decides whether a finding is selected by default (SPEC-09 §4.7).
///
/// The rules are checked in order and the first that applies decides:
/// 1. tag `nested` → not selected, `nested`;
///
///    1a. tag `alternative` → not selected, `alternative`;
/// 2. `Cache`/`Reinstallable` → not selected, `restorable`;
/// 3. `requires_elevation` without `env.is_elevated` → `needs_elevation`;
/// 4. `stats.total_bytes > max_default_item_bytes` → `too_large_default`;
/// 5. `Unknown` with `value < unknown_select_threshold` or
///    `bytes > unknown_max_default_bytes` → `below_threshold`;
/// 6. `Unknown` → selected;
/// 7. `value < select_threshold` → `below_threshold`;
/// 8. otherwise selected.
///
/// `value` is `f.score`, computed beforehand by [`score`](crate::score); a
/// finding without a score counts as `value = 0`. Without `stats` the size
/// rules 4 and 5 do not apply (SPEC-09 §5).
pub fn default_selection(f: &Finding, env: &Environment, cfg: &ScoringConfig) -> SelectionDecision {
    let has_tag = |tag: &str| f.tags.iter().any(|t| t == tag);
    let value = f.score.as_ref().map_or(0.0, |s| s.value);
    let bytes = f.stats.as_ref().map(|s| s.total_bytes);

    if has_tag(NESTED_TAG) {
        return SelectionDecision::rejected(NESTED_TAG);
    }
    if has_tag(ALTERNATIVE_TAG) {
        return SelectionDecision::rejected(ALTERNATIVE_TAG);
    }
    if matches!(f.category, Category::Cache | Category::Reinstallable) {
        return SelectionDecision::rejected(RESTORABLE_TAG);
    }
    if f.requires_elevation && !env.is_elevated {
        return SelectionDecision::rejected(NEEDS_ELEVATION_TAG);
    }
    if bytes.is_some_and(|b| b > cfg.max_default_item_bytes) {
        return SelectionDecision::rejected(TOO_LARGE_DEFAULT_TAG);
    }
    if f.category == Category::Unknown {
        let too_large = bytes.is_some_and(|b| b > cfg.unknown_max_default_bytes);
        if value < cfg.unknown_select_threshold || too_large {
            return SelectionDecision::rejected(BELOW_THRESHOLD_TAG);
        }
        return SelectionDecision::SELECTED;
    }
    if value < cfg.select_threshold {
        return SelectionDecision::rejected(BELOW_THRESHOLD_TAG);
    }
    SelectionDecision::SELECTED
}

/// Applies [`default_selection`] to every finding: sets `default_selected`,
/// adds the reason tag to `tags` (kept sorted, without duplicates) and appends
/// one `scoring.sensitive_selected` warning with the number of selected
/// `Credentials` or `Sensitivity::High` findings in `message_args["count"]`,
/// when there is at least one (SPEC-09 §4.7, FR-09-07).
#[cfg_attr(not(test), allow(dead_code))] // called by run() (T-09-08)
pub(crate) fn select_reporting(
    findings: &mut [Finding],
    env: &Environment,
    cfg: &ScoringConfig,
    issues: &mut Vec<ScanIssue>,
) {
    let mut sensitive = 0u64;
    for f in findings.iter_mut() {
        let decision = default_selection(f, env, cfg);
        f.default_selected = decision.selected;
        if let Some(tag) = decision.reason_tag {
            if !f.tags.iter().any(|t| t == tag) {
                f.tags.push(tag.to_owned());
                f.tags.sort();
            }
        }
        if decision.selected
            && (f.category == Category::Credentials || f.sensitivity == Sensitivity::High)
        {
            sensitive += 1;
        }
    }
    if sensitive > 0 {
        issues.push(warning(
            SENSITIVE_SELECTED_KEY,
            None,
            &[(COUNT_ARG, sensitive.to_string())],
        ));
    }
}
