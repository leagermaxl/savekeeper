//! `manifest.json` v1: machine-readable description of a backup (SPEC-10 §4.4).
//!
//! The JSON form is the contract for restore (SPEC-13); its JSON Schema is
//! [`manifest_json_schema`](crate::manifest_json_schema).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sk_core::config::BackupFormat;
use sk_core::model::{
    AppRef, Category, EnvironmentSnapshot, FindingId, RegHive, Sensitivity, Target,
};
use sk_core::template::PathTemplate;
use specta::Type;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::schema as s;
use crate::types::BackupStatus;

/// `manifest.json` of a backup (SPEC-10 §4.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(title = "SaveKeeper backup manifest v1")]
pub struct BackupManifest {
    /// Always [`BackupManifest::FORMAT`].
    pub format: String,
    /// [`BackupManifest::SCHEMA_VERSION`] when written.
    pub schema_version: u32,
    /// Overall status.
    pub status: BackupStatus,
    /// Backup identifier.
    #[schemars(with = "s::Uuid")]
    pub backup_id: Uuid,
    /// Scan the backup was made from (`ScanReport.scan_id`).
    #[schemars(with = "s::Uuid")]
    pub scan_id: Uuid,
    /// Version of the program that wrote the backup.
    pub app_version: String,
    /// Backup start.
    #[serde(with = "time::serde::rfc3339")]
    #[specta(type = String)]
    #[schemars(with = "s::DateTime")]
    pub created_at: OffsetDateTime,
    /// Backup end.
    #[serde(with = "time::serde::rfc3339")]
    #[specta(type = String)]
    #[schemars(with = "s::DateTime")]
    pub finished_at: OffsetDateTime,
    /// Container of the backup.
    pub container: ManifestContainer,
    /// Anonymized environment of the source machine (SPEC-02 §6).
    #[schemars(with = "s::EnvironmentSnapshot")]
    pub environment: EnvironmentSnapshot,
    /// Snapshot of the selected findings, without resolved paths.
    pub findings: Vec<ManifestFinding>,
    /// One entry per written file, in write order.
    pub entries: Vec<ManifestEntry>,
    /// Results of system exports.
    pub exports: Vec<ManifestExport>,
    /// Files that could not be read because other processes lock them.
    pub locked: Vec<LockedEntry>,
    /// Errors.
    pub errors: Vec<ManifestError>,
    /// Totals.
    pub totals: ManifestTotals,
    /// Result of the verification after writing (FR-10-09).
    pub verification: Verification,
}

impl BackupManifest {
    /// Value of the `format` field.
    pub const FORMAT: &'static str = "savekeeper-backup";
    /// Current format version.
    pub const SCHEMA_VERSION: u32 = 1;
}

/// Container of a backup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ManifestContainer {
    /// Zip archive or plain folder.
    #[schemars(with = "s::BackupFormat")]
    pub format: BackupFormat,
    /// The zip is encrypted with age (FR-10-10).
    pub encrypted: bool,
    /// Zip compression level `0..=9`.
    pub compression_level: u8,
}

/// Snapshot of a selected finding (SPEC-02 §2.1), without resolved paths.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ManifestFinding {
    /// Finding identifier.
    #[schemars(with = "s::FindingId")]
    pub id: FindingId,
    /// Display title.
    pub title: String,
    /// Kind of data.
    #[schemars(with = "s::Category")]
    pub category: Category,
    /// Application or game.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<s::AppRef>")]
    pub app: Option<AppRef>,
    /// What was saved.
    pub target: ManifestTarget,
    /// How sensitive the data is.
    #[schemars(with = "s::Sensitivity")]
    pub sensitivity: Sensitivity,
    /// Folder of the finding's files: `files/<id>` (file targets).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_dir: Option<String>,
    /// Path of the single archive file: `registry/<id>.reg` (registry targets).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_path: Option<String>,
    /// What was written for the finding (file targets).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stats: Option<ManifestFindingStats>,
    /// Result for the finding.
    pub status: FindingStatus,
}

