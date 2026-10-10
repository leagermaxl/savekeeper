//! The `hosts` exporter (SPEC-06 §4.3): a copy of `{WINDIR}\System32\drivers\etc\hosts`
//! when it holds entries of its own (the stock file has only comments). A cloud-only
//! file is never opened (SPEC-10 FR-10-15).

#[cfg(test)]
mod tests;

use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use sk_core::env::Environment;
use sk_core::fs::CloudState;
use sk_core::model::{Category, Finding, Sensitivity};

use super::partial::Partial;
use super::source::{self, SourceFs};
use super::{copy_file, plan_finding, unavailable, Plan, NOT_WINDOWS};
use crate::error::ExportError;
use crate::exporter::{Availability, ExportContext, ExportResult, RestoreHint, SystemExporter};
use crate::reg_export::windir;

/// Exporter identifier.
pub(crate) const ID: &str = "hosts";

/// The exported file, in `system/hosts/` (SPEC-06 §4.3).
pub(crate) const FILE: &str = "hosts";

/// Finding title (i18n key).
const TITLE: &str = "system.title.hosts";

/// `reason_key`: the file has only comments and empty lines (the stock file).
const DEFAULT: &str = "system.hosts.default";

/// `reason_key`: the file is missing or cannot be read.
const MISSING: &str = "system.hosts.missing";

/// Restore instructions (i18n key): copy the file back (needs administrator rights).
const RESTORE_KEY: &str = "system.restore.hosts";

/// The `hosts` file of the machine.
pub(crate) struct HostsExporter {
    /// The `hosts` file; a cloud-only one is not opened (SPEC-10 FR-10-15).
    source: Arc<dyn SourceFs>,
}

impl HostsExporter {
    /// The exporter reading the real `hosts` file.
    pub(crate) fn new() -> Self {
        Self {
            source: source::real(),
        }
    }

    /// The exporter reading the file through `source` (tests).
    #[cfg(test)]
    pub(crate) fn with_source(source: Arc<dyn SourceFs>) -> Self {
        Self { source }
    }

    /// `detect` without the platform check: Available when the file has an entry, a
    /// non-empty line that does not start with `#` (SPEC-06 §4.3). Reading stops at the
    /// first entry, so large block lists do not slow the scan down.
    ///
    /// A cloud-only file is not opened: whether it has entries is unknown, so it is
    /// Available without an estimate, and `run` reports it.
    pub(crate) fn probe(&self, env: &Environment) -> Availability {
        let Some(path) = hosts_path(env) else {
            return unavailable(MISSING);
        };
        let file = match self.source.cloud_state(&path) {
            Ok(CloudState::CloudOnly) => {
                return Availability::Available {
                    estimated_bytes: None,
                    details: Default::default(),
                }
            }
            Ok(CloudState::Local) => self.source.open(&path),
            Err(err) => Err(err),
        };
        match file.and_then(has_entries) {
            Ok(true) => Availability::Available {
                estimated_bytes: std::fs::metadata(&path).ok().map(|m| m.len()),
                details: Default::default(),
            },
            Ok(false) => unavailable(DEFAULT),
            Err(err) => {
                tracing::debug!(error = %err, "hosts file not read");
                unavailable(MISSING)
            }
        }
    }
}

#[async_trait]
impl SystemExporter for HostsExporter {
    fn id(&self) -> &'static str {
        ID
    }

    /// See [`HostsExporter::probe`]. Reasons: `system.hosts.default`,
    /// `system.hosts.missing`, `system.not_windows`.
    fn detect(&self, env: &Environment) -> Availability {
        if cfg!(windows) {
            self.probe(env)
        } else {
            unavailable(NOT_WINDOWS)
        }
    }

    fn plan(&self, _env: &Environment, avail: &Availability) -> Option<Finding> {
        let plan = Plan {
            exporter_id: ID,
            title: TITLE,
            category: Category::SystemSettings,
            sensitivity: Sensitivity::None,
            params: serde_json::json!({}),
        };
        plan_finding(plan, avail)
    }

    /// Copies the `hosts` file as is to `hosts` in the target folder. A missing file is
    /// [`ExportError::NotAvailable`]; a cloud-only file is not opened, the result has no
    /// files and the warning `issue.system.cloud_only` {path} (SPEC-10 FR-10-15).
    async fn run(
        &self,
        ctx: &ExportContext<'_>,
        _params: &serde_json::Value,
    ) -> Result<ExportResult, ExportError> {
        if ctx.cancel.is_cancelled() {
            return Err(ExportError::Cancelled);
        }
        let Some(src) = hosts_path(ctx.env) else {
            return Err(ExportError::NotAvailable);
        };
        let mut files = Vec::new();
        let mut skipped = Partial::default();
        match copy_file(self.source.as_ref(), &src, ctx.target_dir, FILE) {
            Ok(Some(file)) => files.push(file),
            Ok(None) => skipped.cloud_only(&src),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Err(ExportError::NotAvailable)
            }
            Err(err) => return Err(err.into()),
        }
        skipped.finish(
            files,
            RestoreHint::Manual {
                key: RESTORE_KEY.to_owned(),
            },
        )
    }
}

/// `{WINDIR}\System32\drivers\etc\hosts`.
fn hosts_path(env: &Environment) -> Option<PathBuf> {
    windir(env).map(|dir| dir.join("System32").join("drivers").join("etc").join(FILE))
}

/// Whether the open `hosts` file has an entry: a line that is not empty and does not
/// start with `#` after leading whitespace (and a UTF-8 BOM) are removed.
fn has_entries(file: File) -> io::Result<bool> {
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let mut first = true;
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            return Ok(false);
        }
        let mut text = line.as_slice();
        if first {
            text = text.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(text);
            first = false;
        }
        if is_entry(text) {
            return Ok(true);
        }
    }
}

/// Whether one raw line of `hosts` is an entry (not empty, not a comment).
pub(crate) fn is_entry(line: &[u8]) -> bool {
    line.trim_ascii().first().is_some_and(|&b| b != b'#')
}
