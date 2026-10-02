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

/// The scan pipeline failed as a whole (SPEC-01 §4.7).
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// The scan was cancelled; no report is saved.
    #[error("cancelled")]
    Cancelled,
    /// A collector could not work at all.
    #[error(transparent)]
    Collector(#[from] CollectorError),
    /// The environment could not be read.
    #[error(transparent)]
    Environment(#[from] crate::env::EnvError),
    /// The report could not be built.
    #[error("report: {0}")]
    Report(String),
}
