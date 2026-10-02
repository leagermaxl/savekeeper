//! Shared errors (SPEC-01 §4.7).
//!
//! Non-fatal problems become `ScanIssue`s (SPEC-02 §2.6) and never an `Err`.

/// A collector cannot work at all.
#[derive(Debug, thiserror::Error)]
pub enum CollectorError {
    /// File system error.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Anything else.
    #[error("{0}")]
    Other(String),
}
