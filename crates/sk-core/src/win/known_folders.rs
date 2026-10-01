//! Known Folders API (`SHGetKnownFolderPath`).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use windows::core::GUID;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_LocalAppData,
    FOLDERID_LocalAppDataLow, FOLDERID_Music, FOLDERID_Pictures, FOLDERID_Profile,
    FOLDERID_ProgramData, FOLDERID_ProgramFiles, FOLDERID_ProgramFilesX86, FOLDERID_Public,
    FOLDERID_RoamingAppData, FOLDERID_SavedGames, FOLDERID_Videos, FOLDERID_Windows,
    SHGetKnownFolderPath, KF_FLAG_DEFAULT,
};

use crate::env::KnownFolder;

fn folder_id(folder: KnownFolder) -> &'static GUID {
    match folder {
        KnownFolder::Home => &FOLDERID_Profile,
        KnownFolder::AppData => &FOLDERID_RoamingAppData,
        KnownFolder::LocalAppData => &FOLDERID_LocalAppData,
        KnownFolder::LocalLow => &FOLDERID_LocalAppDataLow,
        KnownFolder::Documents => &FOLDERID_Documents,
        KnownFolder::Desktop => &FOLDERID_Desktop,
        KnownFolder::Pictures => &FOLDERID_Pictures,
        KnownFolder::Music => &FOLDERID_Music,
        KnownFolder::Videos => &FOLDERID_Videos,
        KnownFolder::Downloads => &FOLDERID_Downloads,
        KnownFolder::SavedGames => &FOLDERID_SavedGames,
        KnownFolder::ProgramData => &FOLDERID_ProgramData,
        KnownFolder::Public => &FOLDERID_Public,
        KnownFolder::WinDir => &FOLDERID_Windows,
        KnownFolder::ProgramFiles => &FOLDERID_ProgramFiles,
        KnownFolder::ProgramFilesX86 => &FOLDERID_ProgramFilesX86,
    }
}

/// Current path of a known folder, `None` if the folder does not exist or is
/// not available (deleted, offline network redirect).
pub(crate) fn path(folder: KnownFolder) -> Option<PathBuf> {
    // SAFETY: `folder_id` returns a reference to a static GUID; no token is
    // passed, so the current user is used.
    let raw = unsafe { SHGetKnownFolderPath(folder_id(folder), KF_FLAG_DEFAULT, None) }.ok()?;
    // SAFETY: on success `raw` is a valid NUL-terminated string allocated by
    // the shell; it is read once and freed below.
    let path = OsString::from_wide(unsafe { raw.as_wide() });
    // SAFETY: the buffer was allocated with CoTaskMemAlloc and is not used afterwards.
    unsafe { CoTaskMemFree(Some(raw.0.cast_const().cast())) };
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// All known folders that are available.
pub(crate) fn all() -> BTreeMap<KnownFolder, PathBuf> {
    KnownFolder::ALL
        .into_iter()
        .filter_map(|f| path(f).map(|p| (f, p)))
        .collect()
}
