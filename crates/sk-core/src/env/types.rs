//! Types inside [`Environment`](super::Environment) (SPEC-02 §3.3).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use specta::Type;

/// Operating system version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub struct OsInfo {
    /// "Windows 11 Pro" (the edition is part of the string).
    pub product: String,
    /// "24H2" (`DisplayVersion` in `HKLM\...\CurrentVersion`).
    pub display_version: Option<String>,
    /// "26100.2033" = `CurrentBuildNumber` + "." + `UBR`.
    pub build: String,
    /// Architecture of the OS (not the process): "x86_64" | "aarch64" | "x86".
    pub arch: String,
    /// UI language, BCP-47: "ru-RU".
    pub ui_language: String,
}

/// Known folders from SPEC-02 §3.1 that come from the Known Folders API.
///
/// Serialized by token name (`"DOCUMENTS"`, `"SAVED_GAMES"`), so `known_folders`
/// keys match template tokens.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Type,
)]
pub enum KnownFolder {
    /// `{HOME}`, `FOLDERID_Profile`.
    #[serde(rename = "HOME")]
    Home,
    /// `{APPDATA}`, `FOLDERID_RoamingAppData`.
    #[serde(rename = "APPDATA")]
    AppData,
    /// `{LOCALAPPDATA}`, `FOLDERID_LocalAppData`.
    #[serde(rename = "LOCALAPPDATA")]
    LocalAppData,
    /// `{LOCALLOW}`, `FOLDERID_LocalAppDataLow`.
    #[serde(rename = "LOCALLOW")]
    LocalLow,
    /// `{DOCUMENTS}`, `FOLDERID_Documents` (follows redirection to OneDrive).
    #[serde(rename = "DOCUMENTS")]
    Documents,
    /// `{DESKTOP}`, `FOLDERID_Desktop`.
    #[serde(rename = "DESKTOP")]
    Desktop,
    /// `{PICTURES}`, `FOLDERID_Pictures`.
    #[serde(rename = "PICTURES")]
    Pictures,
    /// `{MUSIC}`, `FOLDERID_Music`.
    #[serde(rename = "MUSIC")]
    Music,
    /// `{VIDEOS}`, `FOLDERID_Videos`.
    #[serde(rename = "VIDEOS")]
    Videos,
    /// `{DOWNLOADS}`, `FOLDERID_Downloads`.
    #[serde(rename = "DOWNLOADS")]
    Downloads,
    /// `{SAVED_GAMES}`, `FOLDERID_SavedGames`.
    #[serde(rename = "SAVED_GAMES")]
    SavedGames,
    /// `{PROGRAMDATA}`, `FOLDERID_ProgramData`.
    #[serde(rename = "PROGRAMDATA")]
    ProgramData,
    /// `{PUBLIC}`, `FOLDERID_Public`.
    #[serde(rename = "PUBLIC")]
    Public,
    /// `{WINDIR}`, `FOLDERID_Windows`.
    #[serde(rename = "WINDIR")]
    WinDir,
    /// `{PROGRAMFILES}`, `FOLDERID_ProgramFiles`.
    #[serde(rename = "PROGRAMFILES")]
    ProgramFiles,
    /// `{PROGRAMFILES_X86}`, `FOLDERID_ProgramFilesX86`.
    #[serde(rename = "PROGRAMFILES_X86")]
    ProgramFilesX86,
}

impl KnownFolder {
    /// All known folders, in declaration order.
    pub const ALL: [KnownFolder; 16] = [
        KnownFolder::Home,
        KnownFolder::AppData,
        KnownFolder::LocalAppData,
        KnownFolder::LocalLow,
        KnownFolder::Documents,
        KnownFolder::Desktop,
        KnownFolder::Pictures,
        KnownFolder::Music,
        KnownFolder::Videos,
        KnownFolder::Downloads,
        KnownFolder::SavedGames,
        KnownFolder::ProgramData,
        KnownFolder::Public,
        KnownFolder::WinDir,
        KnownFolder::ProgramFiles,
        KnownFolder::ProgramFilesX86,
    ];

    /// Template token name without braces, e.g. `"SAVED_GAMES"`.
    pub fn token(self) -> &'static str {
        match self {
            KnownFolder::Home => "HOME",
            KnownFolder::AppData => "APPDATA",
            KnownFolder::LocalAppData => "LOCALAPPDATA",
            KnownFolder::LocalLow => "LOCALLOW",
            KnownFolder::Documents => "DOCUMENTS",
            KnownFolder::Desktop => "DESKTOP",
            KnownFolder::Pictures => "PICTURES",
            KnownFolder::Music => "MUSIC",
            KnownFolder::Videos => "VIDEOS",
            KnownFolder::Downloads => "DOWNLOADS",
            KnownFolder::SavedGames => "SAVED_GAMES",
            KnownFolder::ProgramData => "PROGRAMDATA",
            KnownFolder::Public => "PUBLIC",
            KnownFolder::WinDir => "WINDIR",
            KnownFolder::ProgramFiles => "PROGRAMFILES",
            KnownFolder::ProgramFilesX86 => "PROGRAMFILES_X86",
        }
    }

    /// The folder for a token name without braces, e.g. `"DOCUMENTS"`.
    pub fn from_token(token: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.token() == token)
    }
}

