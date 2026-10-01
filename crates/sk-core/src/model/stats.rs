//! `TargetStats`, `Sensitivity`, `Score`, `ScanIssue` (SPEC-02 §2.6).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use specta::Type;
use time::OffsetDateTime;

/// Size and state of a target, filled in the Measure phase (SPEC-03).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub struct TargetStats {
    /// Logical size of all files.
    pub total_bytes: u64,
    /// Number of files.
    pub file_count: u64,
    /// Number of directories.
    pub dir_count: u64,
    /// Newest modification time.
    #[serde(default, with = "time::serde::rfc3339::option")]
    #[specta(type = Option<String>)]
    pub newest_mtime: Option<OffsetDateTime>,
    /// Oldest modification time.
    #[serde(default, with = "time::serde::rfc3339::option")]
    #[specta(type = Option<String>)]
    pub oldest_mtime: Option<OffsetDateTime>,
    /// Files that could not be opened for reading (SPEC-03).
    pub locked_files: u32,
    /// Part of `total_bytes` stored only in the cloud (placeholders, not hydrated, SPEC-03 FR-03-03).
    pub cloud_only_bytes: u64,
    /// Number of cloud-only files.
    pub cloud_only_files: u64,
    /// Largest file, for the FAT32 limit check (SPEC-10 FR-10-04).
    pub largest_file_bytes: Option<u64>,
    /// Traversal stopped by a limit or an error.
    pub truncated: bool,
}

/// How sensitive the data is.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Type,
)]
#[serde(rename_all = "snake_case")]
pub enum Sensitivity {
    /// Nothing sensitive.
    None,
    /// Somewhat personal.
    Low,
    /// Passwords, keys, tokens: warning in the UI, encryption recommended.
    High,
}

/// Importance score (SPEC-09).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub struct Score {
    /// Final value, `0.0..=1.0`.
    pub value: f32,
    /// Formula terms: "irreplaceability", "user_authored", "recency", "size_penalty" ...
    pub components: BTreeMap<String, f32>,
}

/// A problem or note produced during a scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub struct ScanIssue {
    /// How serious it is.
    pub severity: IssueSeverity,
    /// Collector or phase identifier.
    pub source: String,
    /// Related path as a template (anonymized), if any.
    pub path: Option<String>,
    /// i18n key.
    pub message_key: String,
    /// Arguments for the message.
    pub message_args: BTreeMap<String, String>,
}

/// Severity of a [`ScanIssue`].
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Type,
)]
#[serde(rename_all = "snake_case")]
pub enum IssueSeverity {
    /// For information.
    Info,
    /// Something may be missing from the result.
    Warning,
    /// A step failed.
    Error,
}
