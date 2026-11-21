//! Backup writer: archive, manifest, report, encryption and verification (SPEC-10).

mod archive_path;
mod manifest;
mod schema;
mod serde_util;
mod types;

pub use manifest::{
    BackupManifest, EntryFlag, EntryKind, ErrorCode, ExportFile, ExportStatus, FileAttribute,
    FindingStatus, LockedEntry, LockedProcess, ManifestContainer, ManifestEntry, ManifestError,
    ManifestExport, ManifestFinding, ManifestFindingStats, ManifestTarget, ManifestTotals,
    Verification, VerificationMismatch,
};
pub use schema::manifest_json_schema;
/// Backup container format (`config.backup.format`, SPEC-01 §4.8.2).
pub use sk_core::config::BackupFormat;
pub use types::{
    BackupEstimate, BackupOptions, BackupRequest, BackupResult, BackupStatus, BackupTarget,
    EstimateBlocker, EstimateWarning, FsKind, FsLimitSubject, Passphrase, RetryPolicy,
};