/// A logical drive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub struct DriveInfo {
    /// Drive letter, uppercase.
    pub letter: char,
    /// Drive type.
    pub kind: DriveKind,
    /// Storage media (`IOCTL_STORAGE_QUERY_PROPERTY`, SPEC-03 NFR).
    pub media: DriveMedia,
    /// File system: "NTFS", "exFAT", "FAT32".
    pub fs: Option<String>,
    /// Volume label.
    pub label: Option<String>,
    /// Volume serial number, to detect a changed drive letter on restore (SPEC-13).
    pub volume_serial: Option<u32>,
    /// Volume size.
    pub total_bytes: u64,
    /// Free space available to the user.
    pub free_bytes: u64,
}

/// Drive type.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Type,
)]
#[serde(rename_all = "snake_case")]
pub enum DriveKind {
    /// Internal disk.
    Fixed,
    /// USB stick, card reader.
    Removable,
    /// Network share mapped to a letter.
    Network,
    /// Optical drive.
    CdRom,
    /// Anything else.
    Unknown,
}

/// Storage media of a drive.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Type,
)]
#[serde(rename_all = "snake_case")]
pub enum DriveMedia {
    /// No seek penalty.
    Ssd,
    /// Rotational disk.
    Hdd,
    /// Not determined.
    Unknown,
}

/// Root folder of a cloud sync client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub struct CloudRoot {
    /// Sync client.
    pub provider: CloudProvider,
    /// Synced root folder.
    #[serde(with = "crate::serde_util::lossy_path")]
    #[specta(type = String)]
    pub path: PathBuf,
}

/// Cloud sync client.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum CloudProvider {
    /// OneDrive personal account.
    OneDrive,
    /// OneDrive for work or school.
    OneDriveBusiness,
    /// Dropbox.
    Dropbox,
    /// Google Drive.
    GoogleDrive,
    /// Yandex.Disk.
    YandexDisk,
    /// iCloud Drive.
    ICloud,
    /// Another client.
    Other(String),
}

/// An installed game launcher, filled by SPEC-05.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub struct LauncherInfo {
    /// "steam", "epic" ...
    pub id: String,
    /// Launcher root folder.
    #[serde(default, with = "crate::serde_util::lossy_path_opt")]
    #[specta(type = Option<String>)]
    pub root: Option<PathBuf>,
    /// Store accounts known to the launcher.
    pub user_ids: Vec<StoreUser>,
    /// Installed games.
    pub games: Vec<InstalledGame>,
}

/// A store account. Steam: `id` = id3, `alt_id` = id64.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub struct StoreUser {
    /// Main identifier.
    pub id: String,
    /// Alternative identifier.
    pub alt_id: Option<String>,
    /// Display name.
    pub name: Option<String>,
}

/// A game installed through a launcher.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub struct InstalledGame {
    /// Game id in the store.
    pub store_game_id: String,
    /// Display name.
    pub name: String,
    /// Installation folder.
    #[serde(with = "crate::serde_util::lossy_path")]
    #[specta(type = String)]
    pub install_dir: PathBuf,
    /// Size reported by the launcher.
    pub size_bytes: Option<u64>,
    /// Key in the Ludusavi manifest.
    pub manifest_key: Option<String>,
}

/// An installed program from Uninstall keys or MSIX, filled by SPEC-06 §4.4.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub struct InstalledProgram {
    /// Display name.
    pub name: String,
    /// Publisher.
    pub publisher: Option<String>,
    /// Version string.
    pub version: Option<String>,
    /// Installation folder.
    #[serde(default, with = "crate::serde_util::lossy_path_opt")]
    #[specta(type = Option<String>)]
    pub install_location: Option<PathBuf>,
    /// Installation date as stored ("20240131").
    pub install_date: Option<String>,
    /// Estimated size in KiB.
    pub estimated_size_kb: Option<u64>,
    /// Where the program was found.
    pub source: ProgramSource,
    /// Uninstall key name.
    pub uninstall_key: String,
}

/// Where an installed program was found.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Type,
)]
#[serde(rename_all = "snake_case")]
pub enum ProgramSource {
    /// `HKLM\...\Uninstall`.
    Hklm,
    /// `HKLM\...\WOW6432Node\...\Uninstall`.
    Hklm32,
    /// `HKCU\...\Uninstall`.
    Hkcu,
    /// MSIX / Store package.
    Msix,
}
