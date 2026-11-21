//! File attributes without opening the file (SPEC-02 §3.5, §3.7).
//!
//! `GetFileAttributesW` only reads the directory entry, so a cloud-only file
//! is not recalled. Outside Windows there are no such attributes: the stub
//! checks that the entry exists and returns 0.
//!
//! `SetFileAttributesW` is used only on files the backup wrote itself
//! (SPEC-10 §4.3) and in tests; user files are never changed (principle P1).

use std::io;
use std::path::Path;

/// Raw `FILE_ATTRIBUTE_*` flags of the entry at `path` itself (a link is not
/// followed); the file is not opened. A missing entry is
/// [`io::ErrorKind::NotFound`], other failures are the OS error as is.
#[cfg(windows)]
pub(crate) fn attributes(path: &Path) -> io::Result<u32> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::GetLastError;
    use windows::Win32::Storage::FileSystem::{GetFileAttributesW, INVALID_FILE_ATTRIBUTES};

    let name = super::wide(crate::path::to_extended(path));
    // SAFETY: `name` is NUL-terminated and outlives the call.
    let attrs = unsafe { GetFileAttributesW(PCWSTR(name.as_ptr())) };
    if attrs == INVALID_FILE_ATTRIBUTES {
        // SAFETY: no arguments; reads the error of the call just above.
        let code = unsafe { GetLastError() };
        // Win32 error codes fit in an `i32`, as `std` itself assumes.
        return Err(io::Error::from_raw_os_error(code.0 as i32));
    }
    Ok(attrs)
}

/// Stub for other OSes: 0 for an existing entry (a link is not followed).
#[cfg(not(windows))]
pub(crate) fn attributes(path: &Path) -> io::Result<u32> {
    std::fs::symlink_metadata(path).map(|_| 0)
}

/// Sets the `FILE_ATTRIBUTE_*` flags of the entry at `path` (`SetFileAttributesW`
/// on the extended path); 0 means `FILE_ATTRIBUTE_NORMAL`.
#[cfg(windows)]
pub(crate) fn set_attributes(path: &Path, attrs: u32) -> io::Result<()> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        SetFileAttributesW, FILE_ATTRIBUTE_NORMAL, FILE_FLAGS_AND_ATTRIBUTES,
    };

    let attrs = if attrs == 0 {
        FILE_ATTRIBUTE_NORMAL
    } else {
        FILE_FLAGS_AND_ATTRIBUTES(attrs)
    };
    let name = super::wide(crate::path::to_extended(path));
    // SAFETY: `name` is NUL-terminated and outlives the call.
    unsafe { SetFileAttributesW(PCWSTR(name.as_ptr()), attrs) }.map_err(|e| {
        // `HRESULT_FROM_WIN32`: keep the Win32 code, so that `kind()` works.
        let hr = e.code().0 as u32;
        if hr & 0xFFFF_0000 == 0x8007_0000 {
            io::Error::from_raw_os_error((hr & 0xFFFF) as i32)
        } else {
            io::Error::from(e)
        }
    })
}

/// Stub for other OSes: only `FILE_ATTRIBUTE_READONLY` (0x1) has a counterpart,
/// the read-only permission; the other flags are ignored.
#[cfg(not(windows))]
pub(crate) fn set_attributes(path: &Path, attrs: u32) -> io::Result<()> {
    let mut perms = std::fs::symlink_metadata(path)?.permissions();
    let readonly = attrs & 0x1 != 0;
    if perms.readonly() != readonly {
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(readonly);
        std::fs::set_permissions(path, perms)?;
    }
    Ok(())
}
