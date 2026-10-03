use std::path::{Path, PathBuf};

use globset::GlobBuilder;
use sk_core::env::Environment;

use super::*;

fn fake_root() -> PathBuf {
    PathBuf::from(if cfg!(windows) { r"C:\fake" } else { "/fake" })
}

fn join(base: &Path, rel: &str) -> PathBuf {
    rel.split('/').fold(base.to_path_buf(), |p, c| p.join(c))
}

fn game_dir() -> PathBuf {
    join(&fake_root(), "Games/Elden Ring")
}

/// Context of an installed Steam game with every value known.
fn steam_ctx() -> GameCtx {
    GameCtx {
        game_dir: Some(game_dir()),
        launcher: Some("steam".to_owned()),
        root: Some(tpl(r"{PROGRAMFILES_X86}\Steam")),
        store_user_ids: vec!["22202".to_owned(), "76561197960287930".to_owned()],
        store_game_id: Some("1245620".to_owned()),
        os_user_name: Some("max".to_owned()),
    }
}

fn epic_ctx() -> GameCtx {
    GameCtx {
        launcher: Some("epic".to_owned()),
        root: None,
        store_user_ids: Vec::new(),
        ..steam_ctx()
    }
}

fn tpl(s: &str) -> PathTemplate {
    PathTemplate::parse(s).unwrap_or_else(|e| panic!("{s}: {e}"))
}

/// `translate` that must succeed, as (template string, include).
fn ok(path: &str, ctx: &GameCtx) -> (String, Vec<String>) {
    let (template, include) = translate(path, ctx).unwrap_or_else(|| panic!("{path} was rejected"));
    (template.as_str().to_owned(), include)
}

fn no_include(template: &str) -> (String, Vec<String>) {
    (template.to_owned(), Vec::new())
}

/// The glob and, unless it ends in `**`, the contents of a matching folder.
fn with_include(template: &str, include: &str) -> (String, Vec<String>) {
    let mut globs = vec![include.to_owned()];
    if include != "**" && !include.ends_with("/**") {
        globs.push(format!("{include}/**"));
    }
    (template.to_owned(), globs)
}

#[test]
fn root_placeholders_map_to_folder_tokens() {
    let ctx = GameCtx::default();
    let cases = [
        ("<home>/Game", r"{HOME}\Game"),
        ("<winAppData>/EldenRing", r"{APPDATA}\EldenRing"),
        ("<winLocalAppData>/Game/Saved", r"{LOCALAPPDATA}\Game\Saved"),
        (
            "<winLocalAppDataLow>/Team Cherry",
            r"{LOCALLOW}\Team Cherry",
        ),
        (
            "<winDocuments>/My Games/Skyrim",
            r"{DOCUMENTS}\My Games\Skyrim",
        ),
        ("<winPublic>/Documents/Game", r"{PUBLIC}\Documents\Game"),
        ("<winProgramData>/Game", r"{PROGRAMDATA}\Game"),
        ("<winDir>/game.ini", r"{WINDIR}\game.ini"),
    ];
    for (path, template) in cases {
        assert_eq!(ok(path, &ctx), no_include(template), "{path}");
    }
}

#[test]
fn base_maps_to_game_dir() {
    assert_eq!(
        ok("<base>/Game/*.ini", &steam_ctx()),
        with_include(r"{GAME_DIR}\Game", "*.ini")
    );
    assert_eq!(ok("<base>", &steam_ctx()), no_include("{GAME_DIR}"));
}

#[test]
fn game_and_store_game_id_map_to_value_tokens() {
    assert_eq!(
        ok("<winDocuments>/<game>/saves", &steam_ctx()),
        no_include(r"{DOCUMENTS}\{GAME_DIR_NAME}\saves")
    );
    assert_eq!(
        ok("<winAppData>/Store/<storeGameId>_save", &steam_ctx()),
        no_include(r"{APPDATA}\Store\{STORE_GAME_ID}_save")
    );
}

#[test]
fn root_is_substituted_as_text() {
    assert_eq!(
        ok("<root>/userdata/<storeUserId>/1245620/remote", &steam_ctx()),
        no_include(r"{PROGRAMFILES_X86}\Steam\userdata\{STEAM_USERID}\1245620\remote")
    );
    let ctx = GameCtx {
        root: Some(tpl(r"D:\SteamLibrary")),
        ..steam_ctx()
    };
    assert_eq!(
        ok("<root>/steamapps/common/x", &ctx),
        no_include(r"D:\SteamLibrary\steamapps\common\x")
    );
}

