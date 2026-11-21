//! Request, options, estimate and result of a backup (SPEC-10 §4.1).

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sk_core::config::BackupFormat;
use sk_core::model::{FindingId, ScanReport};
use specta::Type;

use crate::manifest::BackupManifest;

/// Backup passphrase (SPEC-10 §4.7): redacted `Debug`, zeroed on drop, never logged or saved.
pub type Passphrase = secrecy::SecretString;

/// Everything needed to write one backup.
#[derive(Debug, Clone)]
pub struct BackupRequest {
    /// Scan whose findings are saved (SPEC-02 §6).
    pub report: Arc<ScanReport>,
    /// Selected findings; only these go into the backup (FR-10-01).
    pub selection: BTreeSet<FindingId>,
    /// Where to write.
    pub target: BackupTarget,
    /// How to write.
    pub options: BackupOptions,
}

/// Where the backup is written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub struct BackupTarget {
    /// Folder the backup is written into.
    #[serde(with = "crate::serde_util::lossy_path")]
    #[specta(type = String)]
    pub parent_dir: PathBuf,
    /// Name without extension; the extension follows the format (FR-10-14).
    pub name: String,
}

/// How the backup is written.
#[derive(Debug, Clone)]
pub struct BackupOptions {
    /// Zip archive or plain folder.
    pub format: BackupFormat,
    /// Zip compression level `0..=9`, from `config.backup.compression_level`.
    pub compression_level: u8,
    /// Encrypt the archive with this passphrase; zip only (FR-10-10).
    pub encryption: Option<Passphrase>,
    /// Read every entry back after writing (FR-10-09).
    pub verify: bool,
    /// On cancel keep `.partial` with an `incomplete` manifest instead of deleting it (FR-10-08).
    pub keep_partial_on_cancel: bool,
    /// Retries of files locked by other processes (FR-10-05).
    pub retry_locked: RetryPolicy,
}

/// Retries of a file that fails with a sharing or lock violation (FR-10-05).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub struct RetryPolicy {
    /// Number of retries after the first attempt.
    pub attempts: u32,
    /// Delay before each retry, in milliseconds; the last one repeats if there are fewer
    /// delays than attempts.
    pub delays_ms: Vec<u64>,
}

impl Default for RetryPolicy {
    /// `{attempts: 3, delays_ms: [200, 500, 1000]}` (FR-10-05).
    fn default() -> Self {
        Self {
            attempts: 3,
            delays_ms: vec![200, 500, 1000],
        }
    }
}

/// Preliminary check of a backup before it starts (FR-10-03, SPEC-10 §4.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub struct BackupEstimate {
    /// Sum of `TargetStats.total_bytes` of the selection (plus an estimate of the exports).
    pub total_bytes: u64,
    /// Number of files.
    pub file_count: u64,
    /// `total_bytes` times the compression ratio of each category (§4.6).
    pub estimated_archive_bytes: u64,
    /// Free space at the target.
    pub free_bytes: u64,
    /// File system of the target.
    pub fs_kind: FsKind,
    /// Largest file of the selection, if the scan knows it.
    pub largest_file_bytes: Option<u64>,
    /// Reasons the backup cannot start; empty means it can.
    pub blockers: Vec<EstimateBlocker>,
    /// Things the user should know before starting.
    pub warnings: Vec<EstimateWarning>,
}

/// File system of the backup target (`GetVolumeInformationW`, SPEC-10 §4.6).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(tag = "kind", content = "name", rename_all = "snake_case")]
pub enum FsKind {
    /// NTFS.
    Ntfs,
    /// exFAT.
    ExFat,
    /// FAT32: files and the zip are limited to 4 GiB − 1 (FR-10-04).
    Fat32,
    /// ReFS.
    ReFs,
    /// A network share.
    Network,
    /// Any other file system, by its name.
    Other(String),
}

/// What exceeds a file system limit (FR-10-04).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum FsLimitSubject {
    /// A single file of the selection.
    File,
    /// The resulting archive.
    Archive,
}

/// Reason a backup cannot start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EstimateBlocker {
    /// Not enough free space at the target (FR-10-03).
    InsufficientSpace {
        /// Space the backup needs by the rules of §4.6.
        required_bytes: u64,
    },
    /// A file or the archive is too large for the target file system (FR-10-04).
    FsLimit {
        /// What is too large.
        subject: FsLimitSubject,
        /// Its size.
        bytes: u64,
        /// Largest size the file system accepts.
        max_bytes: u64,
    },
    /// The target folder is inside the root of a selected file set (§4.6).
    TargetInsideSource {
        /// That finding.
        finding: FindingId,
    },
    /// Encryption is requested for the `dir` format (FR-10-10).
    EncryptWithDir,
    /// The target folder cannot be written.
    NotWritable,
}

/// Something the user should know before the backup starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EstimateWarning {
    /// The selection contains sensitive findings; encryption is recommended.
    Sensitive {
        /// Number of such findings.
        count: u32,
    },
    /// These findings need administrator rights.
    RequiresElevation {
        /// Their ids.
        ids: Vec<FindingId>,
    },
    /// The selection is large.
    LargeSelection,
    /// The target is on the system drive and will be lost if it is formatted (§4.6).
    TargetOnSystemDrive,
    /// A `.partial` left by an interrupted backup was found at the target (§5).
    StalePartial {
        /// Its path.
        #[serde(with = "crate::serde_util::lossy_path")]
        #[specta(type = String)]
        path: PathBuf,
    },
    /// A file over 50 GiB needs explicit confirmation (§5).
    HugeFile {
        /// Finding the file belongs to.
        finding: FindingId,
        /// Its size.
        bytes: u64,
    },
    /// These findings have no stats; their size is unknown (§4.6).
    UnknownSize {
        /// Their ids.
        ids: Vec<FindingId>,
    },
}

/// Result of a backup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub struct BackupResult {
    /// Final file or folder.
    #[serde(with = "crate::serde_util::lossy_path")]
    #[specta(type = String)]
    pub output_path: PathBuf,
    /// Manifest written into the backup (§4.4).
    pub manifest: BackupManifest,
    /// Overall status; the same as `manifest.status`.
    pub status: BackupStatus,
}

/// Overall status of a backup.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum BackupStatus {
    /// Everything selected is saved.
    Complete,
    /// Saved, but some files are locked, missing or failed.
    CompleteWithIssues,
    /// Cancelled or interrupted; only a kept `.partial` has this status (FR-10-08).
    Incomplete,
    /// Verification found a mismatch (FR-10-09).
    VerifyFailed,
}
