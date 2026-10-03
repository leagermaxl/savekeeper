use std::collections::HashMap;

use sk_core::env::{KnownFolder, LauncherInfo};
use sk_scan::{MemFs, MemFsCalls};

use super::*;
use crate::manifest::{FileRule, GameEntry, ManifestMeta, ManifestSource};

const MINI: &str = include_str!("../../../fixtures/samples/ludusavi/manifest-mini.yaml");

fn manifest(yaml: &str) -> Manifest {
    Manifest::parse(yaml.as_bytes(), ManifestSource::Cache).unwrap_or_else(|e| panic!("{e}"))
}

fn root() -> PathBuf {
    PathBuf::from(if cfg!(windows) { r"C:\fake" } else { "/fake" })
}

fn env() -> Environment {
    Environment::fake(&root())
}

/// `rel` (`/`-separated) under a known folder of [`env`].
fn at(folder: KnownFolder, rel: &str) -> PathBuf {
    let env = env();
    let base = env
        .known_folder(folder)
        .unwrap_or_else(|| panic!("{folder:?}"))
        .to_path_buf();
    rel.split('/').fold(base, |p, c| p.join(c))
}

fn s(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn dir(fs: &mut MemFs, folder: KnownFolder, rel: &str) {
    fs.add_dir(&s(&at(folder, rel)));
}

fn file(fs: &mut MemFs, folder: KnownFolder, rel: &str) {
    fs.add_file(&s(&at(folder, rel)), 1, "-1d", None);
}

fn launcher(id: &str) -> LauncherInfo {
    LauncherInfo {
        id: id.to_owned(),
        root: None,
        user_ids: Vec::new(),
        games: Vec::new(),
    }
}

fn find<'a>(
    index: &'a AnchorIndex,
    fs: &MemFs,
    env: &Environment,
    installed: &[&str],
) -> Vec<AnchorHit<'a>> {
    let installed: HashSet<&str> = installed.iter().copied().collect();
    index
        .find(fs, env, &installed, &CancellationToken::new())
        .unwrap_or_else(|e| panic!("{e}"))
}

