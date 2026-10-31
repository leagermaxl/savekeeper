//! Finding merging, importance scoring and default selection (SPEC-09).
//!
//! The `scoring` config section is typed in `sk-core::config` (SPEC-01 §4.8.2)
//! and re-exported here; [`validate_config`] replaces invalid values with the
//! defaults of SPEC-09 §4.1/§4.6 (§4.10).
//! [`authority`] ranks findings by their most authoritative evidence (§4.2).
//! [`merge_by_id`] merges findings with the same id (§4.3), [`merge_nested`]
//! nests findings into enclosing `FileSet`s (§4.4). [`score`] computes the
//! importance score with its components (§4.5).
//! [`default_selection`] applies the default selection rules (§4.7).
//! [`totals`] counts findings per category and selection (§4.9).
//! [`run`] chains all steps into the Score phase of the engine.

mod authority;
mod config;
mod issues;
mod merge;
mod nested;
mod order;
mod run;
mod sanitize;
mod score;
mod select;
mod totals;

pub use authority::{authority, evidence_authority, source_priority, Authority};
pub use config::{validate_config, BAD_CONFIG_KEY, ISSUE_SOURCE};
pub use issues::{BAD_NUMBER_KEY, CATEGORY_CONFLICT_KEY, NO_EVIDENCE_KEY, SENSITIVE_SELECTED_KEY};
pub use merge::{merge_by_id, CONFLICT_TAG};
pub use nested::{merge_nested, NESTED_TAG, VIA_ARG};
pub use run::{run, ScoreInput, ScoreOutput, UNMEASURED_TAG};
pub use score::{
    score, ADJUSTMENTS, BASE, CONFIDENCE, IMPORTANCE_ARG, IRREPLACEABILITY, RECENCY, SIZE_PENALTY,
    USER_AUTHORED, VALUE,
};
pub use select::{
    default_selection, SelectionDecision, ALTERNATIVE_TAG, BELOW_THRESHOLD_TAG, COUNT_ARG,
    NEEDS_ELEVATION_TAG, RESTORABLE_TAG, TOO_LARGE_DEFAULT_TAG,
};
pub use sk_core::config::{default_weights, CategoryWeights, ScoringConfig};
pub use totals::totals;

#[cfg(test)]
mod authority_tests;
#[cfg(test)]
mod config_tests;
#[cfg(test)]
mod merge_tests;
#[cfg(test)]
mod nested_glob_tests;
#[cfg(test)]
mod nested_tests;
#[cfg(test)]
mod order_tests;
#[cfg(test)]
mod run_tests;
#[cfg(test)]
mod sanitize_tests;
#[cfg(test)]
mod score_table_tests;
#[cfg(test)]
mod score_tests;
#[cfg(test)]
mod select_tests;
#[cfg(test)]
mod totals_tests;
