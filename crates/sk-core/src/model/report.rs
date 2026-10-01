//! `ScanReport` and the anonymized snapshots it contains (SPEC-02 §6).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use specta::Type;
use time::OffsetDateTime;
use uuid::Uuid;

use super::{Category, CollectorToggles, Finding, FolderSummary, LlmMode, ScanIssue};
use crate::env::{DriveInfo, DriveKind, Environment, KnownFolder, LauncherInfo, OsInfo};
use crate::privacy::{redact, REDACTED};
use crate::template::{PathTemplate, Token};

/// Result of a scan; saved to `savekeeper-data/scans/<scan_id>.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub struct ScanReport {
    /// Format version, [`ScanReport::SCHEMA_VERSION`] when written.
    pub schema_version: u32,
    /// Scan identifier.
    pub scan_id: Uuid,
    /// Version of the program that made the scan.
    pub app_version: String,
    /// Scan start.
    #[serde(with = "time::serde::rfc3339")]
    #[specta(type = String)]
    pub started_at: OffsetDateTime,
    /// Scan end.
    #[serde(with = "time::serde::rfc3339")]
    #[specta(type = String)]
    pub finished_at: OffsetDateTime,
    /// Anonymized environment.
    pub environment: EnvironmentSnapshot,
    /// Options the scan ran with.
    pub options: ScanOptionsSnapshot,
    /// Findings, sorted by category order, then score descending.
    pub findings: Vec<Finding>,
    /// Folders that stayed unknown ("Not recognized" in the UI).
    pub unknown_summaries: Vec<FolderSummary>,
    /// Problems and notes.
    pub issues: Vec<ScanIssue>,
    /// Totals (SPEC-09 §4.9).
    pub totals: Totals,
}

/// Reading a saved report failed.
#[derive(Debug, thiserror::Error)]
pub enum ReportError {
    /// Not valid report JSON.
    #[error("invalid scan report: {0}")]
    Json(#[from] serde_json::Error),
    /// Written by a newer version of the program.
    #[error("scan report schema {found} is newer than the supported {supported}")]
    UnsupportedVersion {
        /// Version in the file.
        found: u32,
        /// Newest version this program reads.
        supported: u32,
    },
}

impl ScanReport {
    /// Current format version.
    pub const SCHEMA_VERSION: u32 = 1;

    /// Reads a report; accepts `schema_version <= SCHEMA_VERSION`.
    pub fn from_json(json: &str) -> Result<Self, ReportError> {
        #[derive(Deserialize)]
        struct Header {
            schema_version: u32,
        }
        let header: Header = serde_json::from_str(json)?;
        if header.schema_version > Self::SCHEMA_VERSION {
            return Err(ReportError::UnsupportedVersion {
                found: header.schema_version,
                supported: Self::SCHEMA_VERSION,
            });
        }
        Ok(serde_json::from_str(json)?)
    }
}

/// Totals over findings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub struct Totals {
    /// Per category.
    pub by_category: BTreeMap<Category, CategoryTotals>,
    /// Over all findings.
    pub all: CategoryTotals,
    /// Selected findings with high sensitivity.
    pub sensitive_selected: u32,
    /// Findings in the Unknown category.
    pub unknown_count: u32,
    /// Findings that need administrator rights.
    pub needs_elevation_count: u32,
    /// Findings over the default size limit.
    pub too_large_count: u32,
}

/// Count and size of findings, all and selected.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub struct CategoryTotals {
    /// Number of findings.
    pub count: u32,
    /// Their size.
    pub bytes: u64,
    /// Number of selected findings.
    pub selected_count: u32,
    /// Size of selected findings.
    pub selected_bytes: u64,
}

