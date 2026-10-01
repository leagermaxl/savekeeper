//! Path templates with tokens such as `{APPDATA}\Code\User` (SPEC-02 §3).
//!
//! T-02-01 defines only the type used by the domain model. Parsing and token
//! validation, `resolve` and `from_path` are added in T-02-03.

use serde::{Deserialize, Serialize};
use specta::Type;

/// A path with tokens, e.g. `{APPDATA}\Code\User`. The separator is always `\`.
///
/// Serialized as a plain string.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Type)]
#[serde(transparent)]
pub struct PathTemplate(String);

impl PathTemplate {
    /// The template string, e.g. `{APPDATA}\Code\User`.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
