//! Logical drives and their volumes.

use std::ffi::c_void;

use windows::core::PCWSTR;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Storage::FileSystem::{
    CreateFileW, GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
    FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Diagnostics::Debug::{
    SetThreadErrorMode, SEM_FAILCRITICALERRORS, THREAD_ERROR_MODE,
};
use windows::Win32::System::Ioctl::{
    PropertyStandardQuery, StorageDeviceSeekPenaltyProperty, DEVICE_SEEK_PENALTY_DESCRIPTOR,
    IOCTL_STORAGE_QUERY_PROPERTY, STORAGE_PROPERTY_QUERY,
};
use windows::Win32::System::IO::DeviceIoControl;

use super::{from_wide, wide};
use crate::env::{DriveInfo, DriveKind, DriveMedia};

// GetDriveTypeW results.
const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;
const DRIVE_REMOTE: u32 = 4;
const DRIVE_CDROM: u32 = 5;

/// All logical drives. Volumes are queried only for fixed and removable drives,
/// so a disconnected network drive cannot block the scan (SPEC-02 §3.3).
pub(crate) fn drives() -> Vec<DriveInfo> {
    // No "insert a disk" dialogs for empty card readers.
    let mut previous = THREAD_ERROR_MODE::default();
    // SAFETY: `previous` receives the old mode, restored below.
    let restore =
        unsafe { SetThreadErrorMode(SEM_FAILCRITICALERRORS, Some(&mut previous)) }.is_ok();

    // SAFETY: no arguments.
    let mask = unsafe { GetLogicalDrives() };
    let drives = (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| drive(char::from(b'A' + i)))
        .collect();

    if restore {
        // SAFETY: restores the mode saved above.
        let _ = unsafe { SetThreadErrorMode(previous, None) };
    }
    drives
}

fn drive(letter: char) -> DriveInfo {
    let root = wide(format!(r"{letter}:\"));
    // SAFETY: `root` is NUL-terminated.
    let kind = match unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) } {
        DRIVE_FIXED => DriveKind::Fixed,
        DRIVE_REMOVABLE => DriveKind::Removable,
        DRIVE_REMOTE => DriveKind::Network,
        DRIVE_CDROM => DriveKind::CdRom,
        _ => DriveKind::Unknown,
    };
    let mut info = DriveInfo {
        letter,
        kind,
        media: DriveMedia::Unknown,
        fs: None,
        label: None,
        volume_serial: None,
        total_bytes: 0,
        free_bytes: 0,
    };
    if !matches!(kind, DriveKind::Fixed | DriveKind::Removable) {
        return info;
    }

    let mut label = [0u16; 261];
    let mut fs = [0u16; 261];
    let mut serial = 0u32;
    // SAFETY: `root` is NUL-terminated; the buffers are writable slices.
    let volume = unsafe {
        GetVolumeInformationW(
            PCWSTR(root.as_ptr()),
            Some(&mut label),
            Some(&mut serial),
            None,
            None,
            Some(&mut fs),
        )
    };
    if volume.is_ok() {
        let label = from_wide(&label).to_string_lossy().into_owned();
        info.label = (!label.is_empty()).then_some(label);
        info.fs = Some(from_wide(&fs).to_string_lossy().into_owned());
        info.volume_serial = Some(serial);
    }

    let (mut free, mut total) = (0u64, 0u64);
    // SAFETY: `root` is NUL-terminated; the outputs are valid u64 locations.
    if unsafe {
        GetDiskFreeSpaceExW(
            PCWSTR(root.as_ptr()),
            Some(&mut free),
            Some(&mut total),
            None,
        )
    }
    .is_ok()
    {
        info.free_bytes = free;
        info.total_bytes = total;
    }
    info.media = media(letter);
    info
}

/// SSD or HDD by the seek penalty of the volume's device.
fn media(letter: char) -> DriveMedia {
    let device = wide(format!(r"\\.\{letter}:"));
    // SAFETY: `device` is NUL-terminated. Zero access rights are enough for
    // IOCTL_STORAGE_QUERY_PROPERTY and need no elevation.
    let Ok(handle) = (unsafe {
        CreateFileW(
            PCWSTR(device.as_ptr()),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
    }) else {
        return DriveMedia::Unknown;
    };
    let query = STORAGE_PROPERTY_QUERY {
        PropertyId: StorageDeviceSeekPenaltyProperty,
        QueryType: PropertyStandardQuery,
        ..Default::default()
    };
    let mut result = DEVICE_SEEK_PENALTY_DESCRIPTOR::default();
    let mut returned = 0u32;
    // SAFETY: input and output point to structs of the passed sizes; `handle` is open.
    let ok = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_STORAGE_QUERY_PROPERTY,
            Some((&raw const query).cast::<c_void>()),
            size_of::<STORAGE_PROPERTY_QUERY>() as u32,
            Some((&raw mut result).cast::<c_void>()),
            size_of::<DEVICE_SEEK_PENALTY_DESCRIPTOR>() as u32,
            Some(&mut returned),
            None,
        )
    }
    .is_ok();
    // SAFETY: `handle` was opened above and is not used afterwards.
    let _ = unsafe { CloseHandle(handle) };
    match (ok, result.IncursSeekPenalty) {
        (true, true) => DriveMedia::Hdd,
        (true, false) => DriveMedia::Ssd,
        (false, _) => DriveMedia::Unknown,
    }
}
