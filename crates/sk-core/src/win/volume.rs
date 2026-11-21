//! The volume that holds a folder: free space and file system (SPEC-10 §4.6).
//!
//! Outside Windows there is no such query: the stub checks that the folder
//! exists and reports unknown values with unlimited free space.

use std::io;
use std::path::Path;

use crate::fs::VolumeInfo;

/// Volume of the existing folder `path` (see [`crate::fs::volume_info`]).
#[cfg(windows)]
pub(crate) fn volume_info(path: &Path) -> io::Result<VolumeInfo> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        GetDiskFreeSpaceExW, GetDriveTypeW, GetVolumeInformationW,
    };
    use windows::Win32::System::SystemInformation::GetSystemWindowsDirectoryW;

    const DRIVE_REMOTE: u32 = 4;

    if !std::fs::metadata(crate::path::to_extended(path))?.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            "not a directory",
        ));
    }
    let name = super::wide(crate::path::to_extended(path));
    let mut free = 0u64;
    // SAFETY: `name` is NUL-terminated and outlives the call; `free` is a valid u64.
    unsafe { GetDiskFreeSpaceExW(PCWSTR(name.as_ptr()), Some(&mut free), None, None) }
        .map_err(io::Error::from)?;

    let root = volume_root(path)?;
    let root_wide = super::wide(&root);
    let mut fs = [0u16; 261];
    // SAFETY: `root_wide` is NUL-terminated; `fs` is a writable slice.
    let fs_name = unsafe {
        GetVolumeInformationW(
            PCWSTR(root_wide.as_ptr()),
            None,
            None,
            None,
            None,
            Some(&mut fs),
        )
    }
    .ok()
    .map(|()| super::from_wide(&fs).to_string_lossy().into_owned())
    .filter(|name| !name.is_empty());
    // SAFETY: `root_wide` is NUL-terminated.
    let remote = unsafe { GetDriveTypeW(PCWSTR(root_wide.as_ptr())) } == DRIVE_REMOTE;

    let mut windir = [0u16; 261];
    // SAFETY: `windir` is a writable slice; the result is its used length.
    let len = unsafe { GetSystemWindowsDirectoryW(Some(&mut windir)) } as usize;
    let system = (len > 0 && len < windir.len())
        .then(|| volume_root(Path::new(&super::from_wide(&windir[..len]))))
        .and_then(Result::ok)
        .is_some_and(|windows_root| crate::path::eq_ci(&windows_root, &root));

    Ok(VolumeInfo {
        root,
        fs_name,
        remote,
        free_bytes: free,
        system,
    })
}

/// Mount point of the volume of `path` (`C:\`, `\\server\share\`), without
/// the `\\?\` prefix.
#[cfg(windows)]
fn volume_root(path: &Path) -> io::Result<std::path::PathBuf> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::GetVolumePathNameW;

    let name = super::wide(crate::path::to_extended(path));
    let mut buf = vec![0u16; name.len().max(261)];
    // SAFETY: `name` is NUL-terminated; `buf` is a writable slice at least as
    // long as the input, which bounds the result.
    unsafe { GetVolumePathNameW(PCWSTR(name.as_ptr()), &mut buf) }.map_err(io::Error::from)?;
    let root = super::from_wide(&buf).to_string_lossy().into_owned();
    let plain = if let Some(unc) = root.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else if let Some(local) = root.strip_prefix(r"\\?\") {
        local.to_owned()
    } else {
        root
    };
    Ok(std::path::PathBuf::from(plain))
}

/// Stub for other OSes: the folder must exist; the root is `/`, the file
/// system unknown and the free space unlimited.
#[cfg(not(windows))]
pub(crate) fn volume_info(path: &Path) -> io::Result<VolumeInfo> {
    if !std::fs::metadata(path)?.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            "not a directory",
        ));
    }
    Ok(VolumeInfo {
        root: std::path::PathBuf::from("/"),
        fs_name: None,
        remote: false,
        free_bytes: u64::MAX,
        system: false,
    })
}
