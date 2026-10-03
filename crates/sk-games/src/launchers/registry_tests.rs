//! GOG, Ubisoft Connect and Battle.net: detectors reading the registry.

use sk_core::env::{InstalledGame, LauncherInfo, StoreUser};
use sk_core::model::RegHive;

use super::battlenet::UNINSTALL_KEYS;
use super::test_support::{s, Setup};
use super::ubisoft::LAUNCHER_KEY;
use super::{gog, BattleNetDetector, GogDetector, LauncherDetector, UbisoftDetector};

fn games(launcher: &LauncherInfo) -> Vec<(&str, &str, String)> {
    launcher
        .games
        .iter()
        .map(|g| (g.store_game_id.as_str(), g.name.as_str(), s(&g.install_dir)))
        .collect()
}

fn gog(setup: &Setup) -> Option<LauncherInfo> {
    GogDetector::with_registry(setup.registry()).detect(&setup.fs, &setup.env)
}

#[test]
fn gog_not_installed_without_games_key() {
    let setup = Setup::new();
    assert_eq!(gog(&setup), None);
    assert_eq!(GogDetector::new().id(), "gog");
}

#[test]
fn gog_games_from_registry() {
    let mut setup = Setup::new();
    let witcher = setup.path("GOG Games/The Witcher 3");
    let nameless = setup.path("GOG Games/Nameless");
    let key = |id: &str| format!(r"{}\{id}", gog::GAMES_KEY);
    setup
        .hklm(&key("1495134320"), "gameName", "The Witcher 3: Wild Hunt")
        .hklm(&key("1495134320"), "path", &format!("{}\\", s(&witcher)))
        .hklm(&key("1207658924"), "path", &s(&nameless))
        .hklm(&key("1207658925"), "gameName", "Uninstalled")
        .hklm(&key("1207658926"), "path", "relative");
    let launcher = gog(&setup).unwrap_or_else(|| panic!("GOG not detected"));
    assert_eq!(launcher.id, "gog");
    assert_eq!(launcher.root, None);
    assert!(launcher.user_ids.is_empty());
    assert_eq!(
        games(&launcher),
        [
            ("1207658924", "Nameless", s(&nameless)),
            ("1495134320", "The Witcher 3: Wild Hunt", s(&witcher)),
        ]
    );

    // An empty Games key: GOG is there, without games.
    let mut empty = Setup::new();
    empty.registry.add_key(RegHive::Hklm, gog::GAMES_KEY);
    assert_eq!(gog(&empty).map(|l| l.games.len()), Some(0));
}

fn ubisoft(setup: &Setup) -> Option<LauncherInfo> {
    UbisoftDetector::with_registry(setup.registry()).detect(&setup.fs, &setup.env)
}

#[test]
fn ubisoft_not_installed() {
    let mut setup = Setup::new();
    // A registered folder that does not exist does not count.
    let missing = setup.path("Ubisoft");
    setup.hklm(LAUNCHER_KEY, "InstallDir", &s(&missing));
    assert_eq!(ubisoft(&setup), None);
    assert_eq!(UbisoftDetector::new().id(), "ubisoft");
}

#[test]
fn ubisoft_root_games_and_savegames_users() {
    let mut setup = Setup::new();
    let root = setup.path("Games/Ubisoft Game Launcher");
    let far_cry = setup.path("Games/Ubisoft Game Launcher/games/Far Cry 5");
    let installs = format!(r"{LAUNCHER_KEY}\Installs");
    setup
        .dir("Games/Ubisoft Game Launcher/savegames/b1c2-guid/635")
        .dir("Games/Ubisoft Game Launcher/savegames/A0-guid")
        .file("Games/Ubisoft Game Launcher/savegames/readme.txt", "x")
        // `/` and a trailing separator, as the launcher writes them.
        .hklm(
            LAUNCHER_KEY,
            "InstallDir",
            &format!("{}/", s(&root).replace('\\', "/")),
        )
        .hklm(
            &format!(r"{installs}\635"),
            "InstallDir",
            &format!("{}/", s(&far_cry).replace('\\', "/")),
        )
        .hklm(&format!(r"{installs}\720"), "Language", "en-US");
    let launcher = ubisoft(&setup).unwrap_or_else(|| panic!("Ubisoft not detected"));
    assert_eq!(launcher.root, Some(root));
    assert_eq!(games(&launcher), [("635", "Far Cry 5", s(&far_cry))]);
    let users: Vec<&str> = launcher.user_ids.iter().map(|u| u.id.as_str()).collect();
    assert_eq!(users, ["A0-guid", "b1c2-guid"]);
    assert_eq!(
        launcher.user_ids[0],
        StoreUser {
            id: "A0-guid".to_owned(),
            alt_id: None,
            name: None
        }
    );
}

