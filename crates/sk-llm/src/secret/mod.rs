//! API key storage (SPEC-08 §4.10).
//!
//! Values are [`SecretString`]s: `Debug` prints `[REDACTED]`, the memory is
//! zeroed on drop, and the value is exposed only where a request header is
//! built. Nothing here logs a value; logs name the secret and the store only.

use std::collections::HashMap;
use std::fmt;

use secrecy::{ExposeSecret, SecretString};
use sk_core::config::{ApiKeySource, CloudLlmConfig};

#[cfg(windows)]
mod win;

/// Stubs of the Credential Manager backend off Windows: nothing is stored.
#[cfg(not(windows))]
mod win {
    use secrecy::SecretString;

    use super::SecretError;

    pub(super) fn get(_target: &str, _name: &str) -> Result<Option<SecretString>, SecretError> {
        Ok(None)
    }

    pub(super) fn set(
        _target: &str,
        _name: &str,
        _value: &SecretString,
    ) -> Result<(), SecretError> {
        Err(SecretError::Unsupported)
    }

    pub(super) fn delete(_target: &str, _name: &str) -> Result<(), SecretError> {
        Err(SecretError::Unsupported)
    }
}

/// Name of the cloud API key in a [`SecretStore`] (credential target
/// `SaveKeeper/anthropic_api_key`).
pub const API_KEY_NAME: &str = "anthropic_api_key";

/// Prefix of the Credential Manager target name: `SaveKeeper/<name>`.
pub const CREDENTIAL_TARGET_PREFIX: &str = "SaveKeeper/";

/// Service name of the stored credentials (`keyring`; Windows identifies a
/// credential by its target name only).
pub const CREDENTIAL_SERVICE: &str = "SaveKeeper";

/// Longest accepted secret name.
const MAX_NAME_LEN: usize = 128;

/// Error of a [`SecretStore`] write. Messages never contain a secret value.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum SecretError {
    /// The store cannot be written (environment variables).
    #[error("secret store is read-only")]
    ReadOnly,
    /// The store is not available on this OS (Credential Manager off Windows).
    #[error("secret store is not supported on this platform")]
    Unsupported,
    /// The name is empty, too long or has characters other than ASCII
    /// letters, digits, `_`, `-` and `.`.
    #[error("invalid secret name")]
    InvalidName,
    /// The value is empty or only whitespace.
    #[error("empty secret value")]
    EmptyValue,
    /// The OS store failed.
    #[error("secret store error: {0}")]
    Store(String),
}

/// Storage of named secrets (the cloud API key, [`API_KEY_NAME`]).
pub trait SecretStore: Send + Sync {
    /// The secret `name`, trimmed; `None` if it is missing, blank or cannot be
    /// read (a read failure is logged as a warning without the value).
    fn get(&self, name: &str) -> Option<SecretString>;
    /// Creates or replaces the secret `name` with `value` (trimmed).
    fn set(&self, name: &str, value: SecretString) -> Result<(), SecretError>;
    /// Removes the secret `name`; a missing secret is not an error.
    fn delete(&self, name: &str) -> Result<(), SecretError>;
}

/// Windows Credential Manager of the **current user**: generic credentials
/// with target `SaveKeeper/<name>`. A portable copy does not carry the key to
/// another machine or user. Off Windows `get` is `None` and writes are
/// [`SecretError::Unsupported`].
#[derive(Debug, Clone, Default)]
pub struct CredentialManagerStore {
    _private: (),
}

impl CredentialManagerStore {
    /// The store of the current user.
    pub fn new() -> Self {
        Self::default()
    }

    /// The Credential Manager target name of `name`: `SaveKeeper/<name>`.
    pub fn target(name: &str) -> String {
        format!("{CREDENTIAL_TARGET_PREFIX}{name}")
    }
}

impl SecretStore for CredentialManagerStore {
    fn get(&self, name: &str) -> Option<SecretString> {
        check_name(name).ok()?;
        match win::get(&Self::target(name), name) {
            Ok(value) => non_blank(value?),
            Err(e) => {
                tracing::warn!(name, error = %e, "cannot read secret from credential manager");
                None
            }
        }
    }

    fn set(&self, name: &str, value: SecretString) -> Result<(), SecretError> {
        check_name(name)?;
        let value = non_blank(value).ok_or(SecretError::EmptyValue)?;
        win::set(&Self::target(name), name, &value)?;
        tracing::info!(name, "secret saved to credential manager");
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<(), SecretError> {
        check_name(name)?;
        win::delete(&Self::target(name), name)?;
        tracing::info!(name, "secret removed from credential manager");
        Ok(())
    }
}

/// Secrets in environment variables (`llm.cloud.api_key_env`), read-only.
///
/// Each secret name is mapped to a variable with [`Self::with_var`]; an
/// unmapped name is `None`. Variables are read on every `get`.
#[derive(Debug, Clone, Default)]
pub struct EnvStore {
    /// Secret name → variable name.
    vars: HashMap<String, String>,
    /// Values used instead of the process environment (tests).
    values: Option<HashMap<String, SecretString>>,
}

impl EnvStore {
    /// A store without mapped names, reading the process environment.
    pub fn new() -> Self {
        Self::default()
    }

    /// The store of the cloud key: [`API_KEY_NAME`] → `cfg.api_key_env`.
    pub fn for_cloud(cfg: &CloudLlmConfig) -> Self {
        Self::new().with_var(API_KEY_NAME, cfg.api_key_env.trim())
    }

