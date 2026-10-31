//! Finding merging, importance scoring and default selection (SPEC-09).
//!
//! The `scoring` config section is typed in `sk-core::config` (SPEC-01 §4.8.2)
//! and re-exported here; [`validate_config`] replaces invalid values with the
//! defaults of SPEC-09 §4.1/§4.6 (§4.10).
//! [`authority`] ranks findings by their most authoritative evidence (§4.2).
//! [`merge_by_id`] merges findings with the same id (§4.3).

mod authority;
mod config;
mod issues;
mod merge;
mod order;
mod sanitize;

pub use authority::{authority, evidence_authority, source_priority, Authority};
pub use config::{validate_config, BAD_CONFIG_KEY, ISSUE_SOURCE};
pub use issues::{BAD_NUMBER_KEY, CATEGORY_CONFLICT_KEY, NO_EVIDENCE_KEY};
pub use merge::{merge_by_id, CONFLICT_TAG};
pub use sk_core::config::{default_weights, CategoryWeights, ScoringConfig};

#[cfg(test)]
mod authority_tests;
#[cfg(test)]
mod config_tests;
#[cfg(test)]
mod merge_tests;
#[cfg(test)]
mod sanitize_tests;
