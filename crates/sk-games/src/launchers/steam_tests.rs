use std::path::{Path, PathBuf};
use std::sync::Arc;

use sk_core::env::{Environment, LauncherInfo};
use sk_core::model::{IssueSeverity, RegHive, ScanIssue};
use sk_core::template::{PathTemplate, ResolveContext};
use sk_scan::MemFs;

use super::*;
use crate::registry::MemRegistry;

fn root() -> PathBuf {
    PathBuf::from(if cfg!(windows) { r"C:\fake" } else { "/fake" })
}

fn join(base: &Path, rel: &str) -> PathBuf {
    rel.split('/').fold(base.to_path_buf(), |p, c| p.join(c))
}

fn s(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn sample(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/samples/steam")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// `{PROGRAMFILES_X86}\Steam` of the fake environment.
fn default_steam() -> PathBuf {
    join(&root(), "Program Files (x86)/Steam")
}

/// A VDF string literal of a path (backslashes escaped).
fn vdf_str(path: &Path) -> String {
    s(path).replace('\\', r"\\")
}

fn acf(app_id: &str, name: &str, install_dir: &str, size: &str) -> String {
    format!(
        "\"AppState\"\n{{\n\t\"appid\"\t\t\"{app_id}\"\n\t\"name\"\t\t\"{name}\"\n\
         \t\"installdir\"\t\t\"{install_dir}\"\n\t\"SizeOnDisk\"\t\t\"{size}\"\n}}\n"
    )
}

fn libraryfolders(paths: &[&Path]) -> String {
    let mut text = String::from("\"libraryfolders\"\n{\n");
    for (i, path) in paths.iter().enumerate() {
        text.push_str(&format!(
            "\t\"{i}\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n",
            vdf_str(path)
        ));
    }
    text.push_str("}\n");
    text
}

struct Setup {
    env: Environment,
    fs: MemFs,
    registry: MemRegistry,
}

impl Setup {
    fn new() -> Self {
        Self {
            env: Environment::fake(&root()),
            fs: MemFs::new(),
            registry: MemRegistry::new(),
        }
    }

    fn file(&mut self, path: &Path, content: &str) -> &mut Self {
        self.fs.add_file(
            &s(path),
            content.len() as u64,
            "-1d",
            Some(content.as_bytes()),
        );
        self
    }

    fn dir(&mut self, path: &Path) -> &mut Self {
        self.fs.add_dir(&s(path));
        self
    }

    fn steam_path(&mut self, value: &str) -> &mut Self {
        self.registry
            .set_string(RegHive::Hkcu, r"Software\Valve\Steam", "SteamPath", value);
        self
    }

    fn detect(&self) -> (Option<LauncherInfo>, Vec<ScanIssue>) {
        SteamDetector::with_registry(Arc::new(self.registry.clone()))
            .detect_with_issues(&self.fs, &self.env)
    }

    fn launcher(&self) -> LauncherInfo {
        self.detect()
            .0
            .unwrap_or_else(|| panic!("Steam not detected"))
    }
}

fn keys(issues: &[ScanIssue]) -> Vec<(&str, &str)> {
    issues
        .iter()
        .map(|i| {
            assert_eq!(i.severity, IssueSeverity::Info);
            assert_eq!(i.source, "games.steam");
            let reason = i.message_args.get("reason").map_or("", String::as_str);
            (i.message_key.as_str(), reason)
        })
        .collect()
}

#[test]
fn fixture_libraryfolders() {
    let paths = parse_libraryfolders(&sample("libraryfolders.vdf"));
    assert_eq!(
        paths,
        Some(vec![
            r"C:\Program Files (x86)\Steam".to_owned(),
            r"D:\SteamLibrary".to_owned()
        ])
    );
}

#[test]
fn old_libraryfolders_format_and_order() {
    let text = "\"LibraryFolders\"\n{\n\t\"TimeNextStatsReport\"\t\"1\"\n\
                \t\"10\"\t\"F:\\\\Ten\"\n\t\"2\"\t\"E:\\\\Two\"\n}\n";
    assert_eq!(
        parse_libraryfolders(text),
        Some(vec![r"E:\Two".to_owned(), r"F:\Ten".to_owned()])
    );
    assert_eq!(parse_libraryfolders("\"libraryfolders\" {"), None);
}

#[test]
fn fixture_appmanifest() {
    assert_eq!(
        parse_appmanifest(&sample("appmanifest_1245620.acf"), 1),
        Some(AppManifest {
            app_id: "1245620".to_owned(),
            name: "ELDEN RING".to_owned(),
            install_dir: "ELDEN RING".to_owned(),
            size_on_disk: Some(51_352_018_534),
        })
    );
}

#[test]
fn appmanifest_fallbacks_and_unsafe_install_dirs() {
    let text = "\"AppState\" { \"installdir\" \"Game\" \"SizeOnDisk\" \"x\" }";
    assert_eq!(
        parse_appmanifest(text, 70),
        Some(AppManifest {
            app_id: "70".to_owned(),
            name: "Game".to_owned(),
            install_dir: "Game".to_owned(),
            size_on_disk: None,
        })
    );
    for dir in ["", "..", ".", r"..\\..\\Windows", "a/b", "C:"] {
        let text = format!("\"AppState\" {{ \"appid\" \"1\" \"installdir\" \"{dir}\" }}");
        assert_eq!(parse_appmanifest(&text, 1), None, "{dir:?}");
    }
    assert_eq!(
        parse_appmanifest("\"AppState\" { \"appid\" \"1\" }", 1),
        None
    );
}

#[test]
fn fixture_loginusers_and_id_conversion() {
    assert_eq!(
        parse_loginusers(&sample("loginusers.vdf")),
        Some(vec![(12_345_678, "Fixture Gamer".to_owned())])
    );
    assert_eq!(id3_of(76_561_197_972_611_406), Some(12_345_678));
    assert_eq!(id3_of(STEAM_ID64_BASE), Some(0));
    assert_eq!(id3_of(STEAM_ID64_BASE - 1), None);
    assert_eq!(id3_of(STEAM_ID64_BASE + (1 << 32)), None);
}

#[test]
fn not_installed() {
    let mut setup = Setup::new();
    assert_eq!(setup.detect(), (None, Vec::new()));
    // A registry entry of a removed Steam folder is not enough.
    setup.steam_path(&s(&join(&root(), "Gone/Steam")));
    assert_eq!(setup.detect(), (None, Vec::new()));
    assert_eq!(SteamDetector::new().id(), "steam");
}

#[test]
fn root_from_registry_then_default_folder() {
    let mut setup = Setup::new();
    setup.dir(&default_steam());
    assert_eq!(setup.launcher().root, Some(default_steam()));

    // The registry path is preferred, written with `/` as Steam does.
    let custom = join(&root(), "Games/Steam");
    setup.dir(&custom);
    setup.steam_path(&format!("{}/", s(&custom).replace('\\', "/")));
    assert_eq!(setup.launcher().root, Some(custom));

    // A registry path that is not a folder falls back to the default.
    setup.steam_path(&s(&join(&root(), "Gone/Steam")));
    assert_eq!(setup.launcher().root, Some(default_steam()));
}

#[test]
fn full_detection() {
    let steam = default_steam();
    let second = join(&root(), "Games/SteamLibrary");
    let mut setup = Setup::new();
    setup
        .file(
            &join(&steam, "steamapps/libraryfolders.vdf"),
            &libraryfolders(&[&join(&root(), "PROGRAM FILES (X86)/steam"), &second]),
        )
        .file(
            &join(&steam, "steamapps/appmanifest_1245620.acf"),
            &acf("1245620", "ELDEN RING", "ELDEN RING", "51352018534"),
        )
        .file(&join(&steam, "steamapps/appmanifest_9.acf"), "broken {")
        .file(&join(&steam, "steamapps/appmanifest_x.acf"), "ignored")
        .file(
            &join(&second, "steamapps/appmanifest_367520.acf"),
            &acf("367520", "Hollow Knight", "Hollow Knight", "9184628461"),
        )
        // The same game listed twice keeps the first library.
        .file(
            &join(&second, "steamapps/appmanifest_1245620.acf"),
            &acf("1245620", "ELDEN RING", "ELDEN RING", "1"),
        )
        .file(
            &join(&steam, "config/loginusers.vdf"),
            &sample("loginusers.vdf"),
        );
    for user in ["12345678", "222", "abc", "0123"] {
        setup.dir(&join(&steam, &format!("userdata/{user}/config")));
    }
    setup.file(&join(&steam, "userdata/333"), "not a folder");

    let (launcher, issues) = setup.detect();
    let launcher = launcher.unwrap_or_else(|| panic!("Steam not detected"));
    assert_eq!(launcher.id, "steam");
    assert_eq!(launcher.root, Some(steam.clone()));
    let games: Vec<(&str, &str, PathBuf, Option<u64>)> = launcher
        .games
        .iter()
        .map(|g| {
            assert_eq!(g.manifest_key, None);
            (
                g.store_game_id.as_str(),
                g.name.as_str(),
                g.install_dir.clone(),
                g.size_bytes,
            )
        })
        .collect();
    assert_eq!(
        games,
        [
            (
                "1245620",
                "ELDEN RING",
                join(&steam, "steamapps/common/ELDEN RING"),
                Some(51_352_018_534)
            ),
            (
                "367520",
                "Hollow Knight",
                join(&second, "steamapps/common/Hollow Knight"),
                Some(9_184_628_461)
            ),
        ]
    );
    let users: Vec<(&str, Option<&str>, Option<&str>)> = launcher
        .user_ids
        .iter()
        .map(|u| (u.id.as_str(), u.alt_id.as_deref(), u.name.as_deref()))
        .collect();
    assert_eq!(
        users,
        [
            ("222", Some("76561197960265950"), None),
            ("12345678", Some("76561197972611406"), Some("Fixture Gamer")),
        ]
    );
    assert_eq!(keys(&issues), [(ISSUE_APPMANIFEST, "invalid")]);
    assert_eq!(
        issues[0].path.as_deref(),
        Some(r"{PROGRAMFILES_X86}\Steam\steamapps\appmanifest_9.acf")
    );
}

#[test]
fn missing_or_broken_libraryfolders_keeps_the_main_library() {
    let steam = default_steam();
    let mut setup = Setup::new();
    setup.file(
        &join(&steam, "steamapps/appmanifest_10.acf"),
        &acf("10", "Counter-Strike", "Half-Life", "5"),
    );
    let (launcher, issues) = setup.detect();
    let games = launcher.map(|l| l.games).unwrap_or_default();
    assert_eq!(games.len(), 1);
    assert_eq!(keys(&issues), [(ISSUE_LIBRARYFOLDERS, "not_found")]);
    assert_eq!(
        issues[0].path.as_deref(),
        Some(r"{PROGRAMFILES_X86}\Steam\steamapps\libraryfolders.vdf")
    );

    setup.file(
        &join(&steam, "steamapps/libraryfolders.vdf"),
        "\"libraryfolders\" {",
    );
    let (launcher, issues) = setup.detect();
    assert_eq!(launcher.map(|l| l.games.len()), Some(1));
    assert_eq!(keys(&issues), [(ISSUE_LIBRARYFOLDERS, "invalid")]);
}

#[test]
fn main_library_without_steamapps_has_no_games() {
    let mut setup = Setup::new();
    setup.dir(&default_steam());
    let (launcher, issues) = setup.detect();
    let launcher = launcher.unwrap_or_else(|| panic!("Steam not detected"));
    assert!(launcher.games.is_empty());
    assert!(launcher.user_ids.is_empty());
    assert_eq!(keys(&issues), [(ISSUE_LIBRARYFOLDERS, "not_found")]);
}

#[test]
fn unavailable_libraries_are_skipped() {
    let steam = default_steam();
    let absent = join(&root(), "Games/Absent");
    let offline = PathBuf::from(r"Q:\SteamLibrary");
    let mut setup = Setup::new();
    setup.file(
        &join(&steam, "steamapps/libraryfolders.vdf"),
        &libraryfolders(&[&steam, &absent, &offline]),
    );
    let before = setup.fs.calls().read_dir;
    let (launcher, issues) = setup.detect();
    assert_eq!(launcher.map(|l| l.games.len()), Some(0));
    assert_eq!(
        keys(&issues),
        [
            (ISSUE_LIBRARY, "not_found"),
            (ISSUE_LIBRARY, "drive_missing")
        ]
    );
    let drive = |i: &ScanIssue| i.message_args.get("drive").cloned();
    assert_eq!(drive(&issues[0]), cfg!(windows).then(|| "C".to_owned()));
    assert_eq!(drive(&issues[1]), Some("Q".to_owned()));
    // Outside Windows `Q:` is not a drive prefix, so `from_path` keeps it literal.
    let offline_template = PathTemplate::from_path(&offline, &setup.env).to_string();
    if cfg!(windows) {
        assert_eq!(offline_template, r"{DRIVE:Q}\SteamLibrary");
    }
    assert_eq!(issues[1].path.as_deref(), Some(offline_template.as_str()));
    // steamapps of the main and the absent library, userdata; not the Q: drive.
    assert_eq!(setup.fs.calls().read_dir - before, 3);
}

#[test]
fn fixture_files_on_a_fake_profile() {
    let steam = default_steam();
    let mut setup = Setup::new();
    setup
        .file(
            &join(&steam, "steamapps/libraryfolders.vdf"),
            &sample("libraryfolders.vdf"),
        )
        .file(
            &join(&steam, "steamapps/appmanifest_1245620.acf"),
            &sample("appmanifest_1245620.acf"),
        )
        .file(
            &join(&steam, "config/loginusers.vdf"),
            &sample("loginusers.vdf"),
        )
        .dir(&join(&steam, "userdata/12345678/1245620/remote"));
    let (launcher, issues) = setup.detect();
    let launcher = launcher.unwrap_or_else(|| panic!("Steam not detected"));
    assert_eq!(launcher.games.len(), 1);
    assert_eq!(launcher.games[0].store_game_id, "1245620");
    assert_eq!(
        launcher.user_ids,
        [StoreUser {
            id: "12345678".to_owned(),
            alt_id: Some("76561197972611406".to_owned()),
            name: Some("Fixture Gamer".to_owned()),
        }]
    );
    // The fixture lists the real `C:\Program Files (x86)\Steam` (not this
    // fake one) and a `D:` drive the fake environment does not have.
    assert_eq!(
        keys(&issues),
        [
            (ISSUE_LIBRARY, "not_found"),
            (ISSUE_LIBRARY, "drive_missing")
        ]
    );
}

/// `{STEAM}` and `{STEAM_USERID}` resolve from the detected launcher, and a
/// template specialized to one id3 (SPEC-02 §3.2) resolves to that account
/// only and is what `from_path` gives back.
#[test]
fn steam_tokens_resolve_from_the_detected_launcher() {
    let steam = default_steam();
    let mut setup = Setup::new();
    for user in ["111", "222"] {
        setup.dir(&join(&steam, &format!("userdata/{user}/remote")));
    }
    let launcher = setup.launcher();
    let ctx = ResolveContext {
        steam_user_ids: launcher.user_ids.iter().map(|u| u.id.clone()).collect(),
        ..ResolveContext::default()
    };
    let mut env = setup.env.clone();
    env.launchers.push(launcher);

    let template = PathTemplate::parse(r"{STEAM}\userdata\{STEAM_USERID}\remote")
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        template.resolve(&env, &ctx),
        [
            join(&steam, "userdata/111/remote"),
            join(&steam, "userdata/222/remote")
        ]
    );

    let specialized = PathTemplate::parse(&template.as_str().replace("{STEAM_USERID}", "222"))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(specialized.as_str(), r"{STEAM}\userdata\222\remote");
    assert_eq!(
        specialized.resolve(&env, &ResolveContext::default()),
        [join(&steam, "userdata/222/remote")]
    );
    assert_eq!(
        PathTemplate::from_path(&join(&steam, "userdata/222/remote"), &env),
        specialized
    );
}

#[test]
fn native_paths_and_drive_letters() {
    let sep = MAIN_SEPARATOR_STR;
    assert_eq!(
        native_path(" c:/program files (x86)/steam/ "),
        PathBuf::from(format!("c:{sep}program files (x86){sep}steam"))
    );
    assert_eq!(native_path("D:/"), PathBuf::from(format!("D:{sep}")));
    assert_eq!(drive_letter(Path::new(r"d:\SteamLibrary")), Some('D'));
    assert_eq!(drive_letter(Path::new(r"\\?\E:\Lib")), Some('E'));
    assert_eq!(drive_letter(Path::new(r"\\server\share")), None);
    assert_eq!(drive_letter(Path::new("/fake/Steam")), None);
}
