//! Registry key probing for the `registry_exists` condition (SPEC-04 §4.3).
//!
//! [`RegistryProbe`] abstracts the registry so conditions can be tested on
//! any OS: [`SystemRegistry`] reads the real registry (Windows only, through
//! [`crate::win`]), [`MemRegistry`] is an in-memory fake for tests. Probing
//! only opens keys for reading (principle P1).

use std::collections::BTreeMap;

use sk_core::model::RegHive;

/// Whether a registry key can be seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyState {
    /// The key exists and can be opened for reading.
    Present,
    /// The key does not exist (or the registry is unavailable on this OS).
    Missing,
    /// The key exists but cannot be opened for reading.
    AccessDenied,
}

/// Read-only access to registry keys: the real registry or a fake one.
pub trait RegistryProbe: Send + Sync {
    /// State of `key` (`\`-separated, relative to `hive`).
    fn key_state(&self, hive: RegHive, key: &str) -> KeyState;
}

/// The registry of this machine. On other OSes every key is
/// [`KeyState::Missing`].
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemRegistry;

impl RegistryProbe for SystemRegistry {
    fn key_state(&self, hive: RegHive, key: &str) -> KeyState {
        crate::win::key_state(hive, &normalize_key(key))
    }
}

/// An in-memory registry for tests.
///
/// Key names compare without case, as in the registry; adding a key makes its
/// parent keys exist too.
#[derive(Debug, Clone, Default)]
pub struct MemRegistry {
    keys: BTreeMap<(RegHive, String), KeyState>,
}

impl MemRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a readable key and its parents.
    pub fn add_key(&mut self, hive: RegHive, key: &str) -> &mut Self {
        self.insert(hive, key, KeyState::Present)
    }

    /// Adds a key that cannot be opened for reading; its parents are readable.
    pub fn add_denied_key(&mut self, hive: RegHive, key: &str) -> &mut Self {
        self.insert(hive, key, KeyState::AccessDenied)
    }

    fn insert(&mut self, hive: RegHive, key: &str, state: KeyState) -> &mut Self {
        let key = normalize_key(key).to_lowercase();
        let mut parent = String::new();
        for part in key.split('\\').filter(|p| !p.is_empty()) {
            if !parent.is_empty() {
                parent.push('\\');
            }
            parent.push_str(part);
            self.keys
                .entry((hive, parent.clone()))
                .or_insert(KeyState::Present);
        }
        self.keys.insert((hive, key), state);
        self
    }
}

impl RegistryProbe for MemRegistry {
    fn key_state(&self, hive: RegHive, key: &str) -> KeyState {
        let key = normalize_key(key).to_lowercase();
        if key.is_empty() {
            // The hive root always exists.
            return KeyState::Present;
        }
        self.keys
            .get(&(hive, key))
            .copied()
            .unwrap_or(KeyState::Missing)
    }
}

/// `key` without leading, trailing or repeated `\` (and `/` read as `\`).
pub(crate) fn normalize_key(key: &str) -> String {
    key.split(['\\', '/'])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mem_registry_parents_and_case() {
        let mut reg = MemRegistry::new();
        reg.add_key(RegHive::Hkcu, "Software\\SimonTatham\\PuTTY")
            .add_denied_key(RegHive::Hklm, "SOFTWARE\\Secret");
        assert_eq!(
            reg.key_state(RegHive::Hkcu, "software\\simontatham\\putty"),
            KeyState::Present
        );
        assert_eq!(
            reg.key_state(RegHive::Hkcu, "\\Software\\SimonTatham\\"),
            KeyState::Present
        );
        assert_eq!(reg.key_state(RegHive::Hkcu, ""), KeyState::Present);
        assert_eq!(
            reg.key_state(RegHive::Hklm, "Software\\SimonTatham"),
            KeyState::Missing
        );
        assert_eq!(
            reg.key_state(RegHive::Hklm, "software\\secret"),
            KeyState::AccessDenied
        );
        assert_eq!(reg.key_state(RegHive::Hklm, "Software"), KeyState::Present);
    }

    #[test]
    fn normalize_key_trims_separators() {
        assert_eq!(normalize_key("\\Software\\\\Foo\\"), "Software\\Foo");
        assert_eq!(normalize_key("Software/Foo"), "Software\\Foo");
        assert_eq!(normalize_key(""), "");
    }
}