#[test]
fn ubisoft_default_root_and_registry_only() {
    let mut setup = Setup::new();
    setup.dir("Program Files (x86)/Ubisoft/Ubisoft Game Launcher");
    let launcher = ubisoft(&setup).unwrap_or_else(|| panic!("Ubisoft not detected"));
    assert_eq!(
        launcher.root,
        Some(setup.path("Program Files (x86)/Ubisoft/Ubisoft Game Launcher"))
    );
    assert!(launcher.games.is_empty() && launcher.user_ids.is_empty());

    let mut registry_only = Setup::new();
    let game = registry_only.path("Ubi/Game");
    registry_only.hklm(
        &format!(r"{LAUNCHER_KEY}\Installs\1"),
        "InstallDir",
        &s(&game),
    );
    let launcher = ubisoft(&registry_only).unwrap_or_else(|| panic!("Ubisoft not detected"));
    assert_eq!(launcher.root, None);
    assert_eq!(games(&launcher), [("1", "Game", s(&game))]);
}

fn battlenet(setup: &Setup) -> Option<LauncherInfo> {
    BattleNetDetector::with_registry(setup.registry()).detect(&setup.fs, &setup.env)
}

#[test]
fn battlenet_not_installed_without_blizzard_entries() {
    let mut setup = Setup::new();
    let other = format!(r"{}\Other", UNINSTALL_KEYS[0]);
    let location = s(&setup.path("Other"));
    setup
        .hklm(&other, "Publisher", "Valve")
        .hklm(&other, "InstallLocation", &location);
    assert_eq!(battlenet(&setup), None);
    assert_eq!(BattleNetDetector::new().id(), "battlenet");
}

#[test]
fn battlenet_games_from_uninstall_entries() {
    let mut setup = Setup::new();
    let launcher_dir = setup.path("Program Files (x86)/Battle.net");
    let overwatch = setup.path("Games/Overwatch");
    let diablo = setup.path("Games/Diablo IV");
    let wow32 = |entry: &str| format!(r"{}\{entry}", UNINSTALL_KEYS[0]);
    let native = |entry: &str| format!(r"{}\{entry}", UNINSTALL_KEYS[1]);
    setup
        .hklm(&wow32("Battle.net"), "Publisher", "Blizzard Entertainment")
        .hklm(&wow32("Battle.net"), "InstallLocation", &s(&launcher_dir))
        .hklm(&wow32("Overwatch"), "Publisher", "Blizzard Entertainment")
        .hklm(&wow32("Overwatch"), "DisplayName", "Overwatch 2")
        .hklm(&wow32("Overwatch"), "InstallLocation", &s(&overwatch))
        .hklm(
            &wow32("Hearthstone"),
            "Publisher",
            "blizzard entertainment, inc.",
        )
        .hklm(&native("Overwatch"), "Publisher", "Blizzard Entertainment")
        .hklm(&native("Overwatch"), "InstallLocation", &s(&diablo))
        .hklm(
            &native("Diablo IV"),
            "Publisher",
            " Blizzard Entertainment ",
        )
        .hklm(&native("Diablo IV"), "InstallLocation", &s(&diablo))
        .hklm(&native("Steam"), "Publisher", "Valve");
    setup
        .registry
        .set_dword(RegHive::Hklm, &wow32("Overwatch"), "EstimatedSize", 2);
    let launcher = battlenet(&setup).unwrap_or_else(|| panic!("Battle.net not detected"));
    assert_eq!(launcher.id, "battlenet");
    assert_eq!(launcher.root, Some(launcher_dir));
    assert_eq!(
        games(&launcher),
        [
            ("Overwatch", "Overwatch 2", s(&overwatch)),
            ("Diablo IV", "Diablo IV", s(&diablo)),
        ]
    );
    assert_eq!(launcher.games[0].size_bytes, Some(2048));
    assert_eq!(launcher.games[1].size_bytes, None);

    // Only a game: detected without a root.
    let mut game_only = Setup::new();
    game_only
        .hklm(&native("Diablo IV"), "Publisher", "Blizzard Entertainment")
        .hklm(&native("Diablo IV"), "InstallLocation", &s(&diablo));
    let launcher = battlenet(&game_only).unwrap_or_else(|| panic!("Battle.net not detected"));
    assert_eq!(launcher.root, None);
    assert_eq!(
        launcher.games,
        [InstalledGame {
            store_game_id: "Diablo IV".to_owned(),
            name: "Diablo IV".to_owned(),
            install_dir: diablo,
            size_bytes: None,
            manifest_key: None,
        }]
    );
}
