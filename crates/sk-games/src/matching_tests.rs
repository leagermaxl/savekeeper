use std::path::PathBuf;

use sk_core::env::{InstalledGame, LauncherInfo};

use super::*;
use crate::manifest::ManifestSource;

const MINI: &str = include_str!("../../../fixtures/samples/ludusavi/manifest-mini.yaml");

fn manifest(yaml: &str) -> Manifest {
    Manifest::parse(yaml.as_bytes(), ManifestSource::Cache).unwrap_or_else(|e| panic!("{e}"))
}

fn index(yaml: &str) -> MatchIndex {
    MatchIndex::new(&manifest(yaml))
}

fn game(id: &str, name: &str, dir: &str) -> InstalledGame {
    InstalledGame {
        store_game_id: id.to_owned(),
        name: name.to_owned(),
        install_dir: PathBuf::from("Games").join(dir),
        size_bytes: None,
        manifest_key: None,
    }
}

fn matched(
    index: &MatchIndex,
    launcher: &str,
    g: &InstalledGame,
) -> Option<(String, f32, MatchBy)> {
    index
        .match_game(launcher, g)
        .map(|m| (m.key, m.confidence, m.by))
}

fn full(key: &str, by: MatchBy) -> Option<(String, f32, MatchBy)> {
    Some((key.to_owned(), FULL_CONFIDENCE, by))
}

fn ambiguous(key: &str, by: MatchBy) -> Option<(String, f32, MatchBy)> {
    Some((key.to_owned(), AMBIGUOUS_CONFIDENCE, by))
}

#[test]
fn name_key_follows_app_ref_normalization() {
    assert_eq!(name_key("ELDEN RING™"), "elden-ring");
    assert_eq!(name_key("ELDEN RING"), "elden-ring");
    assert_eq!(
        name_key("Sekiro™ Shadows Die Twice"),
        "sekiro-shadows-die-twice"
    );
    assert_eq!(name_key("Game™Name"), "gamename");
    assert_eq!(
        name_key("The Elder Scrolls V: Skyrim Special Edition"),
        "the-elder-scrolls-v-skyrim-special-edition"
    );
    assert_eq!(
        name_key("  Half-Life_2 : Episode®  "),
        "half-life-2-episode"
    );
    assert_eq!(name_key("Ведьмак 3"), "ведьмак-3");
    assert_eq!(name_key("™ - ©"), "");
    assert_eq!(name_key(""), "");
}

#[test]
fn steam_games_match_by_app_id() {
    let index = index(MINI);
    // The launcher name and folder do not matter when the id is known.
    let g = game("1245620", "Something Else", "Other");
    assert_eq!(
        matched(&index, "steam", &g),
        full("ELDEN RING", MatchBy::StoreId)
    );
    let g = game("489830", "Skyrim SE", "Skyrim Special Edition");
    assert_eq!(
        matched(&index, "steam", &g),
        full(
            "The Elder Scrolls V: Skyrim Special Edition",
            MatchBy::StoreId
        )
    );
}

#[test]
fn gog_games_match_by_product_id() {
    let index = index(MINI);
    let g = game("1456460669", "BG3", "Baldurs Gate 3");
    assert_eq!(
        matched(&index, "gog", &g),
        full("Baldur's Gate 3", MatchBy::StoreId)
    );
    // A GOG id is not a Steam id: an Epic game with the same id is matched by name.
    let g = game("1456460669", "Unknown", "Unknown");
    assert_eq!(matched(&index, "epic", &g), None);
}