#[test]
fn store_user_id_is_steam_user_id_only_for_steam() {
    assert_eq!(
        ok("<winAppData>/Game/<storeUserId>/save.dat", &steam_ctx()),
        no_include(r"{APPDATA}\Game\{STEAM_USERID}\save.dat")
    );
    // Outside Steam the segment becomes a glob and starts the include.
    assert_eq!(
        ok("<winAppData>/Game/<storeUserId>/save.dat", &epic_ctx()),
        with_include(r"{APPDATA}\Game", "*/save.dat")
    );
    assert_eq!(
        ok("<winAppData>/Game/user_<storeUserId>", &GameCtx::default()),
        with_include(r"{APPDATA}\Game", "user_*")
    );
}

#[test]
fn os_user_name_is_substituted_as_text() {
    assert_eq!(
        ok("<winDocuments>/Game/<osUserName>/save", &steam_ctx()),
        no_include(r"{DOCUMENTS}\Game\max\save")
    );
    assert_eq!(
        ok("<winDocuments>/Game/*/<osUserName>.sav", &steam_ctx()),
        with_include(r"{DOCUMENTS}\Game", "*/max.sav")
    );
}

#[test]
fn unsupported_and_unknown_placeholders_are_rejected() {
    for path in [
        "<xdgData>/game",
        "<xdgConfig>/game",
        "<regHkcu>/Software/Game",
        "<regHklm>/Software/Game",
        "<winAppData>/<xdgData>",
        "<unknown>/game",
        "<winAppData>/<unknown>/x",
        "<winAppData>/a<b",
        "<winAppData>/a>b",
        "<winAppData>/<>/x",
    ] {
        assert_eq!(translate(path, &steam_ctx()), None, "{path}");
    }
}

#[test]
fn path_must_start_with_a_root() {
    for path in [
        "Game/saves",
        "/home/user/.game",
        "~/.game",
        "*/saves",
        "",
        "/",
        "<storeUserId>/x",
        "<game>/x",
        "<osUserName>/x",
        "<winAppData>Low/Game",
        "<home><home>/.prefs/*.sav",
        "x<winAppData>/Game",
    ] {
        assert_eq!(translate(path, &steam_ctx()), None, "{path:?}");
    }
    assert_eq!(
        ok("C:/Games/Old/save.dat", &GameCtx::default()),
        no_include(r"C:\Games\Old\save.dat")
    );
}

#[test]
fn root_placeholder_elsewhere_is_rejected() {
    for path in [
        "<winDocuments>/<home>/x",
        "<winDocuments>/<base>",
        "<winDocuments>/<root>/x",
        "<winDocuments>/*/<winAppData>",
        "<winDocuments>/*/<base>",
    ] {
        assert_eq!(translate(path, &steam_ctx()), None, "{path}");
    }
}

#[test]
fn dot_segments_are_rejected() {
    for path in [
        "<winDocuments>/../batclient",
        "<winLocalAppData>/../LocalLow/Game",
        "<winAppData>/./Game",
        "<winAppData>/Game/*/../x",
    ] {
        assert_eq!(translate(path, &steam_ctx()), None, "{path}");
    }
    // Dots inside a name are fine.
    assert_eq!(
        ok("<base>/egg is broken..exe", &steam_ctx()),
        no_include(r"{GAME_DIR}\egg is broken..exe")
    );
}

#[test]
fn missing_context_values_reject_the_path() {
    let ctx = GameCtx::default();
    for path in [
        "<base>/saves",
        "<winDocuments>/<game>",
        "<root>/userdata/<storeUserId>/1/remote",
        "<winAppData>/<storeGameId>",
        "<winAppData>/x/<osUserName>",
        "<winAppData>/*/<game>.sav",
        "<winAppData>/*/<storeGameId>.sav",
        "<winAppData>/*/<osUserName>.sav",
    ] {
        assert_eq!(translate(path, &ctx), None, "{path}");
    }
    let no_root = GameCtx {
        root: None,
        ..steam_ctx()
    };
    assert_eq!(translate("<root>/userdata", &no_root), None);
}