/// `sk_core::model::Target` without resolved paths.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ManifestTarget {
    /// A set of files under a root.
    FileSet {
        /// Root as a template.
        #[schemars(with = "s::PathTemplate")]
        root: PathTemplate,
        /// Globs relative to `root`; empty means `**`.
        include: Vec<String>,
        /// Globs relative to `root`.
        exclude: Vec<String>,
    },
    /// A single file.
    File {
        /// File path as a template.
        #[schemars(with = "s::PathTemplate")]
        path: PathTemplate,
    },
    /// A registry key.
    Registry {
        /// Registry hive.
        #[schemars(with = "s::RegHive")]
        hive: RegHive,
        /// Key path inside the hive.
        key: String,
        /// Subkeys included.
        recursive: bool,
    },
    /// Output of a system exporter (SPEC-06).
    SystemExport {
        /// Exporter identifier.
        exporter_id: String,
        /// Exporter-specific parameters.
        params: serde_json::Value,
    },
}

impl From<&Target> for ManifestTarget {
    fn from(target: &Target) -> Self {
        match target {
            Target::FileSet {
                root,
                include,
                exclude,
                ..
            } => Self::FileSet {
                root: root.clone(),
                include: include.clone(),
                exclude: exclude.clone(),
            },
            Target::File { path, .. } => Self::File { path: path.clone() },
            Target::Registry {
                hive,
                key,
                recursive,
            } => Self::Registry {
                hive: *hive,
                key: key.clone(),
                recursive: *recursive,
            },
            Target::SystemExport {
                exporter_id,
                params,
            } => Self::SystemExport {
                exporter_id: exporter_id.clone(),
                params: params.clone(),
            },
        }
    }
}

/// Size and number of files written for a finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ManifestFindingStats {
    /// Bytes written.
    pub total_bytes: u64,
    /// Files written.
    pub file_count: u64,
}

/// Result for one finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FindingStatus {
    /// Everything is saved.
    Complete,
    /// Some files are locked, missing or failed.
    Partial,
    /// Nothing could be saved.
    Failed,
    /// Nothing to save: the finding has no files (§5).
    Skipped,
}

/// One written file, empty folder or symbolic link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ManifestEntry {
    /// Finding the entry belongs to.
    #[schemars(with = "s::FindingId")]
    pub finding_id: FindingId,
    /// Kind of entry.
    pub kind: EntryKind,
    /// Source path as a template; never an absolute path with the user name.
    #[schemars(with = "s::PathTemplate")]
    pub source: PathTemplate,
    /// Path in the archive, separator `/`.
    pub archive_path: String,
    /// Original file name if it was escaped in `archive_path` (§4.2).
    pub original_name: Option<String>,
    /// Size in bytes; `0` for folders and links.
    pub size: u64,
    /// Modification time.
    #[serde(default, with = "time::serde::rfc3339::option")]
    #[specta(type = Option<String>)]
    #[schemars(with = "Option<s::DateTime>")]
    pub mtime: Option<OffsetDateTime>,
    /// Creation time.
    #[serde(default, with = "time::serde::rfc3339::option")]
    #[specta(type = Option<String>)]
    #[schemars(with = "Option<s::DateTime>")]
    pub ctime: Option<OffsetDateTime>,
    /// File attributes.
    pub attributes: Vec<FileAttribute>,
    /// BLAKE3 of the content, 64 hex characters; `null` for folders and links.
    pub blake3: Option<String>,
    /// Notes about the entry.
    pub flags: Vec<EntryFlag>,
    /// Target of a symbolic link or junction, not followed (§4.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_target: Option<String>,
}

/// Kind of a [`ManifestEntry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    /// A regular file.
    File,
    /// An empty folder.
    Dir,
    /// A symbolic link or junction; the content is not copied.
    Symlink,
}

/// Saved file attribute.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    Type,
    JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum FileAttribute {
    /// `FILE_ATTRIBUTE_READONLY`.
    Readonly,
    /// `FILE_ATTRIBUTE_HIDDEN`.
    Hidden,
    /// `FILE_ATTRIBUTE_SYSTEM`.
    System,
    /// `FILE_ATTRIBUTE_ARCHIVE`.
    Archive,
}