#[test]
fn extra_ids_match_after_primary_ones() {
    let yaml = "\
Main:
  steam: { id: 10 }
  gog: { id: 100 }
Bundle:
  steam: { id: 20 }
  id: { steamExtra: [10, 30], gogExtra: [100, 300] }
";
    let index = index(yaml);
    let g = |id: &str| game(id, "x", "x");
    assert_eq!(
        matched(&index, "steam", &g("10")),
        full("Main", MatchBy::StoreId)
    );
    assert_eq!(
        matched(&index, "steam", &g("30")),
        full("Bundle", MatchBy::StoreId)
    );
    assert_eq!(
        matched(&index, "gog", &g("100")),
        full("Main", MatchBy::StoreId)
    );
    assert_eq!(
        matched(&index, "gog", &g("300")),
        full("Bundle", MatchBy::StoreId)
    );
    assert_eq!(
        matched(&index, "steam", &g(" 20 ")),
        full("Bundle", MatchBy::StoreId)
    );
}

#[test]
fn unknown_store_id_falls_back_to_name_and_folder() {
    let index = index(MINI);
    let g = game("999999", "ELDEN RING", "Somewhere");
    assert_eq!(
        matched(&index, "steam", &g),
        full("ELDEN RING", MatchBy::Name)
    );
    let g = game("not-a-number", "Unknown", "Hollow Knight");
    assert_eq!(
        matched(&index, "gog", &g),
        full("Hollow Knight", MatchBy::InstallDir)
    );
}

#[test]
fn other_launchers_match_by_normalized_name() {
    let index = index(MINI);
    let g = game("Fortnite", "ELDEN RING™", "Elden");
    assert_eq!(
        matched(&index, "epic", &g),
        full("ELDEN RING", MatchBy::Name)
    );
    let g = game("a1", "Sekiro: Shadows Die Twice", "Sekiro Game");
    assert_eq!(
        matched(&index, "epic", &g),
        full("Sekiro™ Shadows Die Twice", MatchBy::Name)
    );
    // Steam ids are not looked at for other launchers.
    let g = game("1245620", "Nothing Like It", "Nothing");
    assert_eq!(matched(&index, "epic", &g), None);
}

#[test]
fn install_folder_matches_install_dir_case_insensitively() {
    let index = index(MINI);
    let g = game("x", "Skyrim SE (Anniversary)", "skyrim special EDITION");
    assert_eq!(
        matched(&index, "ea", &g),
        full(
            "The Elder Scrolls V: Skyrim Special Edition",
            MatchBy::InstallDir
        )
    );
}

#[test]
fn ambiguous_names_prefer_matching_install_dir() {
    let yaml = "\
\"Doom: Eternal\":
  installDir: { DOOMEternal: {} }
Doom Eternal:
  installDir: { Doom Eternal: {} }
DOOM ETERNAL™:
  installDir: { Other: {} }
";
    let index = index(yaml);
    let g = game("e", "DOOM Eternal", "doometernal");
    assert_eq!(
        matched(&index, "epic", &g),
        full("Doom: Eternal", MatchBy::Name)
    );
    let g = game("e", "DOOM Eternal", "Doom Eternal");
    assert_eq!(
        matched(&index, "epic", &g),
        full("Doom Eternal", MatchBy::Name)
    );
    // No installDir matches: the first key in byte order, with lower confidence.
    let g = game("e", "DOOM Eternal", "Elsewhere");
    assert_eq!(
        matched(&index, "epic", &g),
        ambiguous("DOOM ETERNAL™", MatchBy::Name)
    );
}

#[test]
fn several_entries_with_the_same_install_dir_are_ambiguous() {
    let yaml = "\
B Game:
  installDir: { Shared: {} }
A Game:
  installDir: { Shared: {} }
Same:
  installDir: { Shared: {} }
Same™:
  installDir: { Shared: {} }
";
    let index = index(yaml);
    let g = game("x", "Unrelated", "Shared");
    assert_eq!(
        matched(&index, "ubisoft", &g),
        ambiguous("A Game", MatchBy::InstallDir)
    );
    // Name candidates that all match the install folder stay ambiguous.
    let g = game("x", "SAME", "shared");
    assert_eq!(
        matched(&index, "ubisoft", &g),
        ambiguous("Same", MatchBy::Name)
    );
}