/// `(game key, manifest path)` of each hit.
fn keys(hits: &[AnchorHit<'_>]) -> Vec<(String, String)> {
    hits.iter()
        .map(|hit| (hit.rule.key.clone(), hit.rule.path.clone()))
        .collect()
}

fn pair(key: &str, path: &str) -> (String, String) {
    (key.to_owned(), path.to_owned())
}

fn sorted_anchors(index: &AnchorIndex) -> Vec<&str> {
    let mut anchors: Vec<&str> = index.anchors.keys().map(String::as_str).collect();
    anchors.sort_unstable();
    anchors
}

#[test]
fn mini_manifest_anchors() {
    let index = AnchorIndex::new(&manifest(MINI));
    // Entries with `<base>` / `<root>` (Celeste, Deep Rock Galactic, Portal 2,
    // Slay the Spire, Subnautica) are not indexed.
    assert_eq!(index.rules.len(), 26);
    assert!(index.wide.is_empty());
    assert_eq!(
        sorted_anchors(&index),
        [
            r"{appdata}\darksoulsiii",
            r"{appdata}\eldenring",
            r"{appdata}\factorio\config",
            r"{appdata}\factorio\player-data.json",
            r"{appdata}\factorio\saves",
            r"{appdata}\sekiro",
            r"{appdata}\stardewvalley\saves",
            r"{appdata}\stardewvalley\startup_preferences",
            r"{documents}\my games\skyrim special edition",
            r"{documents}\my games\terraria",
            r"{documents}\rockstar games\gta v",
            r"{documents}\rockstar games\red dead redemption 2",
            r"{documents}\saved games\hades",
            r"{localappdata}\cd projekt red\cyberpunk 2077",
            r"{localappdata}\factorygame\saved",
            r"{localappdata}\hogwarts legacy\saved",
            r"{localappdata}\larian studios\baldur's gate 3",
            r"{locallow}\team cherry\hollow knight",
            r"{saved_games}\cd projekt red\cyberpunk 2077",
        ]
    );
    let bases: Vec<&str> = index.bases.keys().map(String::as_str).collect();
    assert_eq!(
        bases,
        [
            "{appdata}",
            "{documents}",
            "{localappdata}",
            "{locallow}",
            "{saved_games}"
        ]
    );
    let hk = &index.rules[index.anchors[r"{locallow}\team cherry\hollow knight"][0]];
    assert_eq!(hk.key, "Hollow Knight");
    assert_eq!(
        hk.template.as_str(),
        r"{LOCALLOW}\Team Cherry\Hollow Knight"
    );
    assert_eq!(hk.include, ["*.dat", "*.dat/**"]);
}

/// A machine where several games of the mini manifest were removed but left
/// their saves, plus unrelated folders.
fn leftovers() -> MemFs {
    let mut fs = MemFs::new();
    // Other case than in the manifest.
    file(
        &mut fs,
        KnownFolder::AppData,
        "ELDENRING/76561197960287930/ER0000.sl2",
    );
    file(&mut fs, KnownFolder::AppData, "Factorio/config/config.ini");
    dir(&mut fs, KnownFolder::AppData, "Microsoft/Windows");
    file(
        &mut fs,
        KnownFolder::Documents,
        "My Games/Skyrim Special Edition/Saves/save1.ess",
    );
    file(
        &mut fs,
        KnownFolder::Documents,
        "My Games/Skyrim Special Edition/Skyrim.ini",
    );
    // Anchor exists, but only one of the three Terraria entries does.
    file(
        &mut fs,
        KnownFolder::Documents,
        "My Games/Terraria/Players/p.plr",
    );
    dir(&mut fs, KnownFolder::Documents, "My Games/Other Game");
    file(
        &mut fs,
        KnownFolder::LocalLow,
        "Team Cherry/Hollow Knight/user1.dat",
    );
    file(
        &mut fs,
        KnownFolder::SavedGames,
        "CD Projekt Red/Cyberpunk 2077/save.dat",
    );
    // Publisher folders without the game: listed, no candidate.
    dir(&mut fs, KnownFolder::LocalAppData, "CD Projekt Red");
    dir(&mut fs, KnownFolder::LocalAppData, "Larian Studios");
    dir(&mut fs, KnownFolder::LocalAppData, "Temp/x");
    fs
}

#[test]
fn mini_manifest_finds_leftover_saves_with_few_exists_calls() {
    let index = AnchorIndex::new(&manifest(MINI));
    let fs = leftovers();
    let env = env();
    let hits = find(&index, &fs, &env, &[]);
    assert_eq!(
        keys(&hits),
        [
            pair(
                "Cyberpunk 2077",
                "<home>/Saved Games/CD Projekt Red/Cyberpunk 2077"
            ),
            pair("ELDEN RING", "<winAppData>/EldenRing"),
            pair("Factorio", "<winAppData>/Factorio/config/config.ini"),
            pair(
                "Hollow Knight",
                "<winLocalAppDataLow>/Team Cherry/Hollow Knight/*.dat"
            ),
            pair("Terraria", "<winDocuments>/My Games/Terraria/Players"),
            pair(
                "The Elder Scrolls V: Skyrim Special Edition",
                "<winDocuments>/My Games/Skyrim Special Edition/*.ini"
            ),
            pair(
                "The Elder Scrolls V: Skyrim Special Edition",
                "<winDocuments>/My Games/Skyrim Special Edition/Saves"
            ),
        ]
    );
    assert_eq!(hits[1].paths, [at(KnownFolder::AppData, "EldenRing")]);
    assert_eq!(
        hits[3].paths,
        [at(KnownFolder::LocalLow, "Team Cherry/Hollow Knight")]
    );

    // Only the entries with an existing anchor are checked: the 7 hits and
    // the two other Terraria entries, out of 26 indexed entries.
    let calls = fs.calls();
    assert_eq!(
        calls,
        MemFsCalls {
            // 5 roots + EldenRing-free nested folders: Factorio, My Games,
            // CD Projekt Red (twice), Larian Studios, Team Cherry.
            read_dir: 11,
            exists: 9,
            read_head: 0,
        }
    );
    assert!(calls.exists < index.rules.len() as u64);
}

#[test]
fn installed_games_and_when_conditions_are_not_checked() {
    let yaml = r"
Alias Game:
  alias: Steam Only
  files:
    <winAppData>/AliasGame: {}
Installed Game:
  files:
    <winAppData>/InstalledGame: {}
Linux Only:
  files:
    <winAppData>/LinuxOnly:
      when:
        - os: linux
Prime Only:
  files:
    <winAppData>/PrimeOnly:
      when:
        - store: prime
Steam Only:
  files:
    <winAppData>/SteamOnly:
      when:
        - os: windows
          store: steam
Wide Game:
  files:
    <winAppData>/*/WideSaves: {}
";
    let index = AnchorIndex::new(&manifest(yaml));
    let rules: Vec<(&str, &str)> = index
        .rules
        .iter()
        .map(|r| (r.key.as_str(), r.path.as_str()))
        .collect();
    assert_eq!(
        rules,
        [
            ("Installed Game", "<winAppData>/InstalledGame"),
            ("Steam Only", "<winAppData>/SteamOnly"),
            ("Wide Game", "<winAppData>/*/WideSaves"),
        ]
    );
    let wide: Vec<&str> = index.wide().map(|r| r.key.as_str()).collect();
    assert_eq!(wide, ["Wide Game"]);
    assert_eq!(
        sorted_anchors(&index),
        [r"{appdata}\installedgame", r"{appdata}\steamonly"]
    );

    let mut fs = MemFs::new();
    for name in [
        "AliasGame",
        "InstalledGame",
        "LinuxOnly",
        "PrimeOnly",
        "SteamOnly",
    ] {
        dir(&mut fs, KnownFolder::AppData, name);
    }
    dir(&mut fs, KnownFolder::AppData, "Some/WideSaves");

    let mut env = env();
    assert!(find(&index, &fs, &env, &["Installed Game"]).is_empty());
    assert_eq!(fs.calls().exists, 0);

    env.launchers.push(launcher("steam"));
    let hits = find(&index, &fs, &env, &["Installed Game"]);
    assert_eq!(keys(&hits), [pair("Steam Only", "<winAppData>/SteamOnly")]);
    let hits = find(&index, &fs, &env, &[]);
    assert_eq!(hits.len(), 2);
}