#[test]
fn glob_splits_root_and_include() {
    let ctx = steam_ctx();
    let cases = [
        (
            "<winDocuments>/My Games/Skyrim/Saves/*.ess",
            r"{DOCUMENTS}\My Games\Skyrim\Saves",
            "*.ess",
        ),
        ("<base>/**/*.sav", "{GAME_DIR}", "**/*.sav"),
        ("<winAppData>/*/Saves", "{APPDATA}", "*/Saves"),
        (
            "<winAppData>/Game/save?.dat",
            r"{APPDATA}\Game",
            "save?.dat",
        ),
        (
            "<base>/save/<storeUserId>/profile[0,1,3].ojs",
            r"{GAME_DIR}\save\{STEAM_USERID}",
            "profile[0,1,3].ojs",
        ),
        ("<winAppData>/Game/Saves/**", r"{APPDATA}\Game\Saves", "**"),
        (
            "<winAppData>/A/s*/deep/<storeUserId>/x",
            r"{APPDATA}\A",
            "s*/deep/*/x",
        ),
    ];
    for (path, template, include) in cases {
        assert_eq!(ok(path, &ctx), with_include(template, include), "{path}");
    }
}

#[test]
fn brackets_without_a_class_are_text() {
    assert_eq!(
        ok("<winAppData>/Game [GOTY/save", &steam_ctx()),
        no_include(r"{APPDATA}\Game [GOTY\save")
    );
    assert_eq!(
        ok("<winAppData>/Game]/save", &steam_ctx()),
        no_include(r"{APPDATA}\Game]\save")
    );
}

#[test]
fn separators_are_normalized() {
    assert_eq!(
        ok(r"<winAppData>\Game//Saves/", &steam_ctx()),
        no_include(r"{APPDATA}\Game\Saves")
    );
    assert_eq!(
        ok(r"<winAppData>/Game\Saves\*.sav", &steam_ctx()),
        with_include(r"{APPDATA}\Game\Saves", "*.sav")
    );
}

#[test]
fn home_folders_use_known_folder_tokens() {
    let ctx = GameCtx::default();
    let cases = [
        (
            "<home>/AppData/LocalLow/Team Cherry/Hollow Knight/*.dat",
            r"{LOCALLOW}\Team Cherry\Hollow Knight",
            Some("*.dat"),
        ),
        ("<home>/AppData/Roaming/Game", r"{APPDATA}\Game", None),
        ("<home>/appdata/local/Game", r"{LOCALAPPDATA}\Game", None),
        (
            "<home>/Saved Games/id Software/x",
            r"{SAVED_GAMES}\id Software\x",
            None,
        ),
        ("<home>/AppData/LocalLow", "{LOCALLOW}", None),
        (
            "<home>/AppData/Other/Game",
            r"{HOME}\AppData\Other\Game",
            None,
        ),
        ("<home>/AppData", r"{HOME}\AppData", None),
        ("<home>/AppData/*/Game", r"{HOME}\AppData", Some("*/Game")),
        (
            "<winDocuments>/AppData/Roaming",
            r"{DOCUMENTS}\AppData\Roaming",
            None,
        ),
    ];
    for (path, template, include) in cases {
        let expected = match include {
            Some(include) => with_include(template, include),
            None => no_include(template),
        };
        assert_eq!(ok(path, &ctx), expected, "{path}");
    }
}

#[test]
fn token_like_text_is_rejected_and_guid_braces_are_kept() {
    for path in [
        "<winAppData>/{APPDATA}/x",
        "<winAppData>/x/{STEAM_USERID}",
        "<base>/${DIMETROSAUR}.exe",
    ] {
        assert_eq!(translate(path, &steam_ctx()), None, "{path}");
    }
    assert_eq!(
        ok(
            "<winAppData>/Disney/{random number sequence}/Saved",
            &steam_ctx()
        ),
        no_include(r"{APPDATA}\Disney\{random number sequence}\Saved")
    );
    assert_eq!(
        ok("<winAppData>/{1AC14E77-02E7-4E5D}/s", &steam_ctx()),
        no_include(r"{APPDATA}\{1AC14E77-02E7-4E5D}\s")
    );
}

