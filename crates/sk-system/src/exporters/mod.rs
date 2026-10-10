//! The exporters of SPEC-06 §4.3 and the helpers they share.
//!
//! Each exporter lives in its own module and is listed by [`all`] in catalogue order.

mod checklist;
mod disks_info;
mod drivers;
mod env_vars;
mod fonts;
mod hosts;
mod partial;
mod powershell_modules;
mod printers;
mod programs;
mod registry_user;
mod scheduled_tasks;
mod source;
mod wifi;
mod windows_info;
mod winget;

#[cfg(test)]
mod cloud_tests;
#[cfg(test)]
pub(crate) mod test_util;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::Arc;

use sk_core::model::{
    AppKind, AppRef, Category, Evidence, EvidenceSource, Finding, FindingId, IssueSeverity,
    ScanIssue, Sensitivity, Target,
};

use crate::cmd::CmdOutput;
use crate::exporter::{Availability, ExportedFile, SystemExporter};
use source::SourceFs;

pub(crate) use checklist::ChecklistExporter;
pub(crate) use disks_info::DisksInfoExporter;
pub(crate) use drivers::DriversExporter;
pub(crate) use env_vars::EnvVarsExporter;
pub(crate) use fonts::FontsExporter;
pub(crate) use hosts::HostsExporter;
pub(crate) use powershell_modules::PowershellModulesExporter;
pub(crate) use printers::PrintersExporter;
pub(crate) use programs::ProgramsExporter;
pub(crate) use registry_user::RegistryUserExporter;
pub(crate) use scheduled_tasks::ScheduledTasksExporter;
pub(crate) use wifi::WifiExporter;
pub(crate) use windows_info::WindowsInfoExporter;
pub(crate) use winget::WingetExporter;

/// `reason_key` of every exporter outside Windows (SPEC-06 §5).
pub(crate) const NOT_WINDOWS: &str = "system.not_windows";

/// `ScanIssue.source` of export warnings.
const ISSUE_SOURCE: &str = "system";

/// i18n key of the evidence of every system export finding.
const EVIDENCE_KEY: &str = "evidence.system_export";

/// Longest process output kept in `ExportError::ProcessFailed`.
const FAILURE_TEXT_CHARS: usize = 2000;

/// Buffer size of [`copy_file`].
const COPY_BUFFER: usize = 256 * 1024;

/// The application all system exports belong to (`AppRef.kind = System`, SPEC-06 §4.1).
const APP_ID: &str = "windows";
const APP_NAME: &str = "Windows";

/// All exporters implemented so far, in the order of the catalogue (SPEC-06 §4.3).
///
/// `winget` comes before `programs`: the backup runs exporters in this order, and
/// `programs` reads `winget.json` for its "in winget" column. `checklist` comes after
/// `winget`, `drivers`, `disks-info` and `windows-info`, whose files it reads.
pub(crate) fn all() -> Vec<Arc<dyn SystemExporter>> {
    vec![
        Arc::new(WingetExporter::new()),
        Arc::new(ProgramsExporter),
        Arc::new(WifiExporter::new()),
        Arc::new(DriversExporter::new()),
        Arc::new(RegistryUserExporter::new()),
        Arc::new(EnvVarsExporter::new()),
        Arc::new(FontsExporter::new()),
        Arc::new(HostsExporter::new()),
        Arc::new(ScheduledTasksExporter::new()),
        Arc::new(PowershellModulesExporter::new()),
        Arc::new(PrintersExporter::new()),
        Arc::new(DisksInfoExporter::new()),
        Arc::new(WindowsInfoExporter::new()),
        Arc::new(ChecklistExporter),
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
    issue(IssueSeverity::Warning, message_key, message_args)
}

/// An issue of an export run with the given severity.
pub(crate) fn issue(
    severity: IssueSeverity,
    message_key: &str,
    message_args: BTreeMap<String, String>,
) -> ScanIssue {
    ScanIssue {
        severity,
        source: ISSUE_SOURCE.to_owned(),
        path: None,
        message_key: message_key.to_owned(),
        message_args,
    }
}

/// Writes `content` to `target_dir/rel_path` and describes the file.
///
/// Only `target_dir` is written (principle P1); it and the folders of `rel_path` are
/// created if missing. A file that fails while being written is removed again
/// ([`write_new`]).
pub(crate) fn write_file(
    target_dir: &Path,
    rel_path: impl AsRef<Path>,
    content: &[u8],
) -> io::Result<ExportedFile> {
    let rel_path = rel_path.as_ref();
    let path = target_dir.join(rel_path);
    std::fs::create_dir_all(path.parent().unwrap_or(target_dir))?;
    write_new(&path, |file| file.write_all(content))?;
    Ok(ExportedFile {
        rel_path: rel_path.to_path_buf(),
        bytes: content.len() as u64,
        blake3: blake3::hash(content).to_hex().to_string(),
    })
}

/// Copies the file `src` to `target_dir/rel_path` and describes the copy; the content is
/// hashed while it is copied.
///
/// A file that is only in the cloud is not opened and nothing is written: `Ok(None)`
/// (SPEC-10 FR-10-15, [`source::open_local`]); the caller reports it
/// ([`partial::Partial::cloud_only`]). `src` is only opened for reading (principle P1);
/// only `target_dir` is written, it and the folders of `rel_path` are created if
/// missing. An existing file is replaced. When reading or writing fails midway, the
/// partial copy is removed ([`write_new`]), so every file left in `target_dir` is one
/// reported in `ExportResult.files` (FR-06-02).
pub(crate) fn copy_file(
    source: &dyn SourceFs,
    src: &Path,
    target_dir: &Path,
    rel_path: impl AsRef<Path>,
) -> io::Result<Option<ExportedFile>> {
    let rel_path = rel_path.as_ref();
    let Some(mut file) = source::open_local(source, src)? else {
        return Ok(None);
    };
    let path = target_dir.join(rel_path);
    std::fs::create_dir_all(path.parent().unwrap_or(target_dir))?;
    let (bytes, blake3) = write_new(&path, |dest| copy_hashed(&mut file, dest))?;
    Ok(Some(ExportedFile {
        rel_path: rel_path.to_path_buf(),
        bytes,
        blake3,
    }))
}

/// Creates the output file `path` (in the target folder) and fills it with `fill`. On
/// any error after the file was created, the file is removed again: an empty or partial
/// output must not stay behind unreported. Only this newly created output file is
/// removed, never a source.
pub(crate) fn write_new<T>(
    path: &Path,
    fill: impl FnOnce(&mut File) -> io::Result<T>,
) -> io::Result<T> {
    let mut file = File::create(path)?;
    let result = fill(&mut file);
    drop(file);
    if result.is_err() {
        if let Err(err) = std::fs::remove_file(path) {
            tracing::warn!(error = %err, "partial export file not removed");
        }
    }
    result
}

/// Copies `source` to `dest`; returns the number of bytes and their BLAKE3 hash.
pub(crate) fn copy_hashed(
    source: &mut impl Read,
    dest: &mut impl Write,
) -> io::Result<(u64, String)> {
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; COPY_BUFFER];
    let mut bytes = 0u64;
    loop {
        let n = match source.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        };
        hasher.update(&buf[..n]);
        dest.write_all(&buf[..n])?;
        bytes += n as u64;
    }
    dest.flush()?;
    Ok((bytes, hasher.finalize().to_hex().to_string()))
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
