//! A temporary registry key for Windows integration tests (SPEC-12 §4.2, NFR-12-04).

use winreg::enums::{HKEY_CURRENT_USER, KEY_ALL_ACCESS};
use winreg::RegKey;

/// Parent of all test keys under `HKEY_CURRENT_USER`.
pub const REG_TEST_PARENT: &str = r"Software\SaveKeeperTest";

/// A fresh key `HKCU\Software\SaveKeeperTest\<uuid>`, deleted with all its
/// subkeys and values on drop. Panics if the key cannot be created.
#[derive(Debug)]
pub struct RegTestKey {
    key: RegKey,
    subkey: String,
}

impl RegTestKey {
    /// Creates a new empty key with a random name.
    pub fn new() -> Self {
        let subkey = format!(r"{REG_TEST_PARENT}\{}", uuid::Uuid::new_v4());
        let (key, _) = RegKey::predef(HKEY_CURRENT_USER)
            .create_subkey_with_flags(&subkey, KEY_ALL_ACCESS)
            .unwrap_or_else(|e| panic!(r"cannot create HKCU\{subkey}: {e}"));
        Self { key, subkey }
    }

    /// The open key, with full access: set values and create subkeys through it.
    pub fn key(&self) -> &RegKey {
        &self.key
    }

    /// Path relative to `HKEY_CURRENT_USER`: `Software\SaveKeeperTest\<uuid>`.
    pub fn subkey(&self) -> &str {
        &self.subkey
    }

    /// Full path as `reg.exe` takes it: `HKCU\Software\SaveKeeperTest\<uuid>`.
    pub fn path(&self) -> String {
        format!(r"HKCU\{}", self.subkey)
    }
}

impl Default for RegTestKey {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for RegTestKey {
    fn drop(&mut self) {
        // Errors cannot be reported from drop; the nightly job checks that
        // the parent key is empty (SPEC-12 §5).
        let _ = RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(&self.subkey);
    }
}
