//! Errors of rule loading and compilation (SPEC-04 §4.1).

/// Error in a rule file or rule.
#[derive(thiserror::Error, Debug)]
pub enum RuleError {
    /// YAML syntax error, unknown field, wrong type or invalid path template.
    #[error("invalid rule YAML: {0}")]
    Yaml(#[from] serde_saphyr::Error),
    /// A rule violates a validation rule (SPEC-04 §4.4).
    #[error("invalid rule {rule_id}: {reason}")]
    Invalid {
        /// Id of the offending rule.
        rule_id: String,
        /// Human-readable reason.
        reason: String,
    },
    /// Two rules in one source share an id.
    #[error("duplicate rule id {0}")]
    DuplicateId(String),
}