#[test]
fn drive_and_windows_roots_are_listed_too() {
    let yaml = r"
Old Game:
  files:
    D:/Games/OldGame/save: {}
Win Game:
  files:
    <winDir>/WinGame.ini: {}
";
    let index = AnchorIndex::new(&manifest(yaml));
    assert_eq!(
        sorted_anchors(&index),
        [r"d:\games\oldgame", r"{windir}\wingame.ini"]
    );
    let mut fs = MemFs::new();
    fs.add_dir(r"D:\Games\OldGame\save");
    file(&mut fs, KnownFolder::WinDir, "WinGame.ini");
    let hits = find(&index, &fs, &env(), &[]);
    assert_eq!(
        keys(&hits),
        [
            pair("Old Game", "D:/Games/OldGame/save"),
            pair("Win Game", "<winDir>/WinGame.ini"),
        ]
    );
    assert_eq!(fs.calls().exists, 2);
}

/// 20 000 games with three entries each, 10 of them left saves: the scan
/// costs a few dozen calls instead of 60 000 `exists`.
#[test]
fn exists_calls_do_not_grow_with_the_manifest() {
    const GAMES: usize = 20_000;
    let mut games = HashMap::new();
    for n in 0..GAMES {
        let files = [
            format!("<winAppData>/Game {n}/Saves"),
            format!("<winDocuments>/My Games/Game {n}"),
            format!("<winLocalAppDataLow>/Studio {n}/Game {n}/*.sav"),
        ]
        .into_iter()
        .map(|path| (path, FileRule::default()))
        .collect();
        let entry = GameEntry {
            files,
            ..GameEntry::default()
        };
        games.insert(format!("Game {n}"), entry);
    }
    let manifest = Manifest {
        games,
        meta: ManifestMeta {
            source: ManifestSource::Cache,
            etag: None,
            fetched_at: None,
            games: GAMES,
        },
    };
    let index = AnchorIndex::new(&manifest);
    assert_eq!(index.rules.len(), 3 * GAMES);

    let mut fs = MemFs::new();
    let left: Vec<usize> = (0..10).map(|i| i * 1999).collect();
    for &n in &left {
        file(
            &mut fs,
            KnownFolder::AppData,
            &format!("Game {n}/Saves/1.sav"),
        );
        dir(
            &mut fs,
            KnownFolder::Documents,
            &format!("My Games/Game {n}"),
        );
        file(
            &mut fs,
            KnownFolder::LocalLow,
            &format!("Studio {n}/Game {n}/1.sav"),
        );
    }
    for n in 0..100 {
        dir(&mut fs, KnownFolder::AppData, &format!("Unrelated {n}"));
        dir(
            &mut fs,
            KnownFolder::LocalAppData,
            &format!("Unrelated {n}"),
        );
    }
    let hits = find(&index, &fs, &env(), &[]);
    assert_eq!(hits.len(), 3 * left.len());

    let calls = fs.calls();
    // One `exists` per entry with an existing anchor.
    assert_eq!(calls.exists, 30);
    // 3 roots + 10 game folders in AppData + My Games + 10 studio folders.
    assert_eq!(calls.read_dir, 24);
    assert!(calls.exists + calls.read_dir <= 100, "{calls:?}");
}

