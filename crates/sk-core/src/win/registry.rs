//! Read-only registry access.

use windows::core::PCWSTR;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, KEY_READ, RRF_RT_REG_DWORD,
    RRF_RT_REG_SZ,
};

use super::{from_wide, wide};

pub(crate) use windows::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

/// A `REG_SZ` value; `None` if the key or value is missing or has another type.
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
    let mut buf = vec![0u16; (size as usize).div_ceil(2)];
    let mut size = u32::try_from(buf.len() * 2).ok()?;
    // SAFETY: `buf` holds `size` bytes, as passed in `pcbdata`.
    let status = unsafe {
        RegGetValueW(
            root,
            PCWSTR(key.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    (status == ERROR_SUCCESS).then(|| from_wide(&buf).to_string_lossy().into_owned())
}

/// A `REG_DWORD` value.
pub(crate) fn dword(root: HKEY, key: &str, value: &str) -> Option<u32> {
    let (key, value) = (wide(key), wide(value));
    let mut data = 0u32;
    let mut size = 4u32;
    // SAFETY: `data` is a 4-byte buffer, as passed in `pcbdata`.
    let status = unsafe {
        RegGetValueW(
            root,
            PCWSTR(key.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut data).cast()),
            Some(&mut size),
        )
    };
    (status == ERROR_SUCCESS).then_some(data)
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
