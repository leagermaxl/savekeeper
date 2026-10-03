use std::path::{Path, PathBuf};

use sk_core::env::{InstalledGame, LauncherInfo};
use sk_core::model::ScanIssue;

use super::epic::parse_item;
use super::test_support::{json_str, keys, root, s, sample, Setup};
use super::{EpicDetector, LauncherDetector, ISSUE_LAUNCHER_FILE};

const MANIFESTS: &str = "ProgramData/Epic/EpicGamesLauncher/Data/Manifests";

fn item(app: &str, name: &str, dir: &Path) -> String {
    format!(
        "{{ \"AppName\": \"{app}\", \"DisplayName\": \"{name}\", \"InstallLocation\": {}, \
         \"InstallSize\": 42 }}",
        json_str(dir)
    )
}

fn detect(setup: &Setup) -> (Option<LauncherInfo>, Vec<ScanIssue>) {
    EpicDetector::new().detect_with_issues(&setup.fs, &setup.env)
}

#[test]
fn fixture_item() {
    let mut bytes = sample("epic/6A2E6CA8B4E3F2D1C0B9A8F7E6D5C4B3.item");
    let expected_dir = if cfg!(windows) {
        PathBuf::from(r"D:\Epic Games\HollowKnight")
    } else {
        // The fixture path is not absolute here: use a Unix one.
        let text = String::from_utf8_lossy(&bytes).replace(r"D:\\Epic Games", "/games");
        bytes = text.into_bytes();
        PathBuf::from("/games/HollowKnight")
    };
    let game = parse_item(&bytes).flatten();
    assert_eq!(
        game,
        Some(InstalledGame {
            store_game_id: "Pine".to_owned(),
            name: "Hollow Knight".to_owned(),
            install_dir: expected_dir,
            size_bytes: Some(9_123_456_789),
            manifest_key: None,
        })
    );
}

#[test]
fn item_fallbacks_and_invalid_items() {
    let dir = json_str(&root().join("Game"));
    let minimal = format!("\u{feff}{{ \"AppName\": \"Fn\", \"InstallLocation\": {dir} }}");
    let game = parse_item(minimal.as_bytes()).flatten();
    assert_eq!(game.as_ref().map(|g| g.name.as_str()), Some("Fn"));
    assert_eq!(game.and_then(|g| g.size_bytes), None);

    let addon = format!(
        "{{ \"AppName\": \"Dlc\", \"MainGameAppName\": \"Fn\", \"InstallLocation\": {dir} }}"
    );
    assert_eq!(parse_item(addon.as_bytes()), Some(None));

    for invalid in [
        String::from("not json"),
        String::from("[1, 2]"),
        format!("{{ \"InstallLocation\": {dir} }}"),
        format!("{{ \"AppName\": \" \", \"InstallLocation\": {dir} }}"),
        String::from("{ \"AppName\": \"Fn\" }"),
        String::from("{ \"AppName\": \"Fn\", \"InstallLocation\": \"relative\\\\dir\" }"),
        format!("{{ \"AppName\": 5, \"InstallLocation\": {dir} }}"),
    ] {
        assert_eq!(parse_item(invalid.as_bytes()), None, "{invalid}");
    }
}

#[test]
fn not_installed_without_data_folder() {
    let setup = Setup::new();
    assert_eq!(detect(&setup), (None, Vec::new()));
    assert_eq!(EpicDetector::new().id(), "epic");
}

#[test]
fn installed_without_manifests_has_no_games() {
    let mut setup = Setup::new();
    setup.dir("ProgramData/Epic/EpicGamesLauncher");
    let (launcher, issues) = detect(&setup);
    let launcher = launcher.unwrap_or_else(|| panic!("Epic not detected"));
    assert_eq!(launcher.id, "epic");
    assert_eq!(
        launcher.root,
        Some(setup.path("ProgramData/Epic/EpicGamesLauncher"))
    );
    assert!(launcher.games.is_empty() && launcher.user_ids.is_empty());
    assert!(issues.is_empty());
}

#[test]
fn games_by_file_name_one_per_app_with_issues() {
    let mut setup = Setup::new();
    let fortnite = setup.path("Epic Games/Fortnite");
    let rl = setup.path("Epic Games/rocketleague");
    setup
        .file(
            &format!("{MANIFESTS}/B.item"),
            &item("Sugar", "Rocket League®", &rl),
        )
        .file(
            &format!("{MANIFESTS}/a.ITEM"),
            &item("Fortnite", "Fortnite", &fortnite),
        )
        .file(
            &format!("{MANIFESTS}/c.item"),
            &item("Fortnite", "Fortnite copy", &rl),
        )
        .file(&format!("{MANIFESTS}/d.item"), "{ broken")
        .file(&format!("{MANIFESTS}/notes.txt"), "{}")
        .dir(&format!("{MANIFESTS}/Pending.item"));
    let locked = setup.path(&format!("{MANIFESTS}/e.item"));
    setup.fs.set_locked(&s(&locked));

    let (launcher, issues) = detect(&setup);
    let games = launcher.map(|l| l.games).unwrap_or_default();
    let ids: Vec<(&str, &str)> = games
        .iter()
        .map(|g| (g.store_game_id.as_str(), g.name.as_str()))
        .collect();
    assert_eq!(ids, [("Fortnite", "Fortnite"), ("Sugar", "Rocket League®")]);
    assert_eq!(games[0].install_dir, fortnite);
    assert_eq!(games[0].size_bytes, Some(42));
    assert_eq!(
        keys(&issues, "games.epic"),
        [
            (ISSUE_LAUNCHER_FILE, "invalid"),
            (ISSUE_LAUNCHER_FILE, "locked")
        ]
    );
    assert_eq!(
        issues[0].path.as_deref(),
        Some(r"{PROGRAMDATA}\Epic\EpicGamesLauncher\Data\Manifests\d.item")
    );
}