#[test]
fn ambiguous_store_ids_use_the_same_tie_break() {
    let yaml = "\
Zeta:
  steam: { id: 5 }
  installDir: { Zeta: {} }
Alpha:
  steam: { id: 5 }
";
    let index = index(yaml);
    let g = game("5", "x", "Zeta");
    assert_eq!(matched(&index, "steam", &g), full("Zeta", MatchBy::StoreId));
    let g = game("5", "x", "x");
    assert_eq!(
        matched(&index, "steam", &g),
        ambiguous("Alpha", MatchBy::StoreId)
    );
}

#[test]
fn aliases_lead_to_their_target() {
    let yaml = "\
Real Game:
  steam: { id: 1 }
Alt Title:
  alias: Real Game
  steam: { id: 2 }
  installDir: { AltDir: {} }
Dangling:
  alias: Missing Game
Chain:
  alias: Alt Title
";
    let index = index(yaml);
    let g = game("e", "ALT TITLE", "x");
    assert_eq!(
        matched(&index, "epic", &g),
        full("Real Game", MatchBy::Name)
    );
    // Aliases have no data of their own: no ids, no installDir.
    assert_eq!(matched(&index, "steam", &game("2", "x", "x")), None);
    assert_eq!(matched(&index, "epic", &game("e", "x", "AltDir")), None);
    // A missing target or an alias of an alias matches nothing.
    assert_eq!(matched(&index, "epic", &game("e", "Dangling", "x")), None);
    assert_eq!(matched(&index, "epic", &game("e", "Chain", "x")), None);
}

#[test]
fn a_name_and_its_alias_are_one_candidate() {
    let yaml = "\
Game™:
  alias: Game
Game:
  installDir: { G: {} }
";
    let index = index(yaml);
    assert_eq!(
        matched(&index, "epic", &game("e", "GAME", "x")),
        full("Game", MatchBy::Name)
    );
}

#[test]
fn names_without_letters_or_digits_never_match() {
    let yaml = "\
\"™\":
  installDir: { X: {} }
";
    let index = index(yaml);
    assert_eq!(matched(&index, "epic", &game("e", "®", "y")), None);
    assert_eq!(matched(&index, "epic", &game("e", "", "")), None);
}

#[test]
fn unmatched_games_give_none() {
    let index = index(MINI);
    let g = game("123", "Totally Unknown Game", "TUG");
    for launcher in ["steam", "epic", "gog", "ubisoft", "ea", "battlenet"] {
        assert_eq!(matched(&index, launcher, &g), None, "{launcher}");
    }
    assert_eq!(matched(&MatchIndex::new(&manifest("")), "steam", &g), None);
}

#[test]
fn result_does_not_depend_on_hash_map_order() {
    let yaml = "\
C: { installDir: { D: {} } }
A: { installDir: { D: {} } }
B: { installDir: { D: {} } }
";
    for _ in 0..16 {
        let g = game("x", "x", "D");
        assert_eq!(
            matched(&index(yaml), "epic", &g),
            ambiguous("A", MatchBy::InstallDir)
        );
    }
}

#[test]
fn annotate_fills_manifest_keys() {
    let index = index(MINI);
    let mut stale = game("x", "Unknown", "Unknown");
    stale.manifest_key = Some("Stale".to_owned());
    let mut launchers = vec![
        LauncherInfo {
            id: "steam".to_owned(),
            root: None,
            user_ids: Vec::new(),
            games: vec![game("1245620", "ELDEN RING", "ELDEN RING"), stale],
        },
        LauncherInfo {
            id: "epic".to_owned(),
            root: None,
            user_ids: Vec::new(),
            games: vec![game("Hades", "Hades", "Hades")],
        },
    ];
    index.annotate(&mut launchers);
    let keys: Vec<Option<&str>> = launchers
        .iter()
        .flat_map(|l| &l.games)
        .map(|g| g.manifest_key.as_deref())
        .collect();
    assert_eq!(keys, [Some("ELDEN RING"), None, Some("Hades")]);
}
