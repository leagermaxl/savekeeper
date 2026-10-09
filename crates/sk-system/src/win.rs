//! Windows-only code of sk-system: enumerating registry values with `winreg`.
//!
//! Keys are opened with `KEY_READ` only (principle P1); nothing here writes to the
//! registry.

use std::io;

use sk_core::model::RegHive;
use sk_core::registry::normalize_key;
use winreg::enums::{
    RegType, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, REG_DWORD, REG_EXPAND_SZ,
    REG_MULTI_SZ, REG_QWORD, REG_SZ,
};
use winreg::types::FromRegValue;
use winreg::RegKey;

use crate::reg_values::{RegData, RegValue};

/// `ERROR_FILE_NOT_FOUND`: the key does not exist.
const ERROR_FILE_NOT_FOUND: i32 = 2;

/// All values of `hive\key` in registry order; `Ok(None)` when the key does not exist.
/// Values that `winreg` cannot represent are skipped.
pub(crate) fn key_values(hive: RegHive, key: &str) -> io::Result<Option<Vec<RegValue>>> {
    let root = RegKey::predef(match hive {
        RegHive::Hkcu => HKEY_CURRENT_USER,
        RegHive::Hklm => HKEY_LOCAL_MACHINE,
    });
    let opened = root.open_subkey_with_flags(normalize_key(key), KEY_READ);
    let handle = match opened {
        Ok(handle) => handle,
        Err(err) if err.raw_os_error() == Some(ERROR_FILE_NOT_FOUND) => return Ok(None),
        Err(err) => return Err(err),
    };
    let mut values = Vec::new();
    for entry in handle.enum_values() {
        // A value of an unknown type or with a non-Unicode name is skipped; the
        // enumeration goes on with the next one.
        let (name, value) = match entry {
            Ok(entry) => entry,
            Err(err) => {
                tracing::debug!(error = %err, "registry value skipped");
                continue;
            }
        };
        values.push(RegValue {
            name,
            data: data(&value),
        });
    }
    Ok(Some(values))
}

/// Raw value data by type; strings are not expanded.
fn data(value: &winreg::RegValue<'_>) -> RegData {
    let parsed = match value.vtype {
        REG_SZ => String::from_reg_value(value).map(RegData::String),
        REG_EXPAND_SZ => String::from_reg_value(value).map(RegData::ExpandString),
        REG_MULTI_SZ => Vec::<String>::from_reg_value(value).map(RegData::MultiString),
        REG_DWORD => u32::from_reg_value(value).map(RegData::Dword),
        REG_QWORD => u64::from_reg_value(value).map(RegData::Qword),
        _ => return RegData::Other(type_name(&value.vtype)),
    };
    parsed.unwrap_or_else(|_| RegData::Other(type_name(&value.vtype)))
}

/// Name of a registry type other than the parsed ones.
fn type_name(vtype: &RegType) -> &'static str {
    match vtype {
        RegType::REG_NONE => "REG_NONE",
        RegType::REG_SZ => "REG_SZ",
        RegType::REG_EXPAND_SZ => "REG_EXPAND_SZ",
        RegType::REG_BINARY => "REG_BINARY",
        RegType::REG_DWORD => "REG_DWORD",
        RegType::REG_DWORD_BIG_ENDIAN => "REG_DWORD_BIG_ENDIAN",
        RegType::REG_LINK => "REG_LINK",
        RegType::REG_MULTI_SZ => "REG_MULTI_SZ",
        RegType::REG_RESOURCE_LIST => "REG_RESOURCE_LIST",
        RegType::REG_FULL_RESOURCE_DESCRIPTOR => "REG_FULL_RESOURCE_DESCRIPTOR",
        RegType::REG_RESOURCE_REQUIREMENTS_LIST => "REG_RESOURCE_REQUIREMENTS_LIST",
        RegType::REG_QWORD => "REG_QWORD",
    }
}
