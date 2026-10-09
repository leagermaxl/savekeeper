//! The exporters of SPEC-06 §4.3 and the helpers they share.
//!
//! Each exporter lives in its own module and is listed by [`all`] in catalogue order.

mod env_vars;
mod partial;
mod programs;
mod registry_user;
mod winget;

#[cfg(test)]
pub(crate) mod test_util;

use std::collections::BTreeMap;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sk_core::model::{
    AppKind, AppRef, Category, Evidence, EvidenceSource, Finding, FindingId, IssueSeverity,
    ScanIssue, Sensitivity, Target,
};

use crate::cmd::CmdOutput;
use crate::exporter::{Availability, ExportedFile, SystemExporter};

pub(crate) use env_vars::EnvVarsExporter;
pub(crate) use programs::ProgramsExporter;
pub(crate) use registry_user::RegistryUserExporter;
pub(crate) use winget::WingetExporter;

/// `reason_key` of every exporter outside Windows (SPEC-06 §5).
pub(crate) const NOT_WINDOWS: &str = "system.not_windows";

/// `ScanIssue.source` of export warnings.
const ISSUE_SOURCE: &str = "system";

/// i18n key of the evidence of every system export finding.
const EVIDENCE_KEY: &str = "evidence.system_export";

/// Longest process output kept in `ExportError::ProcessFailed`.
const FAILURE_TEXT_CHARS: usize = 2000;

/// The application all system exports belong to (`AppRef.kind = System`, SPEC-06 §4.1).
const APP_ID: &str = "windows";
const APP_NAME: &str = "Windows";

/// All exporters implemented so far, in the order of the catalogue (SPEC-06 §4.3).
///
/// `winget` comes before `programs`: the backup runs exporters in this order, and
/// `programs` reads `winget.json` for its "in winget" column.
pub(crate) fn all() -> Vec<Arc<dyn SystemExporter>> {
    vec![
        Arc::new(WingetExporter::new()),
        Arc::new(ProgramsExporter),
        Arc::new(RegistryUserExporter::new()),
        Arc::new(EnvVarsExporter::new()),
    ]
}

/// [`Availability::Unavailable`] with `reason_key`.
pub(crate) fn unavailable(reason_key: &str) -> Availability {
    Availability::Unavailable {
        reason_key: reason_key.to_owned(),
    }
}

/// Shape of a planned finding, apart from what the availability decides.
pub(crate) struct Plan<'a> {
    /// Exporter identifier.
    pub exporter_id: &'static str,
    /// Finding title: an i18n key (SPEC-11 §4.7).
    pub title: &'a str,
    /// Category of the finding (SPEC-06 §4.3).
    pub category: Category,
    /// Sensitivity of the finding (SPEC-06 §4.3).
    pub sensitivity: Sensitivity,
    /// Exporter parameters (`Target::SystemExport.params`).
    pub params: serde_json::Value,
}

/// The `Target::SystemExport` finding of `plan` for `avail` (SPEC-06 §4.1).
///
/// `None` for [`Availability::Unavailable`]; [`Availability::NeedsElevation`] sets
/// `requires_elevation` (FR-06-03). The facts of [`Availability::Available`] become the
/// evidence arguments.
pub(crate) fn plan_finding(plan: Plan<'_>, avail: &Availability) -> Option<Finding> {
    let (requires_elevation, message_args) = match avail {
        Availability::Available { details, .. } => (false, details.clone()),
        Availability::NeedsElevation { .. } => (true, BTreeMap::new()),
        Availability::Unavailable { .. } => return None,
    };
    let target = Target::SystemExport {
        exporter_id: plan.exporter_id.to_owned(),
        params: plan.params,
    };
    Some(Finding {
        id: FindingId::for_target(&target),
        target,
        category: plan.category,
        app: Some(AppRef {
            id: APP_ID.to_owned(),
            name: APP_NAME.to_owned(),
            kind: AppKind::System,
            source_ids: BTreeMap::new(),
            installed: None,
            process_names: Vec::new(),
        }),
        title: plan.title.to_owned(),
        evidence: vec![Evidence {
            source: EvidenceSource::System {
                exporter_id: plan.exporter_id.to_owned(),
            },
            message_key: EVIDENCE_KEY.to_owned(),
            message_args,
            confidence: 1.0,
            importance: None,
        }],
        stats: None,
        sensitivity: plan.sensitivity,
        score: None,
        default_selected: false,
        requires_elevation,
        tags: Vec::new(),
        children: Vec::new(),
        notes_key: None,
    })
}

/// A warning of an export run.
pub(crate) fn warning(message_key: &str, message_args: BTreeMap<String, String>) -> ScanIssue {
    ScanIssue {
        severity: IssueSeverity::Warning,
        source: ISSUE_SOURCE.to_owned(),
        path: None,
        message_key: message_key.to_owned(),
        message_args,
    }
}

/// Writes `content` to `target_dir/rel_path` and describes the file.
///
/// Only `target_dir` is written (principle P1); it is created if missing.
pub(crate) fn write_file(
    target_dir: &Path,
    rel_path: &str,
    content: &[u8],
) -> io::Result<ExportedFile> {
    std::fs::create_dir_all(target_dir)?;
    std::fs::write(target_dir.join(rel_path), content)?;
    Ok(ExportedFile {
        rel_path: PathBuf::from(rel_path),
        bytes: content.len() as u64,
        blake3: blake3::hash(content).to_hex().to_string(),
    })
}

/// Describes the existing file `target_dir/rel_path` (written by an external process):
/// size and BLAKE3 hash, read as a stream.
pub(crate) fn describe_file(
    target_dir: &Path,
    rel_path: impl AsRef<Path>,
) -> io::Result<ExportedFile> {
    let rel_path = rel_path.as_ref();
    let mut file = File::open(target_dir.join(rel_path))?;
    let mut hasher = blake3::Hasher::new();
    let bytes = io::copy(&mut file, &mut hasher)?;
    Ok(ExportedFile {
        rel_path: rel_path.to_path_buf(),
        bytes,
        blake3: hasher.finalize().to_hex().to_string(),
    })
}

/// What a failed process said: stderr, else stdout (winget and `reg.exe` may report
/// errors there), cut to [`FAILURE_TEXT_CHARS`] for [`ExportError::ProcessFailed`].
///
/// [`ExportError::ProcessFailed`]: crate::ExportError::ProcessFailed
pub(crate) fn failure_text(output: &CmdOutput) -> String {
    let text = match output.stderr.trim() {
        "" => output.stdout.trim(),
        stderr => stderr,
    };
    text.chars().take(FAILURE_TEXT_CHARS).collect()
}
