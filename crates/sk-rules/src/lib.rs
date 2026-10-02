//! Known-location rules engine with built-in YAML rules (SPEC-04).
//!
//! [`schema`] is the serde model of the YAML rule format (schema v1);
//! [`RuleError`] reports problems in rule files.

mod error;
pub mod schema;

pub use error::RuleError;
