//! Read-only registry access (SPEC-02 §3.4). Keys are only opened with
//! `KEY_READ`; nothing here writes to the registry (principle P1).

use windows::core::PCWSTR;
use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_MORE_DATA, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, KEY_READ, REG_DWORD,
    REG_VALUE_TYPE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
};

use super::{from_wide, wide};
use crate::model::RegHive;
use crate::registry::KeyState;

pub(crate) use windows::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

/// Attempts to read a string whose expanded size grows between two calls.
const STRING_ATTEMPTS: usize = 4;

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
