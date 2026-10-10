//! Read-only registry access (SPEC-02 §3.4). Keys are only opened with
//! `KEY_READ`; nothing here writes to the registry (principle P1).

use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegEnumValueW, RegGetValueW, RegOpenKeyExW, RegQueryInfoKeyW, HKEY,
    KEY_READ, REG_DWORD, REG_VALUE_TYPE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
};

use super::{from_wide, wide};
use crate::model::RegHive;
use crate::registry::{KeyState, RegValue};

pub(crate) use windows::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

/// Attempts to read a string whose expanded size grows between two calls.
const STRING_ATTEMPTS: usize = 4;

/// Attempts to read one enumerated value that grows between two calls.
const VALUE_ATTEMPTS: usize = 4;

/// Value names are limited to 16 383 characters (plus the NUL).
const MAX_VALUE_NAME: usize = 16_384;

/// The predefined key of a hive.
pub(crate) fn hive_root(hive: RegHive) -> HKEY {
    match hive {
        RegHive::Hkcu => HKEY_CURRENT_USER,
        RegHive::Hklm => HKEY_LOCAL_MACHINE,
    }
}

/// Opens `key` for reading and closes it again: `Present` on success,
/// `AccessDenied` on `ERROR_ACCESS_DENIED`, `Missing` on any other error.
pub(crate) fn key_state(root: HKEY, key: &str) -> KeyState {
    let key = wide(key);
    let mut handle = HKEY::default();
    // SAFETY: `key` is NUL-terminated and outlives the call; `handle`
    // receives the opened key.
    let status = unsafe { RegOpenKeyExW(root, PCWSTR(key.as_ptr()), None, KEY_READ, &mut handle) };
    if status == ERROR_SUCCESS {
        // SAFETY: `handle` was opened above and is not used afterwards.
        let _ = unsafe { RegCloseKey(handle) };
        KeyState::Present
    } else if status == ERROR_ACCESS_DENIED {
        KeyState::AccessDenied
    } else {
        KeyState::Missing
    }
}

