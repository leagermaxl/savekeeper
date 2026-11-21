//! Errors of the backup API (SPEC-10 §4.1).

use std::path::PathBuf;

/// Failure of a backup operation.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BackupError {
    /// The target folder exists, but its volume cannot be queried (`estimate`, §4.6).
    #[error("cannot inspect the backup target {path}: {source}")]
    Target {
        /// The target folder.
        path: PathBuf,
        /// The OS error.
        #[source]
        source: std::io::Error,
    },
}
