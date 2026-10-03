//! Errors of the games collector (SPEC-05 §4.1).

/// Error of loading the Ludusavi manifest or detecting games.
///
/// Network problems are not errors: [`ManifestStore`](crate::ManifestStore)
/// falls back to the cache or the embedded snapshot and reports them as
/// [`UpdateOutcome::Failed`](crate::UpdateOutcome::Failed) or scan issues.
#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum GamesError {
    /// The manifest is not valid YAML or does not match the model of SPEC-05 §4.2.
    #[error("invalid Ludusavi manifest: {0}")]
    ManifestParse(Box<serde_saphyr::Error>),
    /// Reading or writing the manifest cache, or unpacking the embedded snapshot, failed.
    #[error("manifest cache I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// The operation was cancelled by the caller's `CancellationToken`.
    #[error("cancelled")]
    Cancelled,
}

impl From<serde_saphyr::Error> for GamesError {
    fn from(e: serde_saphyr::Error) -> Self {
        Self::ManifestParse(Box::new(e))
    }
}