    /// Reads the secret `name` from the variable `var`.
    #[must_use]
    pub fn with_var(mut self, name: impl Into<String>, var: impl Into<String>) -> Self {
        self.vars.insert(name.into(), var.into());
        self
    }

    /// Reads variables from `values` instead of the process environment
    /// (tests of this and other crates; the environment is never changed).
    #[must_use]
    pub fn with_values<I, K, V>(mut self, values: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let map = values
            .into_iter()
            .map(|(k, v)| (k.into(), SecretString::from(v.into())))
            .collect();
        self.values = Some(map);
        self
    }
}

impl SecretStore for EnvStore {
    fn get(&self, name: &str) -> Option<SecretString> {
        let var = self.vars.get(name)?;
        if var.is_empty() {
            return None;
        }
        let value = match &self.values {
            Some(values) => values.get(var).cloned(),
            None => match std::env::var(var) {
                Ok(value) => Some(SecretString::from(value)),
                Err(std::env::VarError::NotPresent) => None,
                Err(std::env::VarError::NotUnicode(_)) => {
                    tracing::warn!(name, var = %var, "secret variable is not valid Unicode");
                    None
                }
            },
        };
        non_blank(value?)
    }

    fn set(&self, _name: &str, _value: SecretString) -> Result<(), SecretError> {
        Err(SecretError::ReadOnly)
    }

    fn delete(&self, _name: &str) -> Result<(), SecretError> {
        Err(SecretError::ReadOnly)
    }
}

/// Several stores in priority order (§4.10: `api_key_source` first, the
/// other source as fallback).
///
/// `get` returns the first store's value; `set` and `delete` go to the first
/// store that is writable (skipping [`SecretError::ReadOnly`] and
/// [`SecretError::Unsupported`]); if none is, the last such error.
pub struct FallbackStore {
    stores: Vec<(&'static str, Box<dyn SecretStore>)>,
}

impl FallbackStore {
    /// An empty chain; `get` is `None`, writes are [`SecretError::ReadOnly`].
    pub fn new() -> Self {
        Self { stores: Vec::new() }
    }

    /// Appends `store`; `label` names it in logs (`"credential_manager"`, `"env"`).
    #[must_use]
    pub fn with(mut self, label: &'static str, store: impl SecretStore + 'static) -> Self {
        self.stores.push((label, Box::new(store)));
        self
    }

    /// The cloud key chain of `cfg`: [`CredentialManagerStore`] and
    /// [`EnvStore::for_cloud`], ordered by `api_key_source`.
    pub fn for_cloud(cfg: &CloudLlmConfig) -> Self {
        Self::from_stores(
            cfg.api_key_source,
            CredentialManagerStore::new(),
            EnvStore::for_cloud(cfg),
        )
    }

    /// The chain of `credential_manager` and `env`, ordered by `source`.
    pub fn from_stores(
        source: ApiKeySource,
        credential_manager: impl SecretStore + 'static,
        env: impl SecretStore + 'static,
    ) -> Self {
        let chain = Self::new();
        match source {
            ApiKeySource::CredentialManager => chain
                .with("credential_manager", credential_manager)
                .with("env", env),
            ApiKeySource::Env => chain
                .with("env", env)
                .with("credential_manager", credential_manager),
        }
    }

    /// Labels of the stores in priority order.
    pub fn labels(&self) -> Vec<&'static str> {
        self.stores.iter().map(|(label, _)| *label).collect()
    }

    /// Runs `op` on the first writable store.
    fn write(
        &self,
        mut op: impl FnMut(&dyn SecretStore) -> Result<(), SecretError>,
    ) -> Result<(), SecretError> {
        let mut last = SecretError::ReadOnly;
        for (_, store) in &self.stores {
            match op(store.as_ref()) {
                Err(e @ (SecretError::ReadOnly | SecretError::Unsupported)) => last = e,
                other => return other,
            }
        }
        Err(last)
    }
}

impl Default for FallbackStore {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for FallbackStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FallbackStore")
            .field("stores", &self.labels())
            .finish()
    }
}

impl SecretStore for FallbackStore {
    fn get(&self, name: &str) -> Option<SecretString> {
        self.stores.iter().find_map(|(label, store)| {
            let value = store.get(name)?;
            tracing::debug!(name, source = *label, "secret found");
            Some(value)
        })
    }

    fn set(&self, name: &str, value: SecretString) -> Result<(), SecretError> {
        self.write(|store| store.set(name, value.clone()))
    }

    fn delete(&self, name: &str) -> Result<(), SecretError> {
        self.write(|store| store.delete(name))
    }
}

/// Checks a secret name (see [`SecretError::InvalidName`]).
fn check_name(name: &str) -> Result<(), SecretError> {
    let valid = !name.is_empty()
        && name.len() <= MAX_NAME_LEN
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'));
    if valid {
        Ok(())
    } else {
        Err(SecretError::InvalidName)
    }
}

/// `value` trimmed, or `None` if it is blank.
fn non_blank(value: SecretString) -> Option<SecretString> {
    let trimmed = value.expose_secret().trim();
    if trimmed.is_empty() {
        None
    } else if trimmed.len() == value.expose_secret().len() {
        Some(value)
    } else {
        Some(SecretString::from(trimmed))
    }
}

#[cfg(test)]
mod tests;
