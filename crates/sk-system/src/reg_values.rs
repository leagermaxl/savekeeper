//! Reading all values of a registry key (`env-vars`, SPEC-06 §4.3).
//!
//! `sk_core::registry::RegistryReader` reads single values by name; the `env-vars`
//! exporter needs every value of `HKCU\Environment` with its raw type, so the values are
//! enumerated here with `winreg` (only `KEY_READ`, principle P1). [`ValueReader`] lets
//! tests replace the registry.

use std::io;

use sk_core::model::RegHive;

/// Data of one registry value, without expanding `REG_EXPAND_SZ`.
// Outside Windows only tests construct values: there is no registry to read.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RegData {
    /// `REG_SZ`.
    String(String),
    /// `REG_EXPAND_SZ`, as stored (`%USERPROFILE%\bin`).
    ExpandString(String),
    /// `REG_MULTI_SZ`.
    MultiString(Vec<String>),
    /// `REG_DWORD`.
    Dword(u32),
    /// `REG_QWORD`.
    Qword(u64),
    /// Any other type (binary and the like); only its type name is kept.
    Other(&'static str),
}

impl RegData {
    /// Name of the registry type: `REG_SZ`, `REG_EXPAND_SZ`, ...
    pub(crate) fn type_name(&self) -> &'static str {
        match self {
            Self::String(_) => "REG_SZ",
            Self::ExpandString(_) => "REG_EXPAND_SZ",
            Self::MultiString(_) => "REG_MULTI_SZ",
            Self::Dword(_) => "REG_DWORD",
            Self::Qword(_) => "REG_QWORD",
            Self::Other(name) => name,
        }
    }
}

/// A named registry value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RegValue {
    /// Value name (`""` is the default value).
    pub name: String,
    /// Value data.
    pub data: RegData,
}

/// Enumerates the values of a registry key.
pub(crate) trait ValueReader: Send + Sync {
    /// All values of `hive\key` in registry order; `Ok(None)` when the key does not exist.
    fn values(&self, hive: RegHive, key: &str) -> io::Result<Option<Vec<RegValue>>>;
}

/// The registry of this machine; outside Windows every key is missing.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SystemValueReader;

impl ValueReader for SystemValueReader {
    #[cfg(windows)]
    fn values(&self, hive: RegHive, key: &str) -> io::Result<Option<Vec<RegValue>>> {
        crate::win::key_values(hive, key)
    }

    #[cfg(not(windows))]
    fn values(&self, _hive: RegHive, _key: &str) -> io::Result<Option<Vec<RegValue>>> {
        Ok(None)
    }
}

#[cfg(test)]
pub(crate) use mem::MemValues;

#[cfg(test)]
mod mem {
    use std::collections::BTreeMap;

    use super::*;

    /// In-memory keys with values; a key mapped to an error message fails to read.
    #[derive(Default)]
    pub(crate) struct MemValues {
        keys: BTreeMap<(RegHive, String), Result<Vec<RegValue>, String>>,
    }

    impl MemValues {
        pub(crate) fn new() -> Self {
            Self::default()
        }

        /// Adds `hive\key` with `values` (name, data).
        pub(crate) fn key(mut self, hive: RegHive, key: &str, values: &[(&str, RegData)]) -> Self {
            let values = values
                .iter()
                .map(|(name, data)| RegValue {
                    name: (*name).to_owned(),
                    data: data.clone(),
                })
                .collect();
            self.keys.insert((hive, key.to_owned()), Ok(values));
            self
        }

        /// Adds `hive\key` that cannot be read.
        pub(crate) fn broken(mut self, hive: RegHive, key: &str, error: &str) -> Self {
            self.keys
                .insert((hive, key.to_owned()), Err(error.to_owned()));
            self
        }
    }

    impl ValueReader for MemValues {
        fn values(&self, hive: RegHive, key: &str) -> io::Result<Option<Vec<RegValue>>> {
            match self.keys.get(&(hive, key.to_owned())) {
                None => Ok(None),
                Some(Ok(values)) => Ok(Some(values.clone())),
                Some(Err(error)) => Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    error.clone(),
                )),
            }
        }
    }
}
