//! `Environment::fake` layout (SPEC-02 §3.3).

use std::path::{Path, PathBuf};

use super::{DriveInfo, DriveKind, DriveMedia, Environment, KnownFolder, OsInfo};

/// Folder of each known folder relative to the fake root.
fn relative(folder: KnownFolder) -> &'static [&'static str] {
    const HOME: &str = "Users/user";
    match folder {
        KnownFolder::Home => &[HOME],
        KnownFolder::AppData => &[HOME, "AppData", "Roaming"],
        KnownFolder::LocalAppData => &[HOME, "AppData", "Local"],
        KnownFolder::LocalLow => &[HOME, "AppData", "LocalLow"],
        KnownFolder::Documents => &[HOME, "Documents"],
        KnownFolder::Desktop => &[HOME, "Desktop"],
        KnownFolder::Pictures => &[HOME, "Pictures"],
        KnownFolder::Music => &[HOME, "Music"],
        KnownFolder::Videos => &[HOME, "Videos"],
        KnownFolder::Downloads => &[HOME, "Downloads"],
        KnownFolder::SavedGames => &[HOME, "Saved Games"],
        KnownFolder::ProgramData => &["ProgramData"],
        KnownFolder::Public => &["Users", "Public"],
        KnownFolder::WinDir => &["Windows"],
        KnownFolder::ProgramFiles => &["Program Files"],
        KnownFolder::ProgramFilesX86 => &["Program Files (x86)"],
    }
}

fn join(root: &Path, parts: &[&str]) -> PathBuf {
    let mut path = root.to_path_buf();
    for part in parts {
        // "Users/user" is two components on every platform.
        path.extend(part.split('/'));
    }
    path
}

pub(super) fn fake(root: &Path) -> Environment {
    Environment {
        os: OsInfo {
            product: "Windows 11 Pro".to_owned(),
            display_version: Some("24H2".to_owned()),
            build: "26100.2033".to_owned(),
            arch: "x86_64".to_owned(),
            ui_language: "en-US".to_owned(),
        },
        machine_name: "FAKE-PC".to_owned(),
        user_name: "user".to_owned(),
        user_sid: None,
        is_elevated: false,
        known_folders: KnownFolder::ALL
            .into_iter()
            .map(|f| (f, join(root, relative(f))))
            .collect(),
        drives: vec![DriveInfo {
            letter: 'C',
            kind: DriveKind::Fixed,
            media: DriveMedia::Ssd,
            fs: Some("NTFS".to_owned()),
            label: None,
            volume_serial: Some(0x1234_5678),
            total_bytes: 512 * 1024 * 1024 * 1024,
            free_bytes: 128 * 1024 * 1024 * 1024,
        }],
        cloud_roots: Vec::new(),
        launchers: Vec::new(),
        installed_programs: Vec::new(),
        running_processes: Vec::new(),
    }
}
