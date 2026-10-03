//! Errors of the games collector (SPEC-05 §4.1).

/// Error of loading the Ludusavi manifest or detecting games.
#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum GamesError {
    /// The manifest is not valid YAML or does not match the model of SPEC-05 §4.2.
    #[error("invalid Ludusavi manifest: {0}")]
    ManifestParse(Box<serde_saphyr::Error>),
}

impl From<serde_saphyr::Error> for GamesError {
    fn from(e: serde_saphyr::Error) -> Self {
        Self::ManifestParse(Box::new(e))
    }
}
