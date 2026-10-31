//! `ScanIssue` keys and builders of `sk-score` (SPEC-09 §4.3, §5).

use std::cmp::Ordering;
use std::collections::BTreeMap;

use sk_core::model::{IssueSeverity, ScanIssue, Target};

use crate::config::ISSUE_SOURCE;

/// i18n key of a dangerous category conflict between findings with one id
/// (SPEC-09 §4.3, rule 8).
pub const CATEGORY_CONFLICT_KEY: &str = "scoring.category_conflict";

/// i18n key of a NaN or infinite confidence or importance, treated as 0
/// (SPEC-09 §5).
pub const BAD_NUMBER_KEY: &str = "scoring.bad_number";

/// i18n key of a finding without evidence, a P3 violation (SPEC-09 §5).
pub const NO_EVIDENCE_KEY: &str = "scoring.no_evidence";

/// A `Warning` issue from the `scoring` phase.
pub(crate) fn warning(key: &str, path: Option<String>, args: &[(&str, String)]) -> ScanIssue {
    ScanIssue {
        severity: IssueSeverity::Warning,
        source: ISSUE_SOURCE.to_owned(),
        path,
        message_key: key.to_owned(),
        message_args: args
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect::<BTreeMap<_, _>>(),
    }
}

/// The template of a file target, used as the anonymized issue path.
pub(crate) fn target_path(target: &Target) -> Option<String> {
    match target {
        Target::FileSet { root, .. } => Some(root.as_str().to_owned()),
        Target::File { path, .. } => Some(path.as_str().to_owned()),
        Target::Registry { .. } | Target::SystemExport { .. } => None,
    }
}

/// Total order of issues, so the issue list does not depend on input order
/// (SPEC-09 NFR-09-02).
pub(crate) fn compare_issues(a: &ScanIssue, b: &ScanIssue) -> Ordering {
    a.message_key
        .cmp(&b.message_key)
        .then_with(|| a.message_args.cmp(&b.message_args))
        .then_with(|| a.path.cmp(&b.path))
        .then_with(|| a.severity.cmp(&b.severity))
        .then_with(|| a.source.cmp(&b.source))
}
