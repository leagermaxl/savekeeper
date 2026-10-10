//! A temporary registry key for Windows integration tests (SPEC-12 §4.2, NFR-12-04).

use std::borrow::Cow;

use winreg::enums::{RegType, HKEY_CURRENT_USER, KEY_ALL_ACCESS};
use winreg::{RegKey, RegValue};

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

    /// Sets the value `name` of this key to the raw `bytes` with the registry
    /// type number `kind` (`REG_NONE` = 0 … `REG_QWORD` = 11), e.g. a
    /// `REG_EXPAND_SZ` or `REG_MULTI_SZ` as UTF-16LE. Panics if `kind` is not
    /// one of these or the value cannot be written.
    pub fn set_raw_value(&self, name: &str, kind: u32, bytes: &[u8]) {
        let vtype = match kind {
            0 => RegType::REG_NONE,
            1 => RegType::REG_SZ,
            2 => RegType::REG_EXPAND_SZ,
            3 => RegType::REG_BINARY,
            4 => RegType::REG_DWORD,
            5 => RegType::REG_DWORD_BIG_ENDIAN,
            6 => RegType::REG_LINK,
            7 => RegType::REG_MULTI_SZ,
            8 => RegType::REG_RESOURCE_LIST,
            9 => RegType::REG_FULL_RESOURCE_DESCRIPTOR,
            10 => RegType::REG_RESOURCE_REQUIREMENTS_LIST,
            11 => RegType::REG_QWORD,
            _ => panic!("unsupported registry type {kind}"),
        };
        let value = RegValue {
            bytes: Cow::Borrowed(bytes),
            vtype,
        };
        self.key
            .set_raw_value(name, &value)
            .unwrap_or_else(|e| panic!(r"cannot set {name} in HKCU\{}: {e}", self.subkey));
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
