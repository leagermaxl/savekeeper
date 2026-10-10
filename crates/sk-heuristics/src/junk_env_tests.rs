//! Tests of the environment rows of `junk_verdict` (T-07-03, SPEC-07 §4.5):
//! installed programs, files written at once, launchers and game libraries,
//! the Steam and NVIDIA shader caches, and the precedence of the rows.

use std::path::{Path, PathBuf};

use sk_core::env::{InstalledGame, InstalledProgram, LauncherInfo, ProgramSource};
use sk_core::template::PathTemplate;

use super::tests::{abs, assert_verdict, env, local, root, summary, verdict, with_spread};
use super::*;

fn program(location: Option<&Path>) -> InstalledProgram {
    InstalledProgram {
        name: "Foo".to_owned(),
        publisher: Some("Foo Inc.".to_owned()),
        version: Some("1.0".to_owned()),
        install_location: location.map(Path::to_path_buf),
        install_date: None,
        estimated_size_kb: None,
        source: ProgramSource::Hkcu,
        uninstall_key: "Foo".to_owned(),
    }
}

#[test]
fn executables_inside_install_location_are_reinstallable() {
    let mut env = env();
    let location = local(r"Programs\Foo");
    env.installed_programs.push(program(None));
    env.installed_programs.push(program(Some(&location)));
    for path in [
        location.clone(),
        location.join("plugins"),
        local(r"PROGRAMS\FOO"),
    ] {
        assert_verdict(
            verdict(&path, &env, &[Marker::HasExecutables]),
            Category::Reinstallable,
            0.9,
            REASON_INSTALLED,
        );
    }
    // Without executables, or outside the location, the row does not match.
    assert_eq!(verdict(&location, &env, &[]), None);
    assert_eq!(
        verdict(&local(r"Programs\Foobar"), &env, &[Marker::HasExecutables]),
        None
    );
}

