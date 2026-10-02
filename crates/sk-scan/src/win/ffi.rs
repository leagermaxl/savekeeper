//! FFI calls: `FindFirstFileExW` for reparse tags, `GetFileAttributesW` and
//! `CreateFileW` for trial opens.

use std::ffi::{c_void, OsStr};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use sk_core::fs::{CloudState, FsError, Readability};
use sk_core::path::to_extended;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, GetLastError, GENERIC_READ};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FindClose, FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW,
    GetFileAttributesW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_NO_RECALL,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FIND_FIRST_EX_FLAGS, INVALID_FILE_ATTRIBUTES, OPEN_EXISTING, WIN32_FIND_DATAW,
};

use super::{
    cloud_state, readability_from_error, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
};

/// NUL-terminated UTF-16 copy of a path, for `PCWSTR` arguments.
fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// Win32 error code inside an `HRESULT` of the `FACILITY_WIN32` kind
/// (`0x8007xxxx`); other `HRESULT`s are returned as is.
fn win32_code(e: &windows::core::Error) -> u32 {
    let hr = e.code().0 as u32;
    if hr & 0xFFFF_0000 == 0x8007_0000 {
        hr & 0xFFFF
    } else {
        hr
    }
}

/// The reparse tag of the entry at `path` (`dwReserved0` of its
/// `WIN32_FIND_DATAW`), or `None` if it is not a reparse point.
///
/// The entry is looked up by directory enumeration (`FindFirstFileExW` with
/// `FindExInfoBasic`), so it is never opened and a cloud placeholder is not
/// recalled. Reparse points are not followed. The path must name an entry,
/// not a drive root.
pub(crate) fn reparse_tag(path: &Path) -> Result<Option<u32>, FsError> {
    let name = wide(to_extended(path).as_os_str());
    let mut data = WIN32_FIND_DATAW::default();
    // SAFETY: `name` is NUL-terminated and outlives the call; `data` is a
    // writable `WIN32_FIND_DATAW`, the structure `FindExInfoBasic` fills.
    let found = unsafe {
        FindFirstFileExW(
            PCWSTR(name.as_ptr()),
            FindExInfoBasic,
            (&raw mut data).cast::<c_void>(),
            FindExSearchNameMatch,
            None,
            FIND_FIRST_EX_FLAGS(0),
        )
    };
    let handle = match found {
        Ok(handle) => handle,
        Err(e) => {
            return Err(FsError::from(std::io::Error::from_raw_os_error(
                win32_code(&e) as i32,
            )))
        }
    };
    // SAFETY: `handle` is the valid search handle returned above, closed once.
    let _ = unsafe { FindClose(handle) };

    if data.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        Ok(Some(data.dwReserved0))
    } else {
        Ok(None)
    }
}

/// Trial open of `path` for reading, without reading anything.
///
/// Links are not followed (FR-03-02): the attributes are those of the entry
/// itself (`GetFileAttributesW`, which does not open the content), and a
/// cloud-only entry (FR-03-03) is `CloudOnly` and is not opened. Otherwise
/// `CreateFileW` with `GENERIC_READ`, sharing `READ | WRITE | DELETE`,
/// `OPEN_EXISTING`, `FILE_FLAG_OPEN_NO_RECALL | FILE_FLAG_OPEN_REPARSE_POINT`
/// (and `FILE_FLAG_BACKUP_SEMANTICS` for directories), so for a reparse point
/// the link itself is opened, not its target; the handle is closed at once.
/// A sharing or lock violation is `Locked`, a missing path `Missing`, any
/// other failure `Denied`.
pub(crate) fn probe_readable(path: &Path) -> Readability {
    let name = wide(to_extended(path).as_os_str());
    // SAFETY: `name` is NUL-terminated and outlives the call.
    let attrs = unsafe { GetFileAttributesW(PCWSTR(name.as_ptr())) };
    if attrs == INVALID_FILE_ATTRIBUTES {
        // SAFETY: no arguments; reads the error of the call just above.
        let code = unsafe { GetLastError() };
        return readability_from_error(code.0);
    }
    if cloud_state(attrs) == CloudState::CloudOnly {
        return Readability::CloudOnly;
    }

    let mut flags = FILE_FLAG_OPEN_NO_RECALL | FILE_FLAG_OPEN_REPARSE_POINT;
    if attrs & FILE_ATTRIBUTE_DIRECTORY != 0 {
        flags |= FILE_FLAG_BACKUP_SEMANTICS;
    }
    // SAFETY: `name` is NUL-terminated and outlives the call; no security
    // attributes and no template file are passed.
    let opened = unsafe {
        CreateFileW(
            PCWSTR(name.as_ptr()),
            GENERIC_READ.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            flags,
            None,
        )
    };
    match opened {
        Ok(handle) => {
            // SAFETY: `handle` is the valid handle opened above, closed once.
            let _ = unsafe { CloseHandle(handle) };
            Readability::Ok
        }
        Err(e) => readability_from_error(win32_code(&e)),
    }
}