#[test]
fn include_escapes_braces_and_substituted_values() {
    let ctx = GameCtx {
        game_dir: Some(join(&fake_root(), "Games/Game [GOTY] {x}")),
        store_game_id: Some("a*b?".to_owned()),
        ..steam_ctx()
    };
    assert_eq!(
        ok("<winAppData>/G/*/{random}/<game>.sav", &ctx),
        with_include(r"{APPDATA}\G", "*/[{]random[}]/Game [[]GOTY[]] [{]x[}].sav")
    );
    assert_eq!(
        ok("<winAppData>/G/*_<storeGameId>", &ctx),
        with_include(r"{APPDATA}\G", "*_a[*]b[?]")
    );
    assert_eq!(
        ok("<winAppData>/G/*/Game [x/s[12] [y", &ctx),
        with_include(r"{APPDATA}\G", "*/Game [[]x/s[12] [[]y")
    );
}

/// Include globs compile with the options of `sk-scan` and match like Ludusavi.
#[test]
fn include_globs_match_literally() {
    let ctx = GameCtx {
        game_dir: Some(join(&fake_root(), "Games/Game [GOTY] {x}")),
        ..steam_ctx()
    };
    let cases = [
        (
            "<winAppData>/G/*/{random}/<game>.sav",
            "p/{random}/Game [GOTY] {x}.sav",
            "p/random/Game G {x}.sav",
        ),
        (
            "<base>/*/profile[0,1,3].ojs",
            "s/profile1.ojs",
            "s/profile2.ojs",
        ),
        ("<base>/*/save?.dat", "a/save1.dat", "a/b/save1.dat"),
        // `[` without a closing `]` is text, also after a class.
        ("<base>/*/Game [x", "a/Game [x", "a/Game x"),
        ("<base>/*/s[0-9] [x", "a/s1 [x", "a/s1 x"),
    ];
    for (path, matching, other) in cases {
        let (_, include) = ok(path, &ctx);
        let glob = include
            .first()
            .unwrap_or_else(|| panic!("{path}: no include"));
        let matcher = GlobBuilder::new(glob)
            .case_insensitive(true)
            .literal_separator(true)
            .build()
            .unwrap_or_else(|e| panic!("{glob}: {e}"))
            .compile_matcher();
        assert!(matcher.is_match(matching), "{glob} vs {matching}");
        assert!(!matcher.is_match(other), "{glob} vs {other}");
    }
}

#[test]
fn game_dir_name_and_resolve_context() {
    let ctx = steam_ctx();
    assert_eq!(ctx.game_dir_name().as_deref(), Some("Elden Ring"));
    let rc = ctx.resolve_context();
    assert_eq!(rc.game_dir, Some(game_dir()));
    assert_eq!(rc.steam_user_ids, ctx.store_user_ids);
    assert_eq!(rc.store_game_id.as_deref(), Some("1245620"));
    assert_eq!(rc.game_dir_name.as_deref(), Some("Elden Ring"));
    assert_eq!(
        GameCtx::default().resolve_context(),
        ResolveContext::default()
    );
}

#[test]
fn game_dir_token_resolves_to_the_install_folder() {
    let env = Environment::fake(&fake_root());
    let ctx = steam_ctx();
    let rc = ctx.resolve_context();
    let resolve = |path: &str| {
        let (template, _) = translate(path, &ctx).unwrap_or_else(|| panic!("{path}"));
        template.resolve(&env, &rc)
    };
    assert_eq!(
        resolve("<base>/Game/*.ini"),
        vec![join(&game_dir(), "Game")]
    );
    assert_eq!(
        resolve("<winDocuments>/<game>/<storeGameId>"),
        vec![join(
            &fake_root(),
            "Users/user/Documents/Elden Ring/1245620"
        )]
    );
    assert_eq!(
        resolve("<root>/userdata/<storeUserId>/remote"),
        vec![
            join(
                &fake_root(),
                "Program Files (x86)/Steam/userdata/22202/remote"
            ),
            join(
                &fake_root(),
                "Program Files (x86)/Steam/userdata/76561197960287930/remote"
            ),
        ]
    );
    assert_eq!(
        resolve("<home>/AppData/LocalLow/Team Cherry/Hollow Knight"),
        vec![join(
            &fake_root(),
            "Users/user/AppData/LocalLow/Team Cherry/Hollow Knight"
        )]
    );
    // Without a game folder `{GAME_DIR}` resolves to nothing.
    let (template, _) = translate("<base>/saves", &ctx).unwrap_or_else(|| panic!("base"));
    assert!(template
        .resolve(&env, &GameCtx::default().resolve_context())
        .is_empty());
}
