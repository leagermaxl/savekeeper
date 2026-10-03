//! Path templates with tokens such as `{APPDATA}\Code\User` (SPEC-02 §3).
//!
//! A template is stable across machines and users: it is what `FindingId`,
//! rules and the backup manifest store. `resolve` turns it into absolute paths
//! for the current [`Environment`], `from_path` goes the other way.

mod resolve;
mod specialize;
mod syntax;

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize};
use specta::Type;

pub use resolve::ResolveContext;
pub use syntax::{TemplateError, Token};

use crate::env::Environment;
use syntax::Piece;

/// A path with tokens, e.g. `{APPDATA}\Code\User`, in canonical form
/// (separator `\`, no repeated or trailing separators).
///
/// Serialized as a plain string; deserialization validates it with [`parse`](Self::parse).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Type)]
#[serde(transparent)]
pub struct PathTemplate(String);

impl PathTemplate {
    /// Validates and normalizes a template (SPEC-02 §3.1).
    pub fn parse(s: &str) -> Result<Self, TemplateError> {
        syntax::parse(s).map(|parsed| Self(parsed.render()))
    }

    /// Tokens in order of appearance.
    pub fn tokens(&self) -> impl Iterator<Item = Token> {
        let mut tokens = Vec::new();
        if let Some(parsed) = self.parsed() {
            tokens.extend(parsed.root);
            for segment in parsed.segments {
                for piece in segment {
                    if let Piece::Value(token) = piece {
                        tokens.push(token);
                    }
                }
            }
        }
        tokens.into_iter()
    }

    /// Absolute paths on this machine. Multi-valued tokens (`{STEAM_USERID}`,
    /// `{DRIVE:*}`, `{PACKAGE:…}`) give several paths; a token without a value
    /// gives none.
    pub fn resolve(&self, env: &Environment, ctx: &ResolveContext) -> Vec<PathBuf> {
        self.parsed()
            .map(|parsed| resolve::resolve(&parsed, env, ctx))
            .unwrap_or_default()
    }

    /// Specializes the multi-valued tokens for a finding template
    /// (SPEC-02 §3.2): `{DRIVE:*}` becomes `{DRIVE:X}` for every fixed drive
    /// of `env`, `{STEAM_USERID}` (every occurrence) every id of
    /// `ctx.steam_user_ids`; drives first, in the order of `env` and `ctx`.
    ///
    /// A token without values gives an empty list; a template without these
    /// tokens is returned as is. `{PACKAGE:…}`, context values and `*`
    /// segments are left alone.
    pub fn specialize(&self, env: &Environment, ctx: &ResolveContext) -> Vec<PathTemplate> {
        specialize::specialize(self, env, ctx)
    }

    /// The most specific template for an absolute path (SPEC-02 §3.2).
    pub fn from_path(path: &Path, env: &Environment) -> PathTemplate {
        Self(resolve::from_path(path, env, &|_| true).render())
    }

    /// [`from_path`](Self::from_path) limited to the root tokens accepted by
    /// `allow` (SPEC-02 §6: `EnvironmentSnapshot`).
    pub(crate) fn from_path_with(
        path: &Path,
        env: &Environment,
        allow: &dyn Fn(&Token) -> bool,
    ) -> PathTemplate {
        Self(resolve::from_path(path, env, allow).render())
    }

    /// The template string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The stored string is canonical, so parsing only fails for a template
    /// built by `from_path` from a folder literally named like a token
    /// (`{TEMP}`); such a template resolves to nothing.
    fn parsed(&self) -> Option<syntax::Parsed> {
        syntax::parse(&self.0).ok()
    }
}

impl fmt::Display for PathTemplate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::str::FromStr for PathTemplate {
    type Err = TemplateError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl<'de> Deserialize<'de> for PathTemplate {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod specialize_tests;
#[cfg(test)]
mod tests;