/// Anonymized copy of [`Environment`], used in `ScanReport` and the backup
/// manifest (SPEC-10 §4.4); SPEC-13 relies on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub struct EnvironmentSnapshot {
    /// Operating system version.
    pub os: OsInfo,
    /// Computer name.
    pub machine_name: String,
    /// Known folders relative to `{HOME}`, `{ONEDRIVE}` or `{DRIVE:X}`, without the user name.
    pub known_folders: BTreeMap<KnownFolder, PathTemplate>,
    /// Drives without free space.
    pub drives: Vec<DriveSnapshot>,
    /// Launchers.
    pub launchers: Vec<LauncherSnapshot>,
    /// The scan ran with administrator rights.
    pub is_elevated: bool,
}

/// A drive in [`EnvironmentSnapshot`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub struct DriveSnapshot {
    /// Drive letter.
    pub letter: char,
    /// Drive type.
    pub kind: DriveKind,
    /// File system.
    pub fs: Option<String>,
    /// Volume label.
    pub label: Option<String>,
    /// Volume serial number, to detect a changed drive letter (SPEC-13).
    pub volume_serial: Option<u32>,
}

/// A launcher in [`EnvironmentSnapshot`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub struct LauncherSnapshot {
    /// "steam", "epic" ...
    pub id: String,
    /// Root as a template, e.g. `{PROGRAMFILES_X86}\Steam`.
    pub root: Option<PathTemplate>,
    /// Number of installed games.
    pub game_count: u32,
}

/// Options a report was built with: anonymized copy of `ScanOptions` (SPEC-01 §4.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub struct ScanOptionsSnapshot {
    /// Scan roots as templates.
    pub roots: Vec<PathTemplate>,
    /// Collectors that ran.
    pub collectors: CollectorToggles,
    /// LLM mode.
    pub llm: LlmMode,
    /// Depth limit, if set.
    pub max_depth: Option<u32>,
}

impl EnvironmentSnapshot {
    /// Snapshot of `env` by the rules of SPEC-02 §6.
    pub fn from_env(env: &Environment) -> Self {
        let known_folders = env
            .known_folders
            .iter()
            .map(|(&folder, path)| {
                let allow = |token: &Token| match token {
                    Token::Folder(KnownFolder::Home) => folder != KnownFolder::Home,
                    Token::OneDrive | Token::Drive(_) => true,
                    _ => false,
                };
                let template = PathTemplate::from_path_with(path, env, &allow);
                (folder, redacted(&template, env))
            })
            .collect();
        Self {
            os: env.os.clone(),
            machine_name: env.machine_name.clone(),
            known_folders,
            drives: env.drives.iter().map(DriveSnapshot::from).collect(),
            launchers: env
                .launchers
                .iter()
                .map(|l| LauncherSnapshot::from_launcher(l, env))
                .collect(),
            is_elevated: env.is_elevated,
        }
    }
}

impl From<&DriveInfo> for DriveSnapshot {
    fn from(drive: &DriveInfo) -> Self {
        Self {
            letter: drive.letter,
            kind: drive.kind,
            fs: drive.fs.clone(),
            label: drive.label.clone(),
            volume_serial: drive.volume_serial,
        }
    }
}

impl LauncherSnapshot {
    /// Snapshot of a launcher; the root is not written as `{STEAM}`.
    pub fn from_launcher(launcher: &LauncherInfo, env: &Environment) -> Self {
        let root = launcher.root.as_deref().map(|root| {
            let template = PathTemplate::from_path_with(root, env, &|t| *t != Token::Steam);
            redacted(&template, env)
        });
        Self {
            id: launcher.id.clone(),
            root,
            game_count: u32::try_from(launcher.games.len()).unwrap_or(u32::MAX),
        }
    }
}

/// The template with personal data removed. If redaction breaks the syntax
/// (a user named like a token), the whole template is replaced.
fn redacted(template: &PathTemplate, env: &Environment) -> PathTemplate {
    PathTemplate::parse(&redact(template.as_str(), env))
        .or_else(|_| PathTemplate::parse(REDACTED))
        // `<redacted>` is a valid one-segment template, so this is not reached.
        .unwrap_or_else(|_| template.clone())
}
