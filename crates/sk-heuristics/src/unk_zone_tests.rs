//! Tests of the H-UNK zones (SPEC-07 §4.2.1, §4.2.3): exclusions, Store
//! packages, `{LOCALLOW}`, `{PROGRAMDATA}`, the profile root, saved games.

use std::sync::Arc;

use sk_core::env::{CloudProvider, CloudRoot, InstalledProgram, KnownFolder, ProgramSource};
use sk_core::fs::ReparseKind;
use sk_core::model::{Category, Target};
use sk_scan::{ExcludeSet, MemFs};

use super::tests::{abs, at, claimed, excludes_of, file, includes_of, one, Setup, MIB};

#[test]
fn store_packages() {
    let mut fs = MemFs::new();
    let la = KnownFolder::LocalAppData;
    let game = "Packages/Microsoft.SunriseBaseGame_8wekyb3d8bbwe";
    file(
        &mut fs,
        la,
        &format!("{game}/SystemAppData/wgs/c/1.dat"),
        100,
        "-1d",
    );
    file(&mut fs, la, &format!("{game}/LocalCache/x.bin"), 100, "-1d");
    let app = "Packages/Vendor.App_abcdefghijklm";
    file(&mut fs, la, &format!("{app}/LocalState/db.bin"), 100, "-1d");
    let cache_only = "Packages/Vendor.Cache_abcdefghijklm";
    file(
        &mut fs,
        la,
        &format!("{cache_only}/LocalCache/x.bin"),
        100,
        "-1d",
    );
    let system = "Packages/Microsoft.Windows.Photos_8wekyb3d8bbwe";
    file(
        &mut fs,
        la,
        &format!("{system}/LocalState/x.bin"),
        100,
        "-1d",
    );
    let system2 = "Packages/MicrosoftWindows.Client.CBS_cw5n1h2txyewy";
    file(
        &mut fs,
        la,
        &format!("{system2}/LocalState/x.bin"),
        100,
        "-1d",
    );
    let out = Setup::new(fs).run();

    let saves = one(&out, &abs(la, game));
    assert_eq!(saves.category, Category::GameSave);
    assert_eq!(saves.evidence[0].message_key, "heuristic.uwp_saves");
    assert_eq!(
        includes_of(saves),
        [
            "LocalState/**",
            "RoamingState/**",
            "Settings/**",
            "SystemAppData/wgs/**"
        ]
    );
    assert_eq!(excludes_of(saves), ["LocalCache", "TempState", "AC"]);
    // AppData with 0.5: a candidate for the LLM.
    let data = one(&out, &abs(la, app));
    assert_eq!(data.category, Category::Unknown);
    assert_eq!(data.evidence[0].message_key, "heuristic.uwp_package");
    assert_eq!(data.evidence[0].confidence, 0.5);
    assert_eq!(out.unknown.len(), 1);
    assert!(out.unknown[0].path.as_str().starts_with("{PACKAGE:"));
    assert!(at(&out, &abs(la, cache_only)).is_empty());
    assert!(claimed(&out, &abs(la, cache_only)));
    for skipped in [system, system2, "Packages"] {
        assert!(at(&out, &abs(la, skipped)).is_empty(), "{skipped}");
        assert!(!claimed(&out, &abs(la, skipped)), "{skipped}");
    }
    assert_eq!(out.findings.len(), 2);
}

#[test]
fn excluded_folders_are_not_candidates() {
    let mut fs = MemFs::new();
    let la = KnownFolder::LocalAppData;
    file(&mut fs, la, "Temp/x.tmp", 100, "-1d");
    file(&mut fs, la, "CrashDumps/a.dmp", 100, "-1d");
    file(
        &mut fs,
        la,
        "Microsoft/Windows/INetCache/big.dat",
        10 * MIB,
        "-1d",
    );
    file(&mut fs, la, "Microsoft/Windows/Settings/s.json", 300, "-1d");
    file(
        &mut fs,
        KnownFolder::AppData,
        "Private/notes.json",
        100,
        "-1d",
    );
    let mut setup = Setup::new(fs);
    setup.excludes = Arc::new(ExcludeSet::with_user(&setup.env, &["Private".to_owned()]).unwrap());
    let out = setup.run();

    let windows = one(&out, &abs(la, "Microsoft/Windows"));
    // INetCache is excluded from the summary.
    assert_eq!(windows.evidence[0].message_args["bytes"], "300");
    assert_eq!(out.findings.len(), 1, "{:?}", out.findings);
}

