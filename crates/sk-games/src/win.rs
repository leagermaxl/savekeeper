//! Windows-specific code of `sk-games` (SPEC-00 §7.1): registry values
//! through the safe `winreg` API, so no `unsafe` is needed here. Other OSes
//! get a stub without a registry.

use sk_core::model::RegHive;

/// A `REG_SZ`/`REG_EXPAND_SZ` value of `key` (normalized, `\`-separated),
/// opened for reading only.
#[cfg(windows)]
pub(crate) fn string_value(hive: RegHive, key: &str, name: &str) -> Option<String> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, REG_EXPAND_SZ, REG_SZ};
    use winreg::RegKey;

    let root = RegKey::predef(match hive {
        RegHive::Hkcu => HKEY_CURRENT_USER,
        RegHive::Hklm => HKEY_LOCAL_MACHINE,
    });
    let key = root.open_subkey_with_flags(key, KEY_READ).ok()?;
    let raw = key.get_raw_value(name).ok()?;
    if !matches!(raw.vtype, REG_SZ | REG_EXPAND_SZ) {
        return None;
    }
    // UTF-16LE up to the first NUL.
    let units: Vec<u16> = raw
        .bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&pair| u16::from_le_bytes(pair))
        .take_while(|&unit| unit != 0)
        .collect();
    Some(String::from_utf16_lossy(&units))
}

/// There is no registry outside Windows.
#[cfg(not(windows))]
pub(crate) fn string_value(_hive: RegHive, _key: &str, _name: &str) -> Option<String> {
    None
}
