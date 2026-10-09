//! Exports made of several independent parts (one `.reg` file per key, JSON files):
//! a failed part is a warning, the export fails only when no part succeeded.

use std::collections::BTreeMap;

use sk_core::model::{RegHive, ScanIssue};

use super::warning;
use crate::error::ExportError;
use crate::exporter::{ExportResult, ExportedFile, RestoreHint};
use crate::reg_export::full_key;

/// Warning: a registry key to export does not exist (SPEC-06 §4.5). Args: `key`.
pub(crate) const ISSUE_KEY_MISSING: &str = "issue.system.registry_key_missing";

/// Warning: `reg export` of a key failed. Args: `key`, `error`.
pub(crate) const ISSUE_EXPORT_FAILED: &str = "issue.system.registry_export_failed";

/// Warning: the values of a registry key cannot be read. Args: `key`, `error`.
pub(crate) const ISSUE_READ_FAILED: &str = "issue.system.registry_read_failed";

/// Warnings of the failed parts and the first error.
#[derive(Debug, Default)]
pub(crate) struct Partial {
    warnings: Vec<ScanIssue>,
    first_error: Option<ExportError>,
}

impl Partial {
    /// Records a failed part about `hive\key`: a missing key
    /// ([`ExportError::NotAvailable`]) gives [`ISSUE_KEY_MISSING`], any other error
    /// `failed_key` with the error text. Cancellation is returned to stop the export.
    pub(crate) fn add(
        &mut self,
        hive: RegHive,
        key: &str,
        failed_key: &str,
        err: ExportError,
    ) -> Result<(), ExportError> {
        let mut args = BTreeMap::from([("key".to_owned(), full_key(hive, key))]);
        let issue = match &err {
            ExportError::Cancelled => return Err(err),
            ExportError::NotAvailable => ISSUE_KEY_MISSING,
            _ => {
                args.insert("error".to_owned(), err.to_string());
                failed_key
            }
        };
        tracing::debug!(key = %args["key"], error = %err, "export part failed");
        self.warnings.push(warning(issue, args));
        self.first_error.get_or_insert(err);
        Ok(())
    }

    /// The result with `files` and the warnings; with no file at all, the first error.
    pub(crate) fn finish(
        self,
        files: Vec<ExportedFile>,
        restore_hint: RestoreHint,
    ) -> Result<ExportResult, ExportError> {
        if files.is_empty() {
            if let Some(err) = self.first_error {
                return Err(err);
            }
        }
        Ok(ExportResult {
            files,
            warnings: self.warnings,
            restore_hint,
        })
    }
}
