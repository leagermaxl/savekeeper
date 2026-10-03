//! Known-location rules engine with built-in YAML rules (SPEC-04).
//!
//! [`schema`] is the serde model of the YAML rule format (schema v1);
//! [`compile`] validates parsed files and compiles their globs (§4.4);
//! [`diagnostic`] reports problems with line numbers for the CLI;
//! [`RuleSet`] loads the built-in and user rules and merges them (§4.4, §4.6);
//! [`conditions`] evaluates rule conditions with a per-scan cache (§4.3);
//! [`registry`] probes registry keys for `registry_exists`;
//! [`RuleError`] reports problems in rule files.

pub mod compile;
pub mod conditions;
pub mod diagnostic;
mod error;
pub mod registry;
pub mod schema;
mod set;
mod win;

pub use compile::{CompiledRule, CompiledTarget};
pub use conditions::{ConditionEvaluator, ConditionOutcome, APP_RUNNING_TAG};
pub use diagnostic::{DiagnosticSeverity, RuleDiagnostic};
pub use error::RuleError;
pub use registry::{KeyState, MemRegistry, RegistryProbe, SystemRegistry};
pub use set::{RuleSet, RuleSource};
