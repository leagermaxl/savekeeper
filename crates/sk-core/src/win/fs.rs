//! File attributes without opening the file (SPEC-02 §3.5).
//!
//! `GetFileAttributesW` only reads the directory entry, so a cloud-only file
//! is not recalled. Outside Windows there are no such attributes: the stub
//! checks that the entry exists and returns 0.

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

/// Sets the `FILE_ATTRIBUTE_*` flags of `path` (tests only: nothing in
/// `sk-core` changes user files).
#[cfg(all(test, windows))]
pub(crate) fn set_attributes(path: &Path, attrs: u32) -> windows::core::Result<()> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{SetFileAttributesW, FILE_FLAGS_AND_ATTRIBUTES};

    let name = super::wide(crate::path::to_extended(path));
    // SAFETY: `name` is NUL-terminated and outlives the call.
    unsafe { SetFileAttributesW(PCWSTR(name.as_ptr()), FILE_FLAGS_AND_ATTRIBUTES(attrs)) }
}
