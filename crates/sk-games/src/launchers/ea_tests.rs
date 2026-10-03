use sk_core::env::{InstalledGame, LauncherInfo};
use sk_core::model::ScanIssue;

use super::ea::{parse_install_data, GAMES_KEY};
use super::test_support::{json_str, keys, root, s, Setup};
use super::{EaDetector, LauncherDetector, ISSUE_LAUNCHER_FILE};

const INSTALL_DATA: &str = "ProgramData/EA Desktop/InstallData";

fn detect(setup: &Setup) -> (Option<LauncherInfo>, Vec<ScanIssue>) {
    EaDetector::with_registry(setup.registry()).detect_with_issues(&setup.fs, &setup.env)
}

fn ids(launcher: &LauncherInfo) -> Vec<(&str, &str, String)> {
    launcher
        .games
        .iter()
        .map(|g| (g.store_game_id.as_str(), g.name.as_str(), s(&g.install_dir)))
        .collect()
}

#[test]
fn install_data_fields_and_fallbacks() {
    let dir = root().join("EA Games").join("Battlefield 2042");
    let full = format!(
        "{{ \"softwareId\": \"Origin.SFT.50.0001\", \"DisplayName\": \"Battlefield™ 2042\", \
         \"InstallLocation\": {} }}",
        json_str(&dir)
    );
    assert_eq!(
        parse_install_data(full.as_bytes(), "Battlefield 2042"),
        Some(Some(InstalledGame {
            store_game_id: "Origin.SFT.50.0001".to_owned(),
            name: "Battlefield™ 2042".to_owned(),
            install_dir: dir.clone(),
            size_bytes: None,
            manifest_key: None,
        }))
    );

    // Without name and id fields: the folder name, else the install folder name.
    let bare = format!("\u{feff}{{ \"baseInstallPath\": {} }}", json_str(&dir));
    let game = parse_install_data(bare.as_bytes(), "bf2042").flatten();
    let game = game.unwrap_or_else(|| panic!("no game"));
    assert_eq!(
        (game.store_game_id.as_str(), game.name.as_str()),
        ("bf2042", "bf2042")
    );
    let game = parse_install_data(bare.as_bytes(), " ").flatten();
    assert_eq!(game.map(|g| g.name), Some("Battlefield 2042".to_owned()));

    // Not a game record, and not JSON.
    assert_eq!(
        parse_install_data(b"{ \"locale\": \"en_US\" }", "x"),
        Some(None)
    );
    assert_eq!(
        parse_install_data(b"{ \"installLocation\": \"relative\" }", "x"),
        Some(None)
    );
    assert_eq!(parse_install_data(b"[]", "x"), None);
    assert_eq!(parse_install_data(b"{", "x"), None);
}

#[test]
fn not_installed() {
    let setup = Setup::new();
    assert_eq!(detect(&setup), (None, Vec::new()));
    assert_eq!(EaDetector::new().id(), "ea");
}

#[test]
fn json_games_then_registry_games() {
    let mut setup = Setup::new();
    let bf = setup.path("EA Games/Battlefield 2042");
    let sims = setup.path("EA Games/The Sims 4");
    let me = setup.path("Origin Games/Mass Effect");
    let game_json = |dir: &std::path::Path| format!("{{ \"installLocation\": {} }}", json_str(dir));
    setup
        .file(
            &format!("{INSTALL_DATA}/Battlefield 2042/data.json"),
            &game_json(&bf),
        )
        .file(
            &format!("{INSTALL_DATA}/Battlefield 2042/other.JSON"),
            &game_json(&bf),
        )
        .file(&format!("{INSTALL_DATA}/Battlefield 2042/broken.json"), "{")
        .file(&format!("{INSTALL_DATA}/Battlefield 2042/notes.txt"), "{")
        .file(&format!("{INSTALL_DATA}/stray.json"), &game_json(&me))
        .file(
            &format!("{INSTALL_DATA}/Sims/state.json"),
            "{ \"locale\": \"en\" }",
        )
        .hklm(
            &format!(r"{GAMES_KEY}\The Sims 4"),
            "Install Dir",
            &format!("{}\\", s(&sims)),
        )
        .hklm(&format!(r"{GAMES_KEY}\Mass Effect"), "Install Dir", &s(&me))
        .hklm(
            &format!(r"{GAMES_KEY}\Mass Effect"),
            "DisplayName",
            "Mass Effect™",
        )
        .hklm(&format!(r"{GAMES_KEY}\BF"), "Install Dir", &s(&bf))
        .hklm(
            &format!(r"{GAMES_KEY}\EA Desktop"),
            "InstallLocation",
            &s(&bf),
        );
    let (launcher, issues) = detect(&setup);
    let launcher = launcher.unwrap_or_else(|| panic!("EA not detected"));
    assert_eq!(launcher.id, "ea");
    assert_eq!(launcher.root, Some(setup.path("ProgramData/EA Desktop")));
    assert_eq!(
        ids(&launcher),
        [
            ("Battlefield 2042", "Battlefield 2042", s(&bf)),
            ("Mass Effect", "Mass Effect™", s(&me)),
            ("The Sims 4", "The Sims 4", s(&sims)),
        ]
    );
    assert_eq!(
        keys(&issues, "games.ea"),
        [(ISSUE_LAUNCHER_FILE, "invalid")]
    );
    assert_eq!(
        issues[0].path.as_deref(),
        Some(r"{PROGRAMDATA}\EA Desktop\InstallData\Battlefield 2042\broken.json")
    );
}

#[test]
fn registry_only_has_no_root() {
    let mut setup = Setup::new();
    let me = setup.path("Origin Games/Mass Effect");
    setup.hklm(&format!(r"{GAMES_KEY}\Mass Effect"), "Install Dir", &s(&me));
    let (launcher, issues) = detect(&setup);
    let launcher = launcher.unwrap_or_else(|| panic!("EA not detected"));
    assert_eq!(launcher.root, None);
    assert_eq!(ids(&launcher), [("Mass Effect", "Mass Effect", s(&me))]);
    assert!(issues.is_empty());

    // A key without `Install Dir` is not a game.
    let mut no_games = Setup::new();
    no_games.hklm(&format!(r"{GAMES_KEY}\EA Desktop"), "Version", "13");
    assert_eq!(detect(&no_games).0, None);
}

#[test]
fn unreadable_install_data_is_an_issue() {
    let mut setup = Setup::new();
    let install_data = setup.path(INSTALL_DATA);
    setup.fs.set_locked(&s(&install_data));
    let (launcher, issues) = detect(&setup);
    assert_eq!(launcher.map(|l| l.games.len()), Some(0));
    // A locked folder cannot be listed.
    assert_eq!(keys(&issues, "games.ea"), [(ISSUE_LAUNCHER_FILE, "io")]);
}
