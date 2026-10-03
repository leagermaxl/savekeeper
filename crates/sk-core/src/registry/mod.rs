//! Read-only registry access for feature crates (SPEC-02 §3.4).
//!
//! [`RegistryReader`] abstracts the registry so that rules (SPEC-04),
//! launcher detectors (SPEC-05) and system enrichment (SPEC-06) can be tested
//! on any OS: [`SystemRegistry`] reads the registry of this machine (Windows
//! only, through `sk-core::win::registry`), [`MemRegistry`] is an in-memory
//! fake for tests. Keys are only opened for reading (`KEY_READ`, principle
//! P1); there is no registry write in `sk-core`.
//!
//! Keys are written with `\` or `/`; repeated, leading and trailing
//! separators are ignored ([`normalize_key`]). The empty key is the root of
//! the hive.

mod mem;

pub use mem::MemRegistry;

use crate::model::RegHive;

/// Whether a registry key can be seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyState {
    /// The key exists and can be opened for reading.
    Present,
    /// The key does not exist (or there is no registry on this OS).
    Missing,
    /// The key exists but cannot be opened for reading.
    AccessDenied,
}

/// Read-only access to the registry: the real one or a fake one.
///
/// `key` is relative to `hive`; see the [module docs](self) for its syntax.
pub trait RegistryReader: Send + Sync {
    /// State of `key`. The hive root (empty key) is present, except for
    /// `SystemRegistry` outside Windows, where everything is missing.
    fn key_state(&self, hive: RegHive, key: &str) -> KeyState;

    /// The string value `name` of `key` (`""` is the default value):
    /// `REG_SZ`, or `REG_EXPAND_SZ` with environment variables expanded, up
    /// to the first NUL. `None` if the key or value is missing, cannot be
    /// read or has another type.
    fn string_value(&self, hive: RegHive, key: &str, name: &str) -> Option<String>;

    /// The `REG_DWORD` value `name` of `key`; `None` if it is missing,
    /// cannot be read or has another type.
    fn dword_value(&self, hive: RegHive, key: &str, name: &str) -> Option<u32>;

    /// Names of the direct subkeys of `key`; empty if the key is missing or
    /// cannot be read. The order depends on the implementation: callers that
    /// need a stable one sort the names themselves.
    fn subkeys(&self, hive: RegHive, key: &str) -> Vec<String>;
}

/// The registry of this machine, read through `sk-core::win::registry`.
///
/// Outside Windows there is no registry: every key is [`KeyState::Missing`],
/// every value `None`, every subkey list empty. `subkeys` keeps the order of
/// `RegEnumKeyExW`.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemRegistry;

#[cfg(windows)]
impl RegistryReader for SystemRegistry {
    fn key_state(&self, hive: RegHive, key: &str) -> KeyState {
        let key = normalize_key(key);
        if key.is_empty() {
            return KeyState::Present;
        }
        crate::win::registry::key_state(crate::win::registry::hive_root(hive), &key)
    }

    fn string_value(&self, hive: RegHive, key: &str, name: &str) -> Option<String> {
        crate::win::registry::string(
            crate::win::registry::hive_root(hive),
            &normalize_key(key),
            name,
        )
    }

    fn dword_value(&self, hive: RegHive, key: &str, name: &str) -> Option<u32> {
        crate::win::registry::dword(
            crate::win::registry::hive_root(hive),
            &normalize_key(key),
            name,
        )
    }

    fn subkeys(&self, hive: RegHive, key: &str) -> Vec<String> {
        crate::win::registry::subkeys(crate::win::registry::hive_root(hive), &normalize_key(key))
    }
}

/// There is no registry outside Windows.
#[cfg(not(windows))]
impl RegistryReader for SystemRegistry {
    fn key_state(&self, _hive: RegHive, _key: &str) -> KeyState {
        KeyState::Missing
    }

    fn string_value(&self, _hive: RegHive, _key: &str, _name: &str) -> Option<String> {
        None
    }

    fn dword_value(&self, _hive: RegHive, _key: &str, _name: &str) -> Option<u32> {
        None
    }

    fn subkeys(&self, _hive: RegHive, _key: &str) -> Vec<String> {
        Vec::new()
    }
}

/// `key` with `/` read as `\` and without leading, trailing or repeated
/// separators: `\Software\\Foo\` and `Software/Foo` both give `Software\Foo`.
/// The case is kept.
pub fn normalize_key(key: &str) -> String {
    key.split(['\\', '/'])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\\")
}

#[cfg(test)]
mod tests;
