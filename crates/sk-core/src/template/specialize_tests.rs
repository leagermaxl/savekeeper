use std::path::PathBuf;

use super::*;
use crate::env::{DriveInfo, DriveKind, DriveMedia};

fn tpl(s: &str) -> PathTemplate {
    PathTemplate::parse(s).unwrap()
}

fn env() -> Environment {
    Environment::fake(&PathBuf::from(if cfg!(windows) {
        r"C:\fake"
    } else {
        "/fake"
    }))
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

fn ctx(ids: &[&str]) -> ResolveContext {
    ResolveContext {
        steam_user_ids: ids.iter().map(|id| (*id).to_owned()).collect(),
        ..ResolveContext::default()
    }
}

fn texts(templates: &[PathTemplate]) -> Vec<&str> {
    templates.iter().map(PathTemplate::as_str).collect()
}

#[test]
fn template_without_multi_valued_tokens_is_kept() {
    let env = env();
    for s in [
        r"{APPDATA}\X",
        r"{DRIVE:C}\RetroArch",
        r"{PACKAGE:Microsoft.WindowsTerminal}\LocalState",
        r"{GAME_DIR}\saves\{STORE_GAME_ID}\{GAME_DIR_NAME}",
        r"{APPDATA}\*\User",
        r"\\server\share\x",
    ] {
        let template = tpl(s);
        assert_eq!(
            template.specialize(&env, &ctx(&["1"])),
            std::slice::from_ref(&template),
            "{s}"
        );
    }
}

#[test]
fn all_drives_become_fixed_drives_in_env_order() {
    let mut env = env();
    env.drives = vec![
        drive('D', DriveKind::Fixed),
        drive('E', DriveKind::Removable),
        drive('C', DriveKind::Fixed),
        drive('Z', DriveKind::Network),
    ];
    let out = tpl(r"{DRIVE:*}\Emulators\*").specialize(&env, &ctx(&[]));
    assert_eq!(
        texts(&out),
        [r"{DRIVE:D}\Emulators\*", r"{DRIVE:C}\Emulators\*"]
    );
}

#[test]
fn steam_ids_replace_every_occurrence_in_ctx_order() {
    let out = tpl(r"{STEAM}\userdata\{STEAM_USERID}\cfg_{STEAM_USERID}")
        .specialize(&env(), &ctx(&["222", "111"]));
    assert_eq!(
        texts(&out),
        [
            r"{STEAM}\userdata\222\cfg_222",
            r"{STEAM}\userdata\111\cfg_111"
        ]
    );
}

#[test]
fn drives_come_first_then_steam_ids() {
    let mut env = env();
    env.drives = vec![drive('C', DriveKind::Fixed), drive('D', DriveKind::Fixed)];
    let out = tpl(r"{DRIVE:*}\Steam\userdata\{STEAM_USERID}").specialize(&env, &ctx(&["1", "2"]));
    assert_eq!(
        texts(&out),
        [
            r"{DRIVE:C}\Steam\userdata\1",
            r"{DRIVE:C}\Steam\userdata\2",
            r"{DRIVE:D}\Steam\userdata\1",
            r"{DRIVE:D}\Steam\userdata\2",
        ]
    );
}

#[test]
fn token_without_values_gives_nothing() {
    let mut env = env();
    assert!(tpl(r"{STEAM}\userdata\{STEAM_USERID}")
        .specialize(&env, &ctx(&[]))
        .is_empty());
    env.drives = vec![drive('E', DriveKind::Removable)];
    assert!(tpl(r"{DRIVE:*}\RetroArch")
        .specialize(&env, &ctx(&[]))
        .is_empty());
    // One of two tokens without values empties the product.
    env.drives = vec![drive('C', DriveKind::Fixed)];
    assert!(tpl(r"{DRIVE:*}\Steam\{STEAM_USERID}")
        .specialize(&env, &ctx(&[]))
        .is_empty());
}

#[test]
fn variant_that_does_not_parse_is_dropped() {
    // An id with a separator would make a `..` segment.
    let out = tpl(r"{STEAM}\userdata\{STEAM_USERID}").specialize(&env(), &ctx(&["..", "7"]));
    assert_eq!(texts(&out), [r"{STEAM}\userdata\7"]);
}
