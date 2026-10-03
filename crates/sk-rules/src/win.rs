//! Windows-specific code of `sk-rules` (SPEC-00 §7.1): registry probing
//! through the safe `winreg` API, so no `unsafe` is needed here. Other OSes get
//! a stub without a registry.

use sk_core::model::RegHive;

use crate::registry::KeyState;

/// Opens `key` (normalized, `\`-separated) for reading and closes it again.
#[cfg(windows)]
pub(crate) fn key_state(hive: RegHive, key: &str) -> KeyState {
    use std::io::ErrorKind;

    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ};
    use winreg::RegKey;

    let root = RegKey::predef(match hive {
        RegHive::Hkcu => HKEY_CURRENT_USER,
        RegHive::Hklm => HKEY_LOCAL_MACHINE,
    });
    match root.open_subkey_with_flags(key, KEY_READ) {
        Ok(_) => KeyState::Present,
        Err(err) if err.kind() == ErrorKind::PermissionDenied => KeyState::AccessDenied,
        Err(_) => KeyState::Missing,
    }
}

/// There is no registry outside Windows.
#[cfg(not(windows))]
pub(crate) fn key_state(_hive: RegHive, _key: &str) -> KeyState {
    KeyState::Missing
}
