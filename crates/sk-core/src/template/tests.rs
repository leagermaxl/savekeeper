use std::path::{Path, PathBuf};

use super::*;
use crate::env::{
    CloudProvider, CloudRoot, DriveInfo, DriveKind, DriveMedia, KnownFolder, LauncherInfo,
};

fn tpl(s: &str) -> PathTemplate {
    PathTemplate::parse(s).unwrap()
}

fn root() -> PathBuf {
    PathBuf::from(if cfg!(windows) { r"C:\fake" } else { "/fake" })
}

fn env() -> Environment {
    Environment::fake(&root())
}

fn folder(env: &Environment, f: KnownFolder) -> PathBuf {
    env.known_folder(f).unwrap().to_path_buf()
}

fn join(base: &Path, parts: &[&str]) -> PathBuf {
    let mut path = base.to_path_buf();
    path.extend(parts);
    path
}

fn drive(letter: char, kind: DriveKind) -> DriveInfo {
    DriveInfo {
        letter,
        kind,
        media: DriveMedia::Unknown,
        fs: None,
        label: None,
        volume_serial: None,
        total_bytes: 0,
        free_bytes: 0,
    }
}

// --- parse -------------------------------------------------------------------

#[test]
fn parse_normalizes_separators() {
    assert_eq!(tpl(r"{APPDATA}\Code\User").as_str(), r"{APPDATA}\Code\User");
    assert_eq!(
        tpl("{APPDATA}/Code//User/").as_str(),
        r"{APPDATA}\Code\User"
    );
    assert_eq!(tpl(r"\\nas\share\x").as_str(), r"\\nas\share\x");
    assert_eq!(tpl(r"C:\Games").as_str(), r"C:\Games");
}

#[test]
fn parse_reads_tokens() {
    let t = tpl(r"{STEAM}\userdata\{STEAM_USERID}\760\remote");
    assert_eq!(
        t.tokens().collect::<Vec<_>>(),
        [Token::Steam, Token::SteamUserId]
    );
    assert_eq!(
        tpl("{DRIVE:D}").tokens().collect::<Vec<_>>(),
        [Token::Drive('D')]
    );
    assert_eq!(
        tpl(r"{DRIVE:*}\RetroArch").tokens().next(),
        Some(Token::AllDrives)
    );
    assert_eq!(
        tpl(r"{PACKAGE:Microsoft.MinecraftUWP}\LocalState")
            .tokens()
            .next(),
        Some(Token::Package("Microsoft.MinecraftUWP".to_owned()))
    );
    assert_eq!(
        tpl(r"{GAME_DIR}\saves\{GAME_DIR_NAME}_{STORE_GAME_ID}")
            .tokens()
            .collect::<Vec<_>>(),
        [Token::GameDir, Token::GameDirName, Token::StoreGameId]
    );
    for f in KnownFolder::ALL {
        let t = tpl(&format!("{{{}}}", f.token()));
        assert_eq!(t.tokens().next(), Some(Token::Folder(f)));
    }
}

#[test]
fn guid_braces_are_text() {
    let t = tpl(r"{PROGRAMDATA}\{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\data");
    assert_eq!(t.tokens().count(), 1);
    assert_eq!(tpl(r"{LOCALAPPDATA}\a{b}c\{x}").tokens().count(), 1);
}

