//! User, token, OS version and processes.

use std::collections::BTreeSet;

use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL};
use windows::Win32::Globalization::GetUserDefaultUILanguage;
use windows::Win32::Globalization::LCIDToLocaleName;
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{
    GetTokenInformation, TokenElevation, TokenUser, TOKEN_ELEVATION, TOKEN_QUERY, TOKEN_USER,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::SystemInformation::{
    ComputerNameNetBIOS, GetComputerNameExW, GetNativeSystemInfo, SYSTEM_INFO,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::System::WindowsProgramming::GetUserNameW;

use super::from_wide;
use super::registry::{self, HKEY_LOCAL_MACHINE};
use crate::env::{EnvError, OsInfo};

const PROCESSOR_ARCHITECTURE_INTEL: u16 = 0;
const PROCESSOR_ARCHITECTURE_ARM64: u16 = 12;
const PROCESSOR_ARCHITECTURE_AMD64: u16 = 9;

fn os_error(api: &'static str, err: &windows::core::Error) -> EnvError {
    EnvError::Os {
        api,
        code: err.code().0,
    }
}

/// Login name of the current user.
pub(crate) fn user_name() -> Result<String, EnvError> {
    // UNLEN + 1.
    let mut buf = [0u16; 257];
    let mut len = buf.len() as u32;
    // SAFETY: `buf` holds `len` characters.
    unsafe { GetUserNameW(Some(PWSTR(buf.as_mut_ptr())), &mut len) }
        .map_err(|e| os_error("GetUserNameW", &e))?;
    Ok(from_wide(&buf).to_string_lossy().into_owned())
}

/// NetBIOS computer name; empty if unavailable.
pub(crate) fn machine_name() -> String {
    let mut buf = [0u16; 256];
    let mut len = buf.len() as u32;
    // SAFETY: `buf` holds `len` characters.
    match unsafe {
        GetComputerNameExW(ComputerNameNetBIOS, Some(PWSTR(buf.as_mut_ptr())), &mut len)
    } {
        Ok(()) => from_wide(&buf).to_string_lossy().into_owned(),
        Err(_) => String::new(),
    }
}

/// Elevation and SID of the current process token.
pub(crate) fn token_info() -> (bool, Option<String>) {
    let mut token = HANDLE::default();
    // SAFETY: the pseudo handle of the current process is always valid.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.is_err() {
        return (false, None);
    }
    let elevated = elevation(token);
    let sid = sid(token);
    // SAFETY: `token` was opened above and is not used afterwards.
    let _ = unsafe { CloseHandle(token) };
    (elevated, sid)
}

fn elevation(token: HANDLE) -> bool {
    let mut info = TOKEN_ELEVATION::default();
    let mut len = 0u32;
    // SAFETY: `info` is a TOKEN_ELEVATION buffer of the passed size.
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            Some((&raw mut info).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
    };
    ok.is_ok() && info.TokenIsElevated != 0
}

fn sid(token: HANDLE) -> Option<String> {
    let mut len = 0u32;
    // SAFETY: a null buffer asks for the required size only; the call fails by design.
    let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut len) };
    if len == 0 {
        return None;
    }
    // u64 elements keep the buffer aligned for TOKEN_USER.
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    // SAFETY: `buf` holds at least `len` bytes and is 8-byte aligned.
    unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            Some(buf.as_mut_ptr().cast()),
            len,
            &mut len,
        )
    }
    .ok()?;
    // SAFETY: on success the buffer starts with a TOKEN_USER whose SID points into the buffer.
    let user = unsafe { &*buf.as_ptr().cast::<TOKEN_USER>() };
    let mut raw = PWSTR::null();
    // SAFETY: the SID is valid while `buf` lives; `raw` receives a LocalAlloc'ed string.
    unsafe { ConvertSidToStringSidW(user.User.Sid, &mut raw) }.ok()?;
    // SAFETY: `raw` is a valid NUL-terminated string after a successful call.
    let sid = unsafe { raw.to_string() }.ok();
    // SAFETY: `raw` was allocated with LocalAlloc and is not used afterwards.
    let _ = unsafe { LocalFree(Some(HLOCAL(raw.0.cast()))) };
    sid
}

/// OS version from `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion`.
pub(crate) fn os_info() -> OsInfo {
    const KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let build = registry::string(HKEY_LOCAL_MACHINE, KEY, "CurrentBuildNumber").unwrap_or_default();
    let build_number = build.parse::<u32>().unwrap_or(0);
    let product = registry::string(HKEY_LOCAL_MACHINE, KEY, "ProductName").unwrap_or_default();
    let full_build = match registry::dword(HKEY_LOCAL_MACHINE, KEY, "UBR") {
        Some(ubr) => format!("{build}.{ubr}"),
        None => build,
    };
    OsInfo {
        product: product_name(&product, build_number),
        display_version: registry::string(HKEY_LOCAL_MACHINE, KEY, "DisplayVersion"),
        build: full_build,
        arch: arch().to_owned(),
        ui_language: ui_language(),
    }
}

/// Windows 11 still reports "Windows 10 ..." in `ProductName` (SPEC-02 §3.3).
pub(crate) fn product_name(product: &str, build: u32) -> String {
    match product.strip_prefix("Windows 10") {
        Some(rest) if build >= 22000 => format!("Windows 11{rest}"),
        _ => product.to_owned(),
    }
}

fn arch() -> &'static str {
    let mut info = SYSTEM_INFO::default();
    // SAFETY: `info` is a valid SYSTEM_INFO buffer.
    unsafe { GetNativeSystemInfo(&mut info) };
    // SAFETY: the processor architecture variant of the union is always set.
    match unsafe { info.Anonymous.Anonymous.wProcessorArchitecture.0 } {
        PROCESSOR_ARCHITECTURE_AMD64 => "x86_64",
        PROCESSOR_ARCHITECTURE_ARM64 => "aarch64",
        PROCESSOR_ARCHITECTURE_INTEL => "x86",
        _ => "unknown",
    }
}

fn ui_language() -> String {
    // SAFETY: no arguments.
    let lang_id = unsafe { GetUserDefaultUILanguage() };
    // LOCALE_NAME_MAX_LENGTH.
    let mut buf = [0u16; 85];
    // SAFETY: `buf` is a writable buffer; a LANGID with SORT_DEFAULT is a valid LCID.
    let len = unsafe { LCIDToLocaleName(u32::from(lang_id), Some(&mut buf), 0) };
    if len <= 0 {
        return "en-US".to_owned();
    }
    from_wide(&buf).to_string_lossy().into_owned()
}

/// Lowercase executable names of running processes, sorted, without duplicates.
pub(crate) fn running_processes() -> Vec<String> {
    // SAFETY: a process snapshot takes no pointers.
    let Ok(snapshot) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else {
        return Vec::new();
    };
    let mut names = BTreeSet::new();
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    // SAFETY: `snapshot` is open and `entry.dwSize` is set as required.
    let mut ok = unsafe { Process32FirstW(snapshot, &mut entry) }.is_ok();
    while ok {
        let name = from_wide(&entry.szExeFile).to_string_lossy().to_lowercase();
        if !name.is_empty() {
            names.insert(name);
        }
        // SAFETY: as above.
        ok = unsafe { Process32NextW(snapshot, &mut entry) }.is_ok();
    }
    // SAFETY: `snapshot` was opened above and is not used afterwards.
    let _ = unsafe { CloseHandle(snapshot) };
    names.into_iter().collect()
}
