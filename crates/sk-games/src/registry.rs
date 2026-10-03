//! Read-only registry values for launcher detection (SPEC-05 §4.4).
//!
//! [`RegistryReader`] abstracts the registry so detectors can be tested on
//! any OS: [`SystemRegistry`] reads the real registry (Windows only, through
//! [`crate::win`]), [`MemRegistry`] is an in-memory fake for tests. Keys are
//! only opened for reading (principle P1).

use std::collections::BTreeMap;

use sk_core::model::RegHive;

/// Read-only access to registry values: the real registry or a fake one.
pub trait RegistryReader: Send + Sync {
    /// The string value `name` of `key` (`\`-separated, relative to `hive`);
    /// `REG_SZ` or `REG_EXPAND_SZ` (not expanded). `None` if the key or value
    /// is missing, has another type or cannot be read, and on other OSes.
    fn string_value(&self, hive: RegHive, key: &str, name: &str) -> Option<String>;
}

/// The registry of this machine. On other OSes every value is missing.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemRegistry;

impl RegistryReader for SystemRegistry {
    fn string_value(&self, hive: RegHive, key: &str, name: &str) -> Option<String> {
        crate::win::string_value(hive, &normalize_key(key), name)
    }
}

/// An in-memory registry for tests. Key and value names compare without
/// case, as in the registry.
#[derive(Debug, Clone, Default)]
pub struct MemRegistry {
    values: BTreeMap<(RegHive, String, String), String>,
}

impl MemRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the string value `name` of `key`.
    pub fn set_string(&mut self, hive: RegHive, key: &str, name: &str, value: &str) -> &mut Self {
        self.values.insert(entry(hive, key, name), value.to_owned());
        self
    }
}

impl RegistryReader for MemRegistry {
    fn string_value(&self, hive: RegHive, key: &str, name: &str) -> Option<String> {
        self.values.get(&entry(hive, key, name)).cloned()
    }
}

fn entry(hive: RegHive, key: &str, name: &str) -> (RegHive, String, String) {
    (hive, normalize_key(key).to_lowercase(), name.to_lowercase())
}

/// `key` without leading, trailing or repeated `\` (and `/` read as `\`).
fn normalize_key(key: &str) -> String {
    key.split(['\\', '/'])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mem_registry_ignores_case_and_separators() {
        let mut reg = MemRegistry::new();
        reg.set_string(
            RegHive::Hkcu,
            r"Software\Valve\Steam",
            "SteamPath",
            "c:/steam",
        );
        assert_eq!(
            reg.string_value(RegHive::Hkcu, r"\software\VALVE\steam\", "steampath"),
            Some("c:/steam".to_owned())
        );
        assert_eq!(
            reg.string_value(RegHive::Hklm, r"Software\Valve\Steam", "SteamPath"),
            None
        );
        assert_eq!(
            reg.string_value(RegHive::Hkcu, r"Software\Valve\Steam", "SteamExe"),
            None
        );
    }

    #[test]
    fn normalize_key_trims_separators() {
        assert_eq!(normalize_key("\\Software\\\\Foo\\"), "Software\\Foo");
        assert_eq!(normalize_key("Software/Foo"), "Software\\Foo");
    }

    #[cfg(not(windows))]
    #[test]
    fn system_registry_is_empty_outside_windows() {
        assert_eq!(
            SystemRegistry.string_value(RegHive::Hkcu, r"Software\Valve\Steam", "SteamPath"),
            None
        );
    }

    /// Reads values of a temporary test key (SPEC-12 §4.2); the real Steam
    /// key is never read by tests.
    #[cfg(windows)]
    #[test]
    fn system_registry_reads_string_values() {
        let key = sk_testkit::RegTestKey::new();
        key.key()
            .set_value("SteamPath", &"c:/games/steam")
            .unwrap_or_else(|e| panic!("{e}"));
        key.key()
            .set_value("Number", &7u32)
            .unwrap_or_else(|e| panic!("{e}"));
        let reg = SystemRegistry;
        assert_eq!(
            reg.string_value(RegHive::Hkcu, key.subkey(), "SteamPath"),
            Some("c:/games/steam".to_owned())
        );
        assert_eq!(
            reg.string_value(RegHive::Hkcu, key.subkey(), "Number"),
            None
        );
        assert_eq!(
            reg.string_value(RegHive::Hkcu, key.subkey(), "Missing"),
            None
        );
        let missing = format!(r"{}\NoSuchKey", key.subkey());
        assert_eq!(reg.string_value(RegHive::Hkcu, &missing, "SteamPath"), None);
    }
}
