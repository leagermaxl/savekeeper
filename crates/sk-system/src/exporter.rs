//! The `SystemExporter` trait and the types it works with (SPEC-06 §4.1).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use sk_core::env::Environment;
use sk_core::events::EventSink;
use sk_core::model::{Finding, ScanIssue};
use sk_core::CancellationToken;

use crate::error::ExportError;

/// One system export: winget list, registry branches, Wi-Fi profiles, drivers and so on
/// (SPEC-06 §4.3).
///
/// During a scan only [`detect`](Self::detect) and [`plan`](Self::plan) are called; the
/// export itself ([`run`](Self::run)) is executed by the backup (SPEC-10).
#[async_trait]
pub trait SystemExporter: Send + Sync {
    /// Stable exporter identifier: `"winget"`, `"programs"`, `"wifi"`, ... (SPEC-06 §4.3).
    ///
    /// It is the `exporter_id` of `Target::SystemExport` and the name of the output folder
    /// `<backup>/system/<id>/`.
    fn id(&self) -> &'static str;

    /// Checks whether the export can run on this machine (FR-06-01).
    ///
    /// Must be fast (≤ 500 ms) and free of side effects. Off Windows it returns
    /// [`Availability::Unavailable`] with `reason_key = "system.not_windows"` (SPEC-06 §5).
    fn detect(&self, env: &Environment) -> Availability;

    /// Builds the scan finding with `Target::SystemExport` for the given availability
    /// (FR-06-01, SPEC-06 §4.1).
    ///
    /// Returns `None` when no finding is created, in particular for
    /// [`Availability::Unavailable`]. For [`Availability::NeedsElevation`] the finding gets
    /// `requires_elevation: true` (FR-06-03).
    fn plan(&self, env: &Environment, avail: &Availability) -> Option<Finding>;

    /// Runs the export, writing files **only** into [`ExportContext::target_dir`]
    /// (FR-06-02, principle P1).
    ///
    /// `params` are the exporter parameters from the finding as edited in the UI
    /// (SPEC-06 §4.3), e.g. `{ "include_keys": false }` for `wifi`.
    async fn run(
        &self,
        ctx: &ExportContext<'_>,
        params: &serde_json::Value,
    ) -> Result<ExportResult, ExportError>;
}

/// Result of [`SystemExporter::detect`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    /// The export can run with the current rights.
    Available {
        /// Expected size of the export, if it can be estimated cheaply.
        estimated_bytes: Option<u64>,
        /// Facts found by detection (e.g. winget version), shown in the UI and the checklist.
        details: BTreeMap<String, String>,
    },
    /// The export needs administrator rights (FR-06-03): via the elevation helper
    /// (SPEC-14) or skipped with a message.
    NeedsElevation {
        /// Expected size of the export, if it can be estimated cheaply.
        estimated_bytes: Option<u64>,
    },
    /// The export cannot run here (winget missing, no Wi-Fi profiles, not Windows);
    /// no finding is created.
    Unavailable {
        /// i18n key of the reason, e.g. `"system.winget.missing"`.
        reason_key: String,
    },
}

/// Everything [`SystemExporter::run`] gets from its caller (SPEC-10 or the elevation helper).
#[derive(Debug, Clone, Copy)]
pub struct ExportContext<'a> {
    /// Environment of the scanned machine.
    pub env: &'a Environment,
    /// The only folder the exporter writes to: `<backup>/system/<exporter_id>/`.
    pub target_dir: &'a Path,
    /// Cancellation of the backup (principle P8); running processes are killed.
    pub cancel: &'a CancellationToken,
    /// Progress and log events.
    pub events: &'a EventSink,
    /// Whether the exporter runs inside the elevated helper (SPEC-14).
    pub elevated: bool,
}

/// Successful result of [`SystemExporter::run`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportResult {
    /// Created files, relative to [`ExportContext::target_dir`].
    pub files: Vec<ExportedFile>,
    /// Non-fatal problems (e.g. a Wi-Fi profile needing administrator rights).
    pub warnings: Vec<ScanIssue>,
    /// How the export is restored (SPEC-13, `checklist.md`).
    pub restore_hint: RestoreHint,
}

/// A file created by an export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedFile {
    /// Path relative to [`ExportContext::target_dir`].
    pub rel_path: PathBuf,
    /// File size in bytes.
    pub bytes: u64,
    /// BLAKE3 hash of the content, lowercase hex.
    pub blake3: String,
}

/// How to restore an export on a new system (SPEC-13, SPEC-06 §4.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreHint {
    /// Restored by running a program, e.g. `winget import -i winget.json`.
    Command {
        /// Program name or path.
        program: String,
        /// Program arguments.
        args: Vec<String>,
    },
    /// Restored manually by the user; `key` is the i18n key of the instructions.
    Manual {
        /// i18n key of the instructions.
        key: String,
    },
    /// Nothing to restore (informational export such as `printers.json`).
    None,
}