/// Note about a [`ManifestEntry`].
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    Type,
    JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum EntryFlag {
    /// Size or mtime changed while the file was read; it is saved anyway (§4.3).
    ChangedDuringRead,
    /// The source path is not valid UTF-8 and was replaced lossily (SPEC-02 §5).
    NonUtf8Path,
    /// The name was escaped in `archive_path`; see `original_name` (§4.2).
    NameEscaped,
}

/// Result of one system export.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ManifestExport {
    /// Finding of the export.
    #[schemars(with = "s::FindingId")]
    pub finding_id: FindingId,
    /// Exporter identifier (SPEC-06).
    pub exporter_id: String,
    /// Parameters the exporter ran with.
    pub params: serde_json::Value,
    /// Files it produced.
    pub files: Vec<ExportFile>,
    /// Result.
    pub status: ExportStatus,
    /// It ran through the elevation helper (SPEC-14).
    pub elevated: bool,
}

/// A file produced by a system export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ExportFile {
    /// Path in the archive, `system/<exporter_id>/...`.
    pub archive_path: String,
    /// Size in bytes.
    pub size: u64,
    /// BLAKE3 of the content, 64 hex characters.
    pub blake3: String,
}

/// Result of a system export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExportStatus {
    /// Exported.
    Complete,
    /// The exporter failed.
    Failed,
    /// The user declined the UAC prompt.
    ElevationDenied,
    /// The exporter is not available on this machine.
    Unavailable,
}

/// A file that could not be read because other processes lock it (FR-10-06).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct LockedEntry {
    /// Finding of the file.
    #[schemars(with = "s::FindingId")]
    pub finding_id: FindingId,
    /// Source path as a template.
    #[schemars(with = "s::PathTemplate")]
    pub source: PathTemplate,
    /// Processes holding the file (Restart Manager).
    pub processes: Vec<LockedProcess>,
}

/// A process holding a locked file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct LockedProcess {
    /// Executable name, e.g. `chrome.exe`.
    pub name: String,
    /// Process id.
    pub pid: u32,
    /// Application name from Restart Manager (`strAppName`).
    pub app_name: String,
}

/// An error during the backup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ManifestError {
    /// Finding the error belongs to; `null` for errors of the whole backup.
    #[schemars(with = "Option<s::FindingId>")]
    pub finding_id: Option<FindingId>,
    /// Source path as a template, if the error concerns a file.
    #[schemars(with = "Option<s::PathTemplate>")]
    pub source: Option<PathTemplate>,
    /// Error code.
    pub code: ErrorCode,
    /// OS or tool message, for diagnostics.
    pub message: String,
}

/// Code of a [`ManifestError`] (§4.4, FR-10-15).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Access is denied.
    AccessDenied,
    /// The file or the finding's root no longer exists.
    NotFound,
    /// Locked by another process.
    Locked,
    /// Other I/O error.
    Io,
    /// The path is too long.
    PathTooLong,
    /// The name cannot be stored.
    NameInvalid,
    /// A system export failed.
    ExportFailed,
    /// The user declined the UAC prompt.
    ElevationDenied,
    /// The backup was cancelled.
    Cancelled,
    /// Cloud-only file, not downloaded (FR-10-15).
    CloudOnly,
}

/// Totals of a backup.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ManifestTotals {
    /// Files written.
    pub files: u64,
    /// Bytes read from the sources.
    pub bytes: u64,
    /// Size of the archive.
    pub archive_bytes: u64,
    /// Locked files.
    pub locked: u64,
    /// Errors.
    pub errors: u64,
}

/// Result of the verification after writing (FR-10-09).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct Verification {
    /// Verification ran.
    pub performed: bool,
    /// Every entry matched.
    pub ok: bool,
    /// Entries checked.
    pub checked_files: u64,
    /// Entries that did not match.
    pub mismatches: Vec<VerificationMismatch>,
}

/// An entry whose content read back differs from the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct VerificationMismatch {
    /// Path in the archive.
    pub archive_path: String,
    /// BLAKE3 in the manifest.
    pub expected_blake3: String,
    /// BLAKE3 read back; `null` if the entry is missing or unreadable.
    pub actual_blake3: Option<String>,
}