/// A `REG_SZ` value, or a `REG_EXPAND_SZ` one with environment variables
/// expanded, up to the first NUL; `None` if the key or value is missing,
/// cannot be read or has another type.
pub(crate) fn string(root: HKEY, key: &str, value: &str) -> Option<String> {
    let (key, value) = (wide(key), wide(value));
    let mut size = 0u32;
    // SAFETY: key and value are NUL-terminated; a null data pointer asks for the size only.
    let status = unsafe {
        RegGetValueW(
            root,
            PCWSTR(key.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS || size == 0 {
        return None;
    }
    // The value can change (and an expanded string grow) between the calls.
    for _ in 0..STRING_ATTEMPTS {
        let mut buf = vec![0u16; (size as usize).div_ceil(2)];
        let mut len = u32::try_from(buf.len() * 2).ok()?;
        // SAFETY: `buf` holds `len` bytes, as passed in `pcbdata`.
        let status = unsafe {
            RegGetValueW(
                root,
                PCWSTR(key.as_ptr()),
                PCWSTR(value.as_ptr()),
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr().cast()),
                Some(&mut len),
            )
        };
        if status == ERROR_SUCCESS {
            return Some(from_wide(&buf).to_string_lossy().into_owned());
        }
        if status != ERROR_MORE_DATA || len <= size {
            return None;
        }
        size = len;
    }
    None
}

/// A `REG_DWORD` value; `None` for any other type (also a 4-byte
/// `REG_BINARY`, which `RRF_RT_REG_DWORD` lets through).
pub(crate) fn dword(root: HKEY, key: &str, value: &str) -> Option<u32> {
    let (key, value) = (wide(key), wide(value));
    let mut kind = REG_VALUE_TYPE::default();
    let mut data = 0u32;
    let mut size = 4u32;
    // SAFETY: `data` is a 4-byte buffer, as passed in `pcbdata`; `kind`
    // receives the value type.
    let status = unsafe {
        RegGetValueW(
            root,
            PCWSTR(key.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_DWORD,
            Some(&mut kind),
            Some((&raw mut data).cast()),
            Some(&mut size),
        )
    };
    (status == ERROR_SUCCESS && kind == REG_DWORD).then_some(data)
}

/// Names of the direct subkeys of a key; empty if the key is missing.
pub(crate) fn subkeys(root: HKEY, key: &str) -> Vec<String> {
    let key = wide(key);
    let mut handle = HKEY::default();
    // SAFETY: `key` is NUL-terminated; `handle` receives the opened key.
    let status = unsafe { RegOpenKeyExW(root, PCWSTR(key.as_ptr()), None, KEY_READ, &mut handle) };
    if status != ERROR_SUCCESS {
        return Vec::new();
    }
    let mut names = Vec::new();
    for index in 0.. {
        // Key names are limited to 255 characters.
        let mut buf = [0u16; 256];
        let mut len = buf.len() as u32;
        // SAFETY: `handle` is open; `buf` holds `len` characters.
        let status = unsafe {
            RegEnumKeyExW(
                handle,
                index,
                Some(windows::core::PWSTR(buf.as_mut_ptr())),
                &mut len,
                None,
                None,
                None,
                None,
            )
        };
        if status != ERROR_SUCCESS {
            break;
        }
        names.push(
            from_wide(&buf[..len as usize])
                .to_string_lossy()
                .into_owned(),
        );
    }
    // SAFETY: `handle` was opened above and is not used afterwards.
    let _ = unsafe { RegCloseKey(handle) };
    names
}

/// All values of a key in `RegEnumValueW` order with their raw data
/// (`REG_EXPAND_SZ` not expanded, decoded by `registry::value::decode`);
/// `None` if the key is missing or cannot be opened for reading. A value
/// that keeps growing between attempts is skipped.
pub(crate) fn values(root: HKEY, key: &str) -> Option<Vec<RegValue>> {
    let key = wide(key);
    let mut handle = HKEY::default();
    // SAFETY: `key` is NUL-terminated; `handle` receives the opened key.
    let status = unsafe { RegOpenKeyExW(root, PCWSTR(key.as_ptr()), None, KEY_READ, &mut handle) };
    if status != ERROR_SUCCESS {
        return None;
    }
    let values = enum_values(handle);
    // SAFETY: `handle` was opened above and is not used afterwards.
    let _ = unsafe { RegCloseKey(handle) };
    Some(values)
}

/// Enumerates the values of the open key `handle`.
fn enum_values(handle: HKEY) -> Vec<RegValue> {
    let mut values = Vec::new();
    let (mut name_cap, mut data_cap) = buffer_sizes(handle);
    let mut index = 0u32;
    let mut attempts = 0;
    loop {
        let mut name = vec![0u16; name_cap];
        let mut name_len = u32::try_from(name.len()).unwrap_or(u32::MAX);
        let mut data = vec![0u8; data_cap];
        let mut data_len = u32::try_from(data.len()).unwrap_or(u32::MAX);
        let mut kind = 0u32;
        // SAFETY: `handle` is open; `name` holds `name_len` characters and
        // `data` holds `data_len` bytes, as passed.
        let status = unsafe {
            RegEnumValueW(
                handle,
                index,
                Some(windows::core::PWSTR(name.as_mut_ptr())),
                &mut name_len,
                None,
                Some(&mut kind),
                Some(data.as_mut_ptr()),
                Some(&mut data_len),
            )
        };
        if status == ERROR_MORE_DATA && attempts < VALUE_ATTEMPTS {
            // The value (its name or data) grew since the sizes were queried.
            attempts += 1;
            let (name_max, data_max) = buffer_sizes(handle);
            name_cap = name_max.max(name_cap.saturating_mul(2)).min(MAX_VALUE_NAME);
            data_cap = data_max.max(data_len as usize).max(data_cap);
            continue;
        }
        if status == ERROR_SUCCESS {
            data.truncate(data_len as usize);
            let name = from_wide(&name[..(name_len as usize).min(name.len())]);
            values.push(RegValue {
                name: name.to_string_lossy().into_owned(),
                data: crate::registry::value::decode(kind, data),
            });
        } else if status != ERROR_MORE_DATA {
            // ERROR_NO_MORE_ITEMS ends the list; any other error means the
            // key cannot be enumerated further.
            if status != ERROR_NO_MORE_ITEMS {
                tracing::debug!(
                    status = status.0,
                    index,
                    "registry value enumeration stopped"
                );
            }
            break;
        }
        // A value read (or skipped after too many attempts): next one.
        index += 1;
        attempts = 0;
    }
    values
}

/// Buffer sizes for `RegEnumValueW`: the longest value name in characters
/// (with the NUL) and the largest data in bytes, at least 1 each; generous
/// defaults if the key cannot be queried.
fn buffer_sizes(handle: HKEY) -> (usize, usize) {
    let mut name_max = 0u32;
    let mut data_max = 0u32;
    // SAFETY: `handle` is open; the two counters receive the maximum sizes,
    // every other output is not requested.
    let status = unsafe {
        RegQueryInfoKeyW(
            handle,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(&mut name_max),
            Some(&mut data_max),
            None,
            None,
        )
    };
    if status != ERROR_SUCCESS {
        return (MAX_VALUE_NAME, 4096);
    }
    let name = (name_max as usize + 1).min(MAX_VALUE_NAME);
    (name, (data_max as usize).max(1))
}
