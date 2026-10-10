//! Exports made of several independent parts (one `.reg` file per key, JSON files,
//! copied files): a failed part is a warning, the export fails only when no part
//! succeeded.

use std::collections::BTreeMap;
use std::path::Path;

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

/// Warning: a file could not be copied into the backup. Args: `path`, `error`.
pub(crate) const ISSUE_FILE_COPY_FAILED: &str = "issue.system.file_copy_failed";

/// Warning: a folder could not be listed. Args: `path`, `error`.
pub(crate) const ISSUE_FOLDER_READ_FAILED: &str = "issue.system.folder_read_failed";

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
        self.add_issue(issue, args, err)
    }

    /// Records a failed part as the warning `message_key` with `message_args` (the
    /// caller adds the error text if the key needs it). Cancellation is returned to stop
    /// the export.
    pub(crate) fn add_issue(
        &mut self,
        message_key: &str,
        message_args: BTreeMap<String, String>,
        err: ExportError,
    ) -> Result<(), ExportError> {
        if matches!(err, ExportError::Cancelled) {
            return Err(err);
        }
        tracing::debug!(issue = message_key, error = %err, "export part failed");
        self.warnings.push(warning(message_key, message_args));
        self.first_error.get_or_insert(err);
        Ok(())
    }

    /// Records a failed part about the file or folder `path` as the warning
    /// `message_key` with the arguments `path` and `error`. Cancellation is returned to
    /// stop the export.
    pub(crate) fn add_path(
        &mut self,
        message_key: &str,
        path: &Path,
        err: ExportError,
    ) -> Result<(), ExportError> {
        let args = BTreeMap::from([
            ("path".to_owned(), path.to_string_lossy().into_owned()),
            ("error".to_owned(), err.to_string()),
        ]);
        self.add_issue(message_key, args, err)
    }

    /// Records a non-fatal issue that is not a failed part (an `Info` note, a skipped
    /// item): it never becomes the error of the export.
    pub(crate) fn note(&mut self, issue: ScanIssue) {
        self.warnings.push(issue);
    }

    /// The result with `files` and the warnings; with no file at all, the first error,
    /// or [`ExportError::NotAvailable`] when nothing failed (nothing to export).
    pub(crate) fn finish(
        self,
        files: Vec<ExportedFile>,
        restore_hint: RestoreHint,
    ) -> Result<ExportResult, ExportError> {
        if files.is_empty() {
            return Err(self.first_error.unwrap_or(ExportError::NotAvailable));
        }
        Ok(ExportResult {
            files,
            warnings: self.warnings,
            restore_hint,
        })
    }
}