#[test]
fn parse_errors() {
    use TemplateError::*;
    let err = |s: &str| PathTemplate::parse(s).unwrap_err();
    assert_eq!(err(""), Empty);
    assert_eq!(err("///"), Empty);
    assert_eq!(err(r"{APPDTA}\x"), UnknownToken("{APPDTA}".to_owned()));
    assert_eq!(err(r"{APPDATA\x"), Unclosed(0));
    assert_eq!(err(r"{HOME}\a\{STEAM"), Unclosed(9));
    assert_eq!(err(r"x\{APPDATA}"), MisplacedToken("{APPDATA}".to_owned()));
    assert_eq!(err(r"{APPDATA}x\y"), MisplacedToken("{APPDATA}".to_owned()));
    assert_eq!(err(r"\\{HOME}\x"), MisplacedToken("{HOME}".to_owned()));
    assert_eq!(
        err(r"{STEAM_USERID}\x"),
        MisplacedToken("{STEAM_USERID}".to_owned())
    );
    assert_eq!(err("{DRIVE:cd}"), InvalidArgument("{DRIVE:cd}".to_owned()));
    assert_eq!(err("{DRIVE:d}"), InvalidArgument("{DRIVE:d}".to_owned()));
    assert_eq!(err("{DRIVE}"), InvalidArgument("{DRIVE}".to_owned()));
    assert_eq!(err("{PACKAGE:}"), InvalidArgument("{PACKAGE:}".to_owned()));
    assert_eq!(
        err("{PACKAGE:a b}"),
        InvalidArgument("{PACKAGE:a b}".to_owned())
    );
    assert_eq!(err("{HOME:x}"), InvalidArgument("{HOME:x}".to_owned()));
    assert_eq!(err("{FOO:x}"), UnknownToken("{FOO:x}".to_owned()));
    assert_eq!(err(r"{HOME}\..\x"), DotSegment);
    assert_eq!(err(r"{HOME}\.\x"), DotSegment);
}

#[test]
fn lowercase_token_is_text() {
    // `{appdata}` does not have token syntax, so it is a literal folder name.
    assert_eq!(tpl(r"{appdata}\x").tokens().count(), 0);
}

