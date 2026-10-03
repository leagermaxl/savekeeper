//! Errors of system exports (SPEC-06 §4.1, §5).

/// Why a system export did not produce a result.
///
/// None of these stop the backup: the caller (SPEC-10) turns them into warnings.
#[derive(thiserror::Error, Debug)]
pub enum ExportError {
    /// The export cannot run here (e.g. the registry key does not exist, SPEC-06 §4.5).
    #[error("export is not available")]
    NotAvailable,
    /// The export needs administrator rights and the UAC prompt was declined or skipped.
    #[error("export needs administrator rights")]
    NeedsElevation,
    /// An external process exceeded its timeout and was killed (FR-06-04).
    #[error("external process timed out")]
    Timeout,
    /// The export was cancelled (principle P8).
    #[error("export was cancelled")]
    Cancelled,
    /// An external process exited with a non-zero code.
    #[error("external process failed with exit code {code}: {stderr}")]
    ProcessFailed {
        /// Exit code of the process.
        code: i32,
        /// Decoded standard error of the process (SPEC-06 §4.6).
        stderr: String,
    },
    /// Reading or writing files failed.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}
