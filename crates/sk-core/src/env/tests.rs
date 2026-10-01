use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::*;

fn fake_root() -> PathBuf {
    PathBuf::from(if cfg!(windows) { r"C:\fake" } else { "/fake" })
}

#[test]
fn fake_has_all_known_folders_under_root() {
    let root = fake_root();
    let env = Environment::fake(&root);
    assert_eq!(env.known_folders.len(), KnownFolder::ALL.len());
    for (folder, path) in &env.known_folders {
        assert!(path.starts_with(&root), "{folder:?}: {}", path.display());
    }
    let home = root.join("Users").join("user");
    let expect = |folder, rel: &[&str]| {
        let mut path = home.clone();
        path.extend(rel);
        assert_eq!(env.known_folder(folder), Some(path.as_path()), "{folder:?}");
    };
    expect(KnownFolder::Home, &[]);
    expect(KnownFolder::AppData, &["AppData", "Roaming"]);
    expect(KnownFolder::LocalAppData, &["AppData", "Local"]);
    expect(KnownFolder::LocalLow, &["AppData", "LocalLow"]);
    expect(KnownFolder::Documents, &["Documents"]);
    expect(KnownFolder::SavedGames, &["Saved Games"]);
    assert_eq!(
        env.known_folder(KnownFolder::ProgramFilesX86),
        Some(root.join("Program Files (x86)").as_path())
    );
    assert_eq!(
        env.known_folder(KnownFolder::Public),
        Some(root.join("Users").join("Public").as_path())
    );
    assert_eq!(env.user_name, "user");
    assert_eq!(env.drives.len(), 1);
    assert!(env.cloud_roots.is_empty() && env.running_processes.is_empty());
}

#[test]
fn fake_does_not_touch_the_disk() {
    let root = fake_root().join("does-not-exist");
    let env = Environment::fake(&root);
    assert!(!root.exists());
    assert!(env.known_folder(KnownFolder::Home).is_some());
}

#[test]
fn known_folder_tokens() {
    let tokens: BTreeSet<_> = KnownFolder::ALL.iter().map(|f| f.token()).collect();
    assert_eq!(
        tokens.len(),
        KnownFolder::ALL.len(),
        "tokens must be unique"
    );
    for folder in KnownFolder::ALL {
        assert_eq!(KnownFolder::from_token(folder.token()), Some(folder));
        assert_eq!(
            serde_json::to_value(folder).unwrap(),
            serde_json::Value::String(folder.token().to_owned())
        );
    }
    assert_eq!(KnownFolder::SavedGames.token(), "SAVED_GAMES");
    assert_eq!(KnownFolder::ProgramFilesX86.token(), "PROGRAMFILES_X86");
    assert_eq!(KnownFolder::from_token("ONEDRIVE"), None);
    assert_eq!(KnownFolder::from_token("documents"), None);
}

