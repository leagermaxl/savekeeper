//! Domain types exchanged by all crates and the UI (SPEC-02 §2, §6).
//!
//! Their JSON form is a public contract: UI, `scans/*.json`, backup manifest.
//! Enum variants are `snake_case`; tagged enums use the `kind` field.

mod evidence;
mod finding;
mod id;
mod scan;
mod stats;

pub use evidence::{Evidence, EvidenceSource};
pub use finding::{AppKind, AppRef, Category, Finding, RegHive, Target};
pub use id::FindingId;
pub use scan::{CollectorToggles, LlmMode};
pub use stats::{IssueSeverity, ScanIssue, Score, Sensitivity, TargetStats};