#[test]
fn serde_validates() {
    let t: PathTemplate = serde_json::from_str(r#""{APPDATA}/Code""#).unwrap();
    assert_eq!(t.as_str(), r"{APPDATA}\Code");
    assert_eq!(serde_json::to_string(&t).unwrap(), r#""{APPDATA}\\Code""#);
    assert!(serde_json::from_str::<PathTemplate>(r#""{NOPE}\\x""#).is_err());
}

// --- resolve -----------------------------------------------------------------

#[test]
fn resolve_known_folders() {
    let env = env();
    let ctx = ResolveContext::default();
    assert_eq!(
        tpl(r"{APPDATA}\Code\User").resolve(&env, &ctx),
        [join(&folder(&env, KnownFolder::AppData), &["Code", "User"])]
    );
    assert_eq!(
        tpl("{LOCALLOW}").resolve(&env, &ctx),
        [folder(&env, KnownFolder::LocalLow)]
    );
    assert_eq!(
        tpl(r"{APPDATA}\JetBrains\*").resolve(&env, &ctx),
        [join(
            &folder(&env, KnownFolder::AppData),
            &["JetBrains", "*"]
        )]
    );
}

#[test]
fn resolve_missing_values_is_empty() {
    let mut env = env();
    env.known_folders.remove(&KnownFolder::SavedGames);
    let ctx = ResolveContext::default();
    for s in [
        r"{SAVED_GAMES}\x",
        r"{ONEDRIVE}\x",
        r"{STEAM}\x",
        r"{GAME_DIR}\x",
        r"{DRIVE:Z}\x",
        r"{PACKAGE:Foo}\x",
        r"{HOME}\{STEAM_USERID}",
        r"{HOME}\{STORE_GAME_ID}",
    ] {
        assert!(tpl(s).resolve(&env, &ctx).is_empty(), "{s}");
    }
}

#[test]
fn resolve_onedrive_prefers_personal() {
    let mut env = env();
    let business = root().join("OneDrive - Corp");
    let personal = root().join("OneDrive");
    env.cloud_roots.push(CloudRoot {
        provider: CloudProvider::OneDriveBusiness,
        path: business.clone(),
    });
    let ctx = ResolveContext::default();
    assert_eq!(
        tpl(r"{ONEDRIVE}\x").resolve(&env, &ctx),
        [business.join("x")]
    );
    env.cloud_roots.push(CloudRoot {
        provider: CloudProvider::OneDrive,
        path: personal.clone(),
    });
    assert_eq!(
        tpl(r"{ONEDRIVE}\x").resolve(&env, &ctx),
        [personal.join("x")]
    );
}

#[test]
fn resolve_multi_valued_tokens() {
    let mut env = env();
    let steam = root().join("Steam");
    env.launchers.push(LauncherInfo {
        id: "steam".to_owned(),
        root: Some(steam.clone()),
        user_ids: vec![],
        games: vec![],
    });
    let ctx = ResolveContext {
        steam_user_ids: vec!["111".to_owned(), "222".to_owned()],
        ..Default::default()
    };
    assert_eq!(
        tpl(r"{STEAM}\userdata\{STEAM_USERID}\cfg_{STEAM_USERID}").resolve(&env, &ctx),
        [
            join(&steam, &["userdata", "111", "cfg_111"]),
            join(&steam, &["userdata", "222", "cfg_222"]),
        ]
    );

    env.drives.push(drive('D', DriveKind::Fixed));
    env.drives.push(drive('E', DriveKind::Removable));
    let all = tpl(r"{DRIVE:*}\RetroArch").resolve(&env, &ctx);
    assert_eq!(
        all,
        [
            PathBuf::from(r"C:\").join("RetroArch"),
            PathBuf::from(r"D:\").join("RetroArch")
        ]
    );
    assert_eq!(
        tpl(r"{DRIVE:E}\x").resolve(&env, &ctx),
        [PathBuf::from(r"E:\").join("x")]
    );
}

#[test]
fn resolve_packages_by_name() {
    let mut env = env();
    env.store_packages = vec![
        "Microsoft.MinecraftUWP_8wekyb3d8bbwe".to_owned(),
        "Microsoft.MinecraftUWPBeta_8wekyb3d8bbwe".to_owned(),
        "microsoft.minecraftuwp_aaaaaaaaaaaaa".to_owned(),
        "Broken_name".to_owned(),
    ];
    let packages = folder(&env, KnownFolder::LocalAppData).join("Packages");
    assert_eq!(
        tpl(r"{PACKAGE:Microsoft.MinecraftUWP}\LocalState")
            .resolve(&env, &ResolveContext::default()),
        [
            join(
                &packages,
                &["Microsoft.MinecraftUWP_8wekyb3d8bbwe", "LocalState"]
            ),
            join(
                &packages,
                &["microsoft.minecraftuwp_aaaaaaaaaaaaa", "LocalState"]
            ),
        ]
    );
}

#[test]
fn resolve_game_context() {
    let env = env();
    let game_dir = root().join("Games").join("Hades");
    let ctx = ResolveContext {
        game_dir: Some(game_dir.clone()),
        store_game_id: Some("1145360".to_owned()),
        game_dir_name: Some("Hades".to_owned()),
        ..Default::default()
    };
    assert_eq!(
        tpl(r"{GAME_DIR}\saves\{GAME_DIR_NAME}-{STORE_GAME_ID}").resolve(&env, &ctx),
        [join(&game_dir, &["saves", "Hades-1145360"])]
    );
}

#[test]
fn resolve_literal_paths() {
    let env = env();
    let ctx = ResolveContext::default();
    assert_eq!(
        tpl(r"C:\Games\x").resolve(&env, &ctx),
        [PathBuf::from(r"C:\").join("Games").join("x")]
    );
}

// --- from_path ---------------------------------------------------------------

#[test]
fn from_path_picks_most_specific_token() {
    let env = env();
    let local_low = folder(&env, KnownFolder::LocalLow);
    assert_eq!(
        PathTemplate::from_path(&join(&local_low, &["Unity", "Game"]), &env).as_str(),
        r"{LOCALLOW}\Unity\Game"
    );
    let home = folder(&env, KnownFolder::Home);
    assert_eq!(
        PathTemplate::from_path(&join(&home, &[".ssh"]), &env).as_str(),
        r"{HOME}\.ssh"
    );
    assert_eq!(PathTemplate::from_path(&home, &env).as_str(), "{HOME}");
}

#[test]
fn from_path_is_case_insensitive_and_keeps_tail_case() {
    let env = env();
    let app_data = folder(&env, KnownFolder::AppData);
    let upper = PathBuf::from(app_data.to_string_lossy().to_uppercase()).join("Code");
    assert_eq!(
        PathTemplate::from_path(&upper, &env).as_str(),
        r"{APPDATA}\Code"
    );
}

#[test]
fn from_path_does_not_match_partial_names() {
    let env = env();
    // `user2` is not under `{HOME}` = `Users\user`.
    let other = join(&root(), &["Users", "user2", "AppData"]);
    let t = PathTemplate::from_path(&other, &env);
    assert!(!t.as_str().starts_with("{HOME}"), "{t}");
}

#[test]
fn from_path_documents_redirected_to_onedrive() {
    let mut env = env();
    let onedrive = folder(&env, KnownFolder::Home).join("OneDrive");
    let documents = onedrive.join("Документы");
    env.cloud_roots.push(CloudRoot {
        provider: CloudProvider::OneDrive,
        path: onedrive.clone(),
    });
    env.known_folders
        .insert(KnownFolder::Documents, documents.clone());
    assert_eq!(
        PathTemplate::from_path(&documents.join("My Games"), &env).as_str(),
        r"{DOCUMENTS}\My Games"
    );
    assert_eq!(
        PathTemplate::from_path(&onedrive.join("Photos"), &env).as_str(),
        r"{ONEDRIVE}\Photos"
    );
    // Round trip through resolve.
    let t = PathTemplate::from_path(&documents.join("x"), &env);
    assert_eq!(
        t.resolve(&env, &ResolveContext::default()),
        [documents.join("x")]
    );
}

#[test]
fn from_path_store_package() {
    let env = env();
    let pkg = join(
        &folder(&env, KnownFolder::LocalAppData),
        &[
            "Packages",
            "Microsoft.MinecraftUWP_8wekyb3d8bbwe",
            "LocalState",
        ],
    );
    assert_eq!(
        PathTemplate::from_path(&pkg, &env).as_str(),
        r"{PACKAGE:Microsoft.MinecraftUWP}\LocalState"
    );
    let not_pfn = join(
        &folder(&env, KnownFolder::LocalAppData),
        &["Packages", "Temp"],
    );
    assert_eq!(
        PathTemplate::from_path(&not_pfn, &env).as_str(),
        r"{LOCALAPPDATA}\Packages\Temp"
    );
}

#[test]
fn from_path_round_trips_for_every_folder() {
    let env = env();
    for f in KnownFolder::ALL {
        let path = folder(&env, f).join("Some App");
        let t = PathTemplate::from_path(&path, &env);
        assert_eq!(t.resolve(&env, &ResolveContext::default()), [path], "{t}");
    }
}

#[cfg(windows)]
mod windows {
    use super::*;

    #[test]
    fn from_path_drives_and_literals() {
        let mut env = env();
        env.drives.push(drive('D', DriveKind::Fixed));
        assert_eq!(
            PathTemplate::from_path(Path::new(r"D:\Games\Emu"), &env).as_str(),
            r"{DRIVE:D}\Games\Emu"
        );
        // A drive not in the environment still becomes a drive token.
        assert_eq!(
            PathTemplate::from_path(Path::new(r"z:\x"), &env).as_str(),
            r"{DRIVE:Z}\x"
        );
        assert_eq!(
            PathTemplate::from_path(Path::new(r"C:\Windows\Fonts"), &env).as_str(),
            r"{DRIVE:C}\Windows\Fonts"
        );
        assert_eq!(
            PathTemplate::from_path(Path::new(r"\\nas\share\photos"), &env).as_str(),
            r"\\nas\share\photos"
        );
        let verbatim = Path::new(r"\\?\C:\fake\Users\user\AppData\Roaming\Code");
        assert_eq!(
            PathTemplate::from_path(verbatim, &env).as_str(),
            r"{APPDATA}\Code"
        );
    }

    #[test]
    fn real_environment_round_trip() {
        let env = Environment::detect().unwrap();
        let ctx = ResolveContext::default();
        for (f, path) in &env.known_folders {
            let t = PathTemplate::from_path(path, &env);
            let resolved = t.resolve(&env, &ctx);
            assert_eq!(resolved.len(), 1, "{f:?} {t}");
            assert!(crate::path::eq_ci(&resolved[0], path), "{f:?} {t}");
        }
    }
}