#[test]
fn broad_install_locations_are_ignored() {
    let mut env = env();
    let drive_root = PathBuf::from(if cfg!(windows) { r"C:\" } else { "/" });
    for location in [
        drive_root,
        root(),
        abs("Users"),
        abs(r"Users\user"),
        abs("Program Files"),
        local(""),
    ] {
        env.installed_programs.push(program(Some(&location)));
    }
    env.installed_programs.push(program(Some(Path::new(""))));
    let path = abs(r"Users\user\AppData\Local\Tool");
    assert_eq!(verdict(&path, &env, &[Marker::HasExecutables]), None);
}

#[test]
fn binaries_written_at_once_are_reinstallable() {
    let env = env();
    let path = abs(r"Users\user\Tools\Portable");
    let exe = [Marker::HasExecutables];
    let s = with_spread(summary(&path, &env, &exe), 59 * 60);
    assert_verdict(
        junk_verdict(&s, &env),
        Category::Reinstallable,
        0.7,
        REASON_UNIFORM_BINARIES,
    );
    // Spread of an hour or more, or config/database files: not reinstallable.
    let s = with_spread(summary(&path, &env, &exe), 60 * 60);
    assert_eq!(junk_verdict(&s, &env), None);
    for extra in [Marker::ConfigLike, Marker::SqliteFiles] {
        let s = with_spread(summary(&path, &env, &[Marker::HasExecutables, extra]), 0);
        assert_eq!(junk_verdict(&s, &env), None, "{extra:?}");
    }
    let s = with_spread(summary(&path, &env, &[]), 0);
    assert_eq!(junk_verdict(&s, &env), None);
    // No file times (no files): not reinstallable.
    let mut s = summary(&path, &env, &exe);
    s.newest_mtime = None;
    s.oldest_mtime = None;
    assert_eq!(junk_verdict(&s, &env), None);
}

fn launcher(id: &str, root: Option<&Path>, games: &[&Path]) -> LauncherInfo {
    LauncherInfo {
        id: id.to_owned(),
        root: root.map(Path::to_path_buf),
        user_ids: Vec::new(),
        games: games
            .iter()
            .map(|dir| InstalledGame {
                store_game_id: "1".to_owned(),
                name: "Game".to_owned(),
                install_dir: dir.to_path_buf(),
                size_bytes: None,
                manifest_key: None,
            })
            .collect(),
    }
}

#[test]
fn launcher_folders_and_game_libraries_are_reinstallable() {
    let mut env = env();
    let epic = abs(r"Program Files\Epic Games\Launcher");
    let game = abs(r"Games\GOG Games\Witcher");
    let other = abs(r"Games\GOG Games\Gothic");
    env.launchers.push(launcher("epic", Some(&epic), &[]));
    env.launchers.push(launcher("gog", None, &[&game, &other]));
    for path in [epic.clone(), game.clone(), abs(r"games\gog games")] {
        assert_verdict(
            verdict(&path, &env, &[]),
            Category::Reinstallable,
            0.95,
            REASON_LAUNCHER,
        );
    }
    // Data inside a launcher (Steam `userdata`) is not reinstallable by this row.
    assert_eq!(verdict(&epic.join("userdata"), &env, &[]), None);
    assert_eq!(verdict(&abs("Games"), &env, &[]), None);
}

#[test]
fn the_folder_of_a_single_game_is_not_a_library() {
    // A game installed into `D:\Stuff\Game` must not make `D:\Stuff` reinstallable.
    let mut env = env();
    let game = abs(r"Stuff\Game");
    env.launchers.push(launcher("gog", None, &[&game]));
    assert_eq!(verdict(&abs("Stuff"), &env, &[]), None);
    assert_verdict(
        verdict(&game, &env, &[]),
        Category::Reinstallable,
        0.95,
        REASON_LAUNCHER,
    );
    // The same game listed by two launchers is still one game.
    env.launchers
        .push(launcher("other", None, &[&abs(r"STUFF\GAME")]));
    assert_eq!(verdict(&abs("Stuff"), &env, &[]), None);
}

#[test]
fn the_parent_of_two_games_is_a_library() {
    let mut env = env();
    let first = abs(r"Games\First");
    let second = abs(r"games\Second");
    env.launchers.push(launcher("gog", None, &[&first]));
    env.launchers.push(launcher("epic", None, &[&second]));
    for path in [abs("Games"), first, second] {
        assert_verdict(
            verdict(&path, &env, &[]),
            Category::Reinstallable,
            0.95,
            REASON_LAUNCHER,
        );
    }
    assert_eq!(verdict(&abs(r"Games\Saves"), &env, &[]), None);
}

#[test]
fn steamapps_common_is_a_library_even_with_one_game() {
    let mut env = env();
    let library = abs(r"SteamLibrary\steamapps\common");
    let game = library.join("Game");
    env.launchers.push(launcher("steam", None, &[&game]));
    for path in [library.clone(), abs(r"SteamLibrary\SteamApps\Common"), game] {
        assert_verdict(
            verdict(&path, &env, &[]),
            Category::Reinstallable,
            0.95,
            REASON_LAUNCHER,
        );
    }
    // `steamapps` itself and folders with only a similar name are not libraries.
    assert_eq!(verdict(&abs(r"SteamLibrary\steamapps"), &env, &[]), None);
    assert_eq!(verdict(&abs(r"Stuff\common"), &env, &[]), None);
}

#[test]
fn games_in_a_known_folder_or_a_drive_root_do_not_make_it_a_library() {
    let mut env = env();
    let game = abs(r"Users\user\Documents\Game");
    let other = abs(r"Users\user\Documents\Other");
    let drive = PathBuf::from(if cfg!(windows) { r"C:\" } else { "/" });
    env.launchers.push(launcher("gog", None, &[&game, &other]));
    env.launchers.push(launcher(
        "epic",
        None,
        &[&drive.join("A"), &drive.join("B")],
    ));
    let documents = abs(r"Users\user\Documents");
    assert_eq!(verdict(&documents, &env, &[]), None);
    assert_verdict(
        verdict(&game, &env, &[]),
        Category::Reinstallable,
        0.95,
        REASON_LAUNCHER,
    );
}

#[test]
fn the_most_confident_row_wins() {
    let mut env = env();
    // `pip\cache` is a cache name (0.9) and a restorable folder (0.95).
    assert_verdict(
        verdict(&local(r"pip\cache"), &env, &[Marker::CacheLike]),
        Category::Cache,
        0.95,
        REASON_RESTORABLE,
    );
    // A cache name beats the `CacheLike` marker.
    assert_verdict(
        verdict(&local(r"App\Temp"), &env, &[Marker::CacheLike]),
        Category::Cache,
        0.9,
        REASON_CACHE_NAME,
    );
    // A launcher root with executables: the launcher row beats the program row.
    let launcher_root = local("FooLauncher");
    env.launchers
        .push(launcher("foo", Some(&launcher_root), &[]));
    env.installed_programs.push(program(Some(&launcher_root)));
    assert_verdict(
        verdict(&launcher_root, &env, &[Marker::HasExecutables]),
        Category::Reinstallable,
        0.95,
        REASON_LAUNCHER,
    );
}

#[test]
fn steam_shader_cache_and_nvidia_dx_cache_are_cache() {
    let mut env = env();
    let steam = abs(r"Program Files (x86)\Steam");
    env.launchers.push(launcher("steam", Some(&steam), &[]));
    let shaders = steam.join("steamapps").join("shadercache");
    assert_eq!(
        PathTemplate::from_path(&shaders, &env).as_str(),
        r"{STEAM}\steamapps\shadercache"
    );
    for path in [shaders, local(r"NVIDIA\DXCache")] {
        let v = verdict(&path, &env, &[]);
        assert_eq!(v.as_ref().map(|v| v.category), Some(Category::Cache));
        assert_eq!(v.map(|v| v.confidence), Some(0.9));
    }
}
