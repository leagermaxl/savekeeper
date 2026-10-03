//! Known-location rules engine with built-in YAML rules (SPEC-04).
//!
//! [`schema`] is the serde model of the YAML rule format (schema v1);
//! [`compile`] validates parsed files and compiles their globs (§4.4);
//! [`diagnostic`] reports problems with line numbers for the CLI;
//! [`RuleSet`] loads the built-in and user rules and merges them (§4.4, §4.6);
//! [`conditions`] evaluates rule conditions with a per-scan cache (§4.3);
//! registry keys of `registry_exists` and registry targets are read through
//! `sk_core::registry::RegistryReader` (SPEC-02 §3.4);
//! [`expand`] turns targets of matched rules into findings and claimed paths (§4.5);
//! [`RulesCollector`] runs all rules as the `rules` scan collector (§4.5);
//! [`RuleError`] reports problems in rule files.

mod collector;
pub mod compile;
pub mod conditions;
pub mod diagnostic;
mod error;
pub mod expand;
mod once;
pub mod schema;
mod set;

pub use collector::RulesCollector;
pub use compile::{CompiledRule, CompiledTarget};
pub use conditions::{ConditionEvaluator, ConditionOutcome, APP_RUNNING_TAG};
pub use diagnostic::{DiagnosticSeverity, RuleDiagnostic};
pub use error::RuleError;
pub use expand::{RuleOutput, TargetExpander};
pub use set::{RuleSet, RuleSource};