#[test]
fn environment_round_trips_with_token_keys() {
    let mut env = Environment::fake(&fake_root());
    env.cloud_roots.push(CloudRoot {
        provider: CloudProvider::Other("Nextcloud".to_owned()),
        path: fake_root().join("Nextcloud"),
    });
    env.launchers.push(LauncherInfo {
        id: "steam".to_owned(),
        root: Some(fake_root().join("Steam")),
        user_ids: vec![StoreUser {
            id: "12345678".to_owned(),
            alt_id: Some("76561197972611406".to_owned()),
            name: None,
        }],
        games: vec![InstalledGame {
            store_game_id: "1245620".to_owned(),
            name: "ELDEN RING".to_owned(),
            install_dir: fake_root().join("Steam").join("ELDEN RING"),
            size_bytes: Some(50_000_000_000),
            manifest_key: Some("ELDEN RING".to_owned()),
        }],
    });
    env.installed_programs.push(InstalledProgram {
        name: "7-Zip".to_owned(),
        publisher: Some("Igor Pavlov".to_owned()),
        version: Some("24.08".to_owned()),
        install_location: None,
        install_date: Some("20240131".to_owned()),
        estimated_size_kb: Some(5_800),
        source: ProgramSource::Hklm,
        uninstall_key: "7-Zip".to_owned(),
    });

    let json = serde_json::to_value(&env).unwrap();
    let keys: BTreeSet<_> = json["known_folders"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    let tokens: BTreeSet<_> = KnownFolder::ALL
        .iter()
        .map(|f| f.token().to_owned())
        .collect();
    assert_eq!(keys, tokens);
    assert_eq!(json["drives"][0]["letter"], "C");
    assert_eq!(json["drives"][0]["kind"], "fixed");
    assert_eq!(
        json["cloud_roots"][0]["provider"],
        serde_json::json!({ "other": "Nextcloud" })
    );

    let back: Environment = serde_json::from_value(json).unwrap();
    assert_eq!(back, env);
}

#[test]
fn detect_finds_home() {
    let env = Environment::detect().unwrap();
    let home = env.known_folder(KnownFolder::Home).unwrap();
    assert!(home.is_dir(), "{}", home.display());
}

#[cfg(not(windows))]
#[test]
fn detect_stub_uses_home_variable() {
    let env = Environment::detect().unwrap();
    let home = std::env::var("HOME").unwrap();
    assert_eq!(env.known_folder(KnownFolder::Home), Some(Path::new(&home)));
}

#[cfg(windows)]
mod windows {
    use super::*;
    use crate::path::starts_with_ci;

    fn var(name: &str) -> PathBuf {
        PathBuf::from(std::env::var_os(name).unwrap())
    }

    fn assert_same(env: &Environment, folder: KnownFolder, expected: &Path) {
        let actual = env.known_folder(folder).unwrap();
        assert!(
            crate::path::eq_ci(actual, expected),
            "{folder:?}: {} != {}",
            actual.display(),
            expected.display()
        );
    }

    #[test]
    fn detect_returns_mandatory_known_folders() {
        let env = Environment::detect().unwrap();
        for folder in [
            KnownFolder::Home,
            KnownFolder::AppData,
            KnownFolder::LocalAppData,
            KnownFolder::LocalLow,
            KnownFolder::Documents,
            KnownFolder::ProgramData,
            KnownFolder::WinDir,
            KnownFolder::ProgramFiles,
        ] {
            assert!(env.known_folder(folder).is_some(), "{folder:?} missing");
        }
        // Independent sources for the same folders.
        assert_same(&env, KnownFolder::Home, &var("USERPROFILE"));
        assert_same(&env, KnownFolder::AppData, &var("APPDATA"));
        assert_same(&env, KnownFolder::LocalAppData, &var("LOCALAPPDATA"));
        assert_same(&env, KnownFolder::ProgramData, &var("ProgramData"));
        assert_same(&env, KnownFolder::WinDir, &var("SystemRoot"));
        assert_same(&env, KnownFolder::ProgramFiles, &var("ProgramW6432"));
        let local_low = env.known_folder(KnownFolder::LocalLow).unwrap();
        assert!(starts_with_ci(local_low, &var("USERPROFILE")));
    }

    #[test]
    fn documents_match_known_folder_api() {
        let env = Environment::detect().unwrap();
        let api = crate::win::known_folders::path(KnownFolder::Documents).unwrap();
        assert_eq!(
            env.known_folder(KnownFolder::Documents),
            Some(api.as_path())
        );
    }

    #[test]
    fn detect_reads_user_os_and_machine() {
        let env = Environment::detect().unwrap();
        assert!(env
            .user_name
            .eq_ignore_ascii_case(&std::env::var("USERNAME").unwrap()));
        assert!(env
            .machine_name
            .eq_ignore_ascii_case(&std::env::var("COMPUTERNAME").unwrap()));
        assert!(env
            .user_sid
            .as_deref()
            .is_some_and(|s| s.starts_with("S-1-5-")));
        assert!(env.os.product.starts_with("Windows"), "{}", env.os.product);
        let (build, ubr) = env.os.build.split_once('.').unwrap();
        assert!(build.parse::<u32>().unwrap() >= 10240 && ubr.parse::<u32>().is_ok());
        assert!(["x86_64", "aarch64", "x86"].contains(&env.os.arch.as_str()));
        assert!(env.os.ui_language.contains('-'), "{}", env.os.ui_language);
    }

    #[test]
    fn detect_lists_system_drive() {
        let env = Environment::detect().unwrap();
        let system = var("SystemDrive").to_string_lossy().chars().next().unwrap();
        let drive = env.drives.iter().find(|d| d.letter == system).unwrap();
        assert_eq!(drive.kind, DriveKind::Fixed);
        assert!(drive.fs.is_some() && drive.volume_serial.is_some());
        assert!(drive.total_bytes > 0 && drive.free_bytes <= drive.total_bytes);
    }

    #[test]
    fn detect_snapshots_running_processes() {
        let env = Environment::detect().unwrap();
        let exe = std::env::current_exe().unwrap();
        let name = exe.file_name().unwrap().to_string_lossy().to_lowercase();
        assert!(
            env.running_processes.contains(&name),
            "{name} not in snapshot"
        );
        let mut sorted = env.running_processes.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted, env.running_processes);
    }

    #[test]
    fn detect_lists_store_packages() {
        let env = Environment::detect().unwrap();
        let packages = env
            .known_folder(KnownFolder::LocalAppData)
            .unwrap()
            .join("Packages");
        let mut sorted = env.store_packages.clone();
        sorted.sort();
        assert_eq!(sorted, env.store_packages);
        for name in &env.store_packages {
            assert!(packages.join(name).is_dir(), "{name}");
        }
    }

    #[test]
    fn onedrive_roots_exist() {
        let env = Environment::detect().unwrap();
        for root in &env.cloud_roots {
            assert!(matches!(
                root.provider,
                CloudProvider::OneDrive | CloudProvider::OneDriveBusiness
            ));
            assert!(root.path.is_dir());
        }
    }

    #[test]
    fn windows_11_product_name() {
        use crate::win::system::product_name;
        assert_eq!(product_name("Windows 10 Pro", 26100), "Windows 11 Pro");
        assert_eq!(product_name("Windows 10 Pro", 19045), "Windows 10 Pro");
        assert_eq!(
            product_name("Windows Server 2022", 20348),
            "Windows Server 2022"
        );
    }
}
