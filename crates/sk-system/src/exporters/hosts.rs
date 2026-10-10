//! The `hosts` exporter (SPEC-06 §4.3): a copy of `{WINDIR}\System32\drivers\etc\hosts`
//! when it holds entries of its own (the stock file has only comments).

#[cfg(test)]
mod tests;

use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use sk_core::env::Environment;
use sk_core::model::{Category, Finding, Sensitivity};

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
pub(crate) struct HostsExporter;

impl HostsExporter {
    /// `detect` without the platform check: Available when the file has an entry, a
    /// non-empty line that does not start with `#` (SPEC-06 §4.3). Reading stops at the
    /// first entry, so large block lists do not slow the scan down.
    pub(crate) fn probe(env: &Environment) -> Availability {
        let Some(path) = hosts_path(env) else {
            return unavailable(MISSING);
        };
        match has_entries(&path) {
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
            Self::probe(env)
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
    /// [`ExportError::NotAvailable`].
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
        let file = match copy_file(&src, ctx.target_dir, FILE) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Err(ExportError::NotAvailable)
            }
            Err(err) => return Err(err.into()),
        };
        Ok(ExportResult {
            files: vec![file],
            warnings: Vec::new(),
            restore_hint: RestoreHint::Manual {
                key: RESTORE_KEY.to_owned(),
            },
        })
    }
}

/// `{WINDIR}\System32\drivers\etc\hosts`.
fn hosts_path(env: &Environment) -> Option<PathBuf> {
    windir(env).map(|dir| dir.join("System32").join("drivers").join("etc").join(FILE))
}

/// Whether the file at `path` has an entry: a line that is not empty and does not start
/// with `#` after leading whitespace (and a UTF-8 BOM) are removed.
fn has_entries(path: &Path) -> io::Result<bool> {
    let mut reader = BufReader::new(File::open(path)?);
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
