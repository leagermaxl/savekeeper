//! FFI calls: `FindFirstFileExW` for reparse tags and entry metadata,
//! `GetFileAttributesW` and `CreateFileW` for trial opens.

use std::ffi::{c_void, OsStr};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::MetadataExt;
use std::path::{Component, Path};

use sk_core::fs::{CloudState, FsError, Readability};
use sk_core::path::to_extended;
use time::OffsetDateTime;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, GetLastError, FILETIME, GENERIC_READ};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FindClose, FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW,
    GetFileAttributesW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_NO_RECALL,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FIND_FIRST_EX_FLAGS, INVALID_FILE_ATTRIBUTES, OPEN_EXISTING, WIN32_FIND_DATAW,
};

use super::{
    cloud_state, readability_from_error, std_times, RawMeta, FILE_ATTRIBUTE_DIRECTORY,
    FILE_ATTRIBUTE_REPARSE_POINT,
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
    let data = find_data(path)?;
    if data.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        Ok(Some(data.dwReserved0))
    } else {
        Ok(None)
    }
}

/// Metadata of the entry at `path`, without following reparse points and
/// without opening it: from `FindFirstFileExW` data, like a listing. A drive
/// or share root, which enumeration cannot name, is read through
/// `std::fs::metadata` (a root is neither a link nor a cloud entry).
pub(crate) fn find_meta(path: &Path) -> Result<RawMeta, FsError> {
    let extended = to_extended(path);
    if extended.file_name().is_none() {
        let md = std::fs::metadata(&extended)?;
        let attrs = md.file_attributes();
        let (mtime, ctime) = std_times(&md);
        return Ok(RawMeta {
            attrs,
            is_dir: attrs & FILE_ATTRIBUTE_DIRECTORY != 0,
            is_symlink: false,
            tag: None,
            size: md.len(),
            mtime,
            ctime,
        });
    }
    let data = find_data(&extended)?;
    let attrs = data.dwFileAttributes;
    Ok(RawMeta {
        attrs,
        is_dir: attrs & FILE_ATTRIBUTE_DIRECTORY != 0,
        is_symlink: false,
        tag: (attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0).then_some(data.dwReserved0),
        size: u64::from(data.nFileSizeHigh) << 32 | u64::from(data.nFileSizeLow),
        mtime: filetime(data.ftLastWriteTime),
        ctime: filetime(data.ftCreationTime),
    })
}

/// [`RawMeta`] of a listing entry at `path` from `std::fs::DirEntry::metadata`
/// (the listing's `WIN32_FIND_DATAW`, no system call). `std` does not expose
/// the reparse tag, so a reparse point is looked up by [`reparse_tag`]; if
/// that fails, the tag is reported as 0 (`ReparseKind::Other(0)`), so the
/// entry is still not entered.
pub(crate) fn listing_meta(md: &std::fs::Metadata, path: &Path) -> RawMeta {
    let attrs = md.file_attributes();
    let tag = if attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        reparse_tag(path).unwrap_or(Some(0))
    } else {
        None
    };
    let (mtime, ctime) = std_times(md);
    RawMeta {
        attrs,
        is_dir: attrs & FILE_ATTRIBUTE_DIRECTORY != 0,
        is_symlink: false,
        tag,
        size: md.len(),
        mtime,
        ctime,
    }
}

/// A `FILETIME` (100 ns ticks since 1601-01-01 UTC) as a date; `None` for 0.
pub(super) fn filetime(ft: FILETIME) -> Option<OffsetDateTime> {
    /// Ticks between 1601-01-01 and 1970-01-01.
    const UNIX_EPOCH_TICKS: i128 = 116_444_736_000_000_000;
    let ticks = u64::from(ft.dwHighDateTime) << 32 | u64::from(ft.dwLowDateTime);
    if ticks == 0 {
        return None;
    }
    OffsetDateTime::from_unix_timestamp_nanos((i128::from(ticks) - UNIX_EPOCH_TICKS) * 100).ok()
}

/// Sets the `FILE_ATTRIBUTE_*` flags of `path` (tests only: scanning never
/// changes anything).
#[cfg(test)]
pub(crate) fn set_attributes(path: &Path, attrs: u32) -> windows::core::Result<()> {
    use windows::Win32::Storage::FileSystem::{SetFileAttributesW, FILE_FLAGS_AND_ATTRIBUTES};

    let name = wide(to_extended(path).as_os_str());
    // SAFETY: `name` is NUL-terminated and outlives the call.
    unsafe { SetFileAttributesW(PCWSTR(name.as_ptr()), FILE_FLAGS_AND_ATTRIBUTES(attrs)) }
}

/// Whether a component of `path` after the prefix contains a search wildcard
/// of `FindFirstFileExW`: `*`, `?`, or a DOS wildcard `<`, `>`, `"`. The
/// `?` of a `\\?\` prefix does not count.
pub(super) fn has_wildcard(path: &Path) -> bool {
    path.components().any(|c| match c {
        Component::Normal(name) => name
            .encode_wide()
            .any(|c| matches!(c, 0x2A | 0x3F | 0x3C | 0x3E | 0x22)),
        _ => false,
    })
}

/// `WIN32_FIND_DATAW` of the entry at `path` (`FindFirstFileExW`,
/// `FindExInfoBasic`); the entry is not opened.
///
/// `FindFirstFileExW` reads the last component as a search pattern, even
/// with the `\\?\` prefix (`win*.exe` would find `winhlp32.exe`). A path with
/// a wildcard (`* ?` or the DOS wildcards `< > "`) in any component cannot
/// name an entry on Windows, so it is `NotFound` without calling the API
/// (as in `MemFs`). This guards every caller: `reparse_tag` and `find_meta`.
fn find_data(path: &Path) -> Result<WIN32_FIND_DATAW, FsError> {
    if has_wildcard(path) {
        return Err(FsError::NotFound);
    }
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
    Ok(data)
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