#[test]
fn locallow_candidates_are_company_and_product() {
    let mut fs = MemFs::new();
    file(
        &mut fs,
        KnownFolder::LocalLow,
        "Studio/Game/Player.log",
        100,
        "-1d",
    );
    file(
        &mut fs,
        KnownFolder::LocalLow,
        "Studio/Game/save.dat",
        100,
        "-1d",
    );
    file(
        &mut fs,
        KnownFolder::LocalLow,
        "Studio/loose.txt",
        100,
        "-1d",
    );
    let out = Setup::new(fs).run();
    let game = one(&out, &abs(KnownFolder::LocalLow, "Studio/Game"));
    assert_eq!(game.title, "Studio / Game");
    assert_eq!(game.category, Category::GameSave);
    assert!(game.tags.contains(&"unity".to_owned()));
    assert_eq!(out.findings.len(), 1);
}

#[test]
fn program_data_needs_recent_files_without_programs() {
    let mut fs = MemFs::new();
    let pd = KnownFolder::ProgramData;
    file(&mut fs, pd, "Recent/settings.json", 100, "-10d");
    file(&mut fs, pd, "Old/settings.json", 100, "-400d");
    file(&mut fs, pd, "Tools/a.exe", MIB, "-1d");
    file(&mut fs, pd, "Installed/settings.json", 100, "-1d");
    let mut setup = Setup::new(fs);
    setup.env.installed_programs = vec![InstalledProgram {
        name: "Installed".to_owned(),
        publisher: None,
        version: None,
        install_location: Some(abs(pd, "Installed")),
        install_date: None,
        estimated_size_kb: None,
        source: ProgramSource::Hklm,
        uninstall_key: "Installed".to_owned(),
    }];
    let out = setup.run();
    one(&out, &abs(pd, "Recent"));
    assert_eq!(out.findings.len(), 1, "{:?}", out.findings);
    assert_eq!(out.claimed.len(), 1);
}

#[test]
fn profile_root_dot_entries_and_custom_folders() {
    let mut fs = MemFs::new();
    let home = KnownFolder::Home;
    file(&mut fs, home, ".cargo/config.toml", 100, "-1d");
    file(&mut fs, home, ".cargo/credentials.toml", 100, "-1d");
    file(&mut fs, home, ".gitconfig", 200, "-1d");
    file(&mut fs, home, ".big_history", 11 * MIB, "-1d");
    file(&mut fs, home, "NTUSER.DAT", 100, "-1d");
    file(&mut fs, home, "projects/a.bin", 100, "-1d");
    file(
        &mut fs,
        KnownFolder::Documents,
        "My Games/x.bin",
        100,
        "-1d",
    );
    fs.add_reparse(
        abs(home, "Application Data").to_str().unwrap(),
        ReparseKind::Junction,
    );
    let out = Setup::new(fs).run();

    let cargo = one(&out, &abs(home, ".cargo"));
    assert_eq!(cargo.category, Category::DevEnvironment);
    assert_eq!(cargo.evidence[0].message_key, "heuristic.dotfolder");
    let gitconfig = one(&out, &abs(home, ".gitconfig"));
    assert!(matches!(gitconfig.target, Target::File { .. }));
    assert_eq!(gitconfig.category, Category::DevEnvironment);
    assert_eq!(gitconfig.evidence[0].message_key, "heuristic.dotfile");
    let big = one(&out, &abs(home, ".big_history"));
    assert_eq!(big.category, Category::Unknown);
    one(&out, &abs(home, "projects"));
    // Documents is a zone of its own and not a candidate of the profile root.
    one(&out, &abs(KnownFolder::Documents, "My Games"));
    for none in ["NTUSER.DAT", "Application Data", "AppData", "Documents"] {
        assert!(at(&out, &abs(home, none)).is_empty(), "{none}");
    }
    assert_eq!(out.findings.len(), 5, "{:?}", out.findings);
    // `projects` and `My Games`; files have no summary for the LLM.
    assert_eq!(out.unknown.len(), 2);
}

#[test]
fn saved_games_are_game_saves() {
    let mut fs = MemFs::new();
    file(
        &mut fs,
        KnownFolder::SavedGames,
        "Frontier/a.bin",
        100,
        "-1d",
    );
    let out = Setup::new(fs).run();
    let f = one(&out, &abs(KnownFolder::SavedGames, "Frontier"));
    assert_eq!(f.category, Category::GameSave);
    assert_eq!(f.evidence[0].message_key, "heuristic.saved_games");
}

/// A cloud root in the profile is an ordinary candidate, tagged
/// `cloud-synced` by its marker (SPEC-07 §4.2.1, §5).
#[test]
fn cloud_roots_in_the_profile_are_candidates() {
    let mut fs = MemFs::new();
    file(&mut fs, KnownFolder::Home, "Dropbox/notes.bin", 100, "-1d");
    let dropbox = abs(KnownFolder::Home, "Dropbox");
    let mut setup = Setup::new(fs);
    setup.env.cloud_roots = vec![CloudRoot {
        provider: CloudProvider::Dropbox,
        path: dropbox.clone(),
    }];
    let out = setup.run();
    let f = one(&out, &dropbox);
    assert_eq!(f.tags, ["cloud-synced"]);
    assert!(claimed(&out, &dropbox));
}