#[test]
fn cancelled_scan_stops_before_listing() {
    let index = AnchorIndex::new(&manifest(MINI));
    let fs = leftovers();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let result = index.find(&fs, &env(), &HashSet::new(), &cancel);
    assert!(matches!(result, Err(GamesError::Cancelled)), "{result:?}");
    assert_eq!(fs.calls(), MemFsCalls::default());
}

#[test]
fn missing_roots_give_no_hits() {
    let index = AnchorIndex::new(&manifest(MINI));
    let fs = MemFs::new();
    assert!(find(&index, &fs, &env(), &[]).is_empty());
    let calls = fs.calls();
    assert_eq!(calls.read_dir, 5);
    assert_eq!(calls.exists, 0);
}

/// Statistics of the index of the real manifest (manual check of SPEC-05
/// §4.6): `cargo test -p sk-games real_manifest_index -- --ignored --nocapture`.
#[test]
#[ignore = "parses the full embedded-snapshot source (~17 MB)"]
fn real_manifest_index() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/ludusavi/manifest.yaml");
    let yaml = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let manifest = Manifest::parse(&yaml, ManifestSource::Cache).unwrap_or_else(|e| panic!("{e}"));
    let index = AnchorIndex::new(&manifest);
    let nested: usize = index.bases.values().map(|b| b.nested.len()).sum();
    println!(
        "games {}, indexed entries {}, anchors {}, wide {}, roots {:?}, nested folders {nested}",
        manifest.games.len(),
        index.rules.len(),
        index.anchors.len(),
        index.wide.len(),
        index.bases.keys().collect::<Vec<_>>(),
    );
    let largest = index.anchors.iter().max_by_key(|(_, v)| v.len());
    println!("largest anchor: {:?}", largest.map(|(k, v)| (k, v.len())));
    assert!(index.bases.len() < 40);
}
