//! Domain types exchanged by all crates and the UI (SPEC-02 §2, §6).
//!
//! Their JSON form is a public contract: UI, `scans/*.json`, backup manifest.
//! Enum variants are `snake_case`; tagged enums use the `kind` field.

mod evidence;
mod finding;
mod id;
mod report;
mod scan;
mod stats;
mod summary;

pub use evidence::{Evidence, EvidenceSource};
pub use finding::{AppKind, AppRef, Category, Finding, RegHive, Target};
pub use id::FindingId;
pub use report::{
    CategoryTotals, DriveSnapshot, EnvironmentSnapshot, LauncherSnapshot, ReportError,
    ScanOptionsSnapshot, ScanReport, Totals,
};
pub use scan::{CollectorToggles, LlmMode};
pub use stats::{IssueSeverity, ScanIssue, Score, Sensitivity, TargetStats};
pub use summary::{ChildStat, ExtStat, FolderSummary, Marker};
