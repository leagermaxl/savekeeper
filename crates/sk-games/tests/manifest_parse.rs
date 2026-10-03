//! Parsing of full manifest files (SPEC-05 §4.2, T-05-01): fixtures in
//! `fixtures/samples/ludusavi` and a large synthetic manifest.

#![allow(clippy::unwrap_used, clippy::expect_used)] // tests

#[path = "common/synthetic.rs"]
mod synthetic;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use sk_games::{
    CloudFlags, FileRule, GameEntry, GogRef, Manifest, ManifestSource, Os, SteamRef, Store, When,
};

fn sample(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/samples/ludusavi")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn parse_sample() -> Manifest {
    Manifest::parse(&sample("manifest-sample.yaml"), ManifestSource::Cache).unwrap()
}

fn when(os: Option<Os>, store: Option<Store>) -> When {
    When { os, store }
}

fn win() -> When {
    when(Some(Os::Windows), None)
}

#[test]
fn mini_manifest_has_20_games() {
    let m = Manifest::parse(&sample("manifest-mini.yaml"), ManifestSource::Downloaded).unwrap();
    assert_eq!(m.meta.games, 20);
    assert_eq!(m.games.len(), 20);
    assert_eq!(m.meta.source, ManifestSource::Downloaded);
    let elden = &m.games["ELDEN RING"];
    assert_eq!(elden.steam, Some(SteamRef { id: 1_245_620 }));
    assert_eq!(
        elden.files["<winAppData>/EldenRing"].tags,
        ["save", "config"]
    );
    let hk = &m.games["Hollow Knight"];
    assert_eq!(
        hk.registry["HKEY_CURRENT_USER/Software/Team Cherry/Hollow Knight"].tags,
        ["config"]
    );
    let portal = &m.games["Portal 2"].files["<root>/userdata/<storeUserId>/620/remote"];
    assert_eq!(portal.when, [when(None, Some(Store::Steam))]);
    assert!(m.games.contains_key("Sekiro™ Shadows Die Twice"));
}

#[test]
fn sample_counts_and_names() {
    let m = parse_sample();
    assert_eq!(m.meta.games, 34);
    assert_eq!(m.games.len(), 34);
    let aliases: Vec<_> = {
        let mut a: Vec<_> = m
            .games
            .iter()
            .filter(|(_, g)| g.is_alias())
            .map(|(k, g)| (k.as_str(), g.alias.as_deref().unwrap()))
            .collect();
        a.sort_unstable();
        a
    };
    assert_eq!(
        aliases,
        [
            ("Cyberpunk2077", "Cyberpunk 2077"),
            ("Elden Ring", "ELDEN RING"),
            ("Skyrim", "The Elder Scrolls V: Skyrim Special Edition"),
        ]
    );
    // Every alias points to an entry of the file.
    for (_, target) in &aliases {
        assert!(m.games.contains_key(*target), "{target}");
    }
    for name in [
        "1849",
        "2064: Read Only Memories",
        "Baldur's Gate 3",
        "Mount & Blade II: Bannerlord",
        "The Witcher 3: Wild Hunt",
        "Sekiro™ Shadows Die Twice",
        "Мор (Утопия)",
        "ファイナルファンタジーXIV",
    ] {
        assert!(m.games.contains_key(name), "{name}");
    }
    assert_eq!(m.games["Empty Game"], GameEntry::default());
}

#[test]
fn sample_full_entry() {
    let m = parse_sample();
    let g = &m.games["Hollow Knight"];
    assert_eq!(g.files.len(), 4);
    let dir = &g.files["<winLocalAppDataLow>/Team Cherry/Hollow Knight"];
    assert_eq!(dir.tags, ["save"]);
    assert_eq!(dir.when, [win()]);
    let glob = &g.files["<winLocalAppDataLow>/Team Cherry/Hollow Knight/*.dat"];
    assert!(glob.when.is_empty());
    assert_eq!(
        g.files["<home>/Library/Application Support/unity.Team Cherry.Hollow Knight"].when,
        [when(Some(Os::Mac), None)]
    );
    assert_eq!(
        g.files["<xdgConfig>/unity3d/Team Cherry/Hollow Knight"].when,
        [when(Some(Os::Linux), None)]
    );
    assert_eq!(g.install_dir.keys().collect::<Vec<_>>(), ["Hollow Knight"]);
    assert_eq!(g.steam, Some(SteamRef { id: 367_520 }));
    assert_eq!(g.gog, Some(GogRef { id: 1_308_320_804 }));
    assert_eq!(
        g.cloud,
        Some(CloudFlags {
            gog: true,
            steam: true,
            ..CloudFlags::default()
        })
    );
    let ids = g.id.clone().unwrap();
    assert_eq!(ids.flatpak.as_deref(), Some("com.teamcherry.HollowKnight"));
    assert!(ids.gog_extra.is_empty() && ids.steam_extra.is_empty());
    assert_eq!(g.alias, None);
}

#[test]
fn sample_when_variants() {
    let m = parse_sample();
    let bg3 = &m.games["Baldur's Gate 3"].files
        ["<winDocuments>/Larian Studios/Baldur's Gate 3/PlayerProfiles"];
    assert_eq!(
        bg3.when,
        [
            when(Some(Os::Windows), Some(Store::Gog)),
            when(Some(Os::Windows), Some(Store::GogGalaxy)),
        ]
    );
    let cache = &m.games["Baldur's Gate 3"].files
        ["<winLocalAppData>/Larian Studios/Baldur's Gate 3/LevelCache"];
    assert!(
        cache.tags.is_empty(),
        "no tags -> GameSave 0.7 later (FR-05-06)"
    );
    let doom = &m.games["DOOM (1993)"].files["<base>/*.dsg"];
    assert_eq!(doom.when, [when(Some(Os::Dos), None), win()]);

    let stores = |game: &str, path: &str| -> Vec<Option<Store>> {
        m.games[game].files[path]
            .when
            .iter()
            .map(|w| w.store.clone())
            .collect()
    };
    assert_eq!(
        stores(
            "Fortnite",
            "<winLocalAppData>/FortniteGame/Saved/Config/WindowsClient"
        ),
        [
            Some(Store::Epic),
            Some(Store::Legendary),
            Some(Store::Heroic)
        ]
    );
    assert_eq!(
        stores(
            "Mass Effect Legendary Edition",
            "<winDocuments>/BioWare/Mass Effect Legendary Edition/Save"
        ),
        [Some(Store::Ea), Some(Store::Origin), Some(Store::Steam)]
    );
    assert_eq!(
        stores("Prime Gaming Title", "<winAppData>/Prime Title/Saves"),
        [Some(Store::Prime), Some(Store::Other), Some(Store::Lutris)]
    );
    assert_eq!(
        stores(
            "Deep Rock Galactic",
            "<winLocalAppData>/Packages/CoffeeStainStudios.DeepRockGalactic_*/SystemAppData/wgs"
        ),
        [Some(Store::Microsoft)]
    );
    assert_eq!(
        stores("Far Cry 5", "<root>/savegames/<storeUserId>/1803"),
        [Some(Store::Uplay)]
    );
}

#[test]
fn sample_edge_cases() {
    let m = parse_sample();
    let g = &m.games["Tags And When Edge Cases"];
    for path in [
        "<base>/null-rule",
        "<base>/empty-rule",
        "<base>/null-fields",
    ] {
        assert_eq!(g.files[path], FileRule::default(), "{path}");
    }
    let flow = &g.files["<base>/flow-style"];
    assert_eq!(flow.tags, ["save", "config"]);
    assert_eq!(
        flow.when,
        [
            when(Some(Os::Windows), Some(Store::Steam)),
            when(None, Some(Store::Epic)),
        ]
    );
    let unknown = &g.files["<base>/unknown-values"];
    assert_eq!(unknown.tags, ["save", "futureTag"]);
    assert_eq!(
        unknown.when,
        [when(
            Some(Os::Unknown("android".to_owned())),
            Some(Store::Unknown("newStore".to_owned()))
        )]
    );
    assert!(g.files.contains_key("<base>/Game # 2/save"));
    assert!(g.files.contains_key("<game>/<storeGameId>/save.bin"));
    assert_eq!(g.gog, Some(GogRef { id: u64::MAX }));
    assert_eq!(g.registry.len(), 2);
    assert!(g
        .registry
        .values()
        .all(|r| r.tags.is_empty() && r.when.is_empty()));

    assert_eq!(
        m.games["Steam Only Id"].steam,
        Some(SteamRef { id: u32::MAX })
    );
    let no_ids = &m.games["No Store Ids"];
    assert_eq!((no_ids.steam, no_ids.gog), (None, None));
    assert!(no_ids.files.contains_key("<home>/.sharedgame/**/*.sav"));
    assert!(no_ids.files.contains_key("<osUserName>.profile"));
    // `..` is kept by the model; translate drops it (SPEC-05 §5).
    assert!(m.games["Slay the Spire"]
        .files
        .contains_key("<base>/../SlayTheSpire-shared/escape.dat"));
}

#[test]
fn sample_registry_ids_cloud_install_dir() {
    let m = parse_sample();
    let reg = &m.games["Registry Only Game"];
    assert!(reg.files.is_empty());
    assert_eq!(reg.registry.len(), 3);
    assert_eq!(
        reg.registry["HKEY_CURRENT_USER/Software/Registry Only Studio/Game"].tags,
        ["save"]
    );
    let hklm =
        &m.games["Far Cry 5"].registry["HKEY_LOCAL_MACHINE/SOFTWARE/WOW6432Node/Ubisoft/Far Cry 5"];
    assert_eq!(hklm.when, [when(None, Some(Store::Uplay))]);
    let hl2 =
        &m.games["Half-Life 2"].registry["HKEY_CURRENT_USER/Software/Valve/Half-Life 2/Settings"];
    assert_eq!(hl2.when, [when(Some(Os::Windows), Some(Store::Steam))]);

    let witcher = &m.games["The Witcher 3: Wild Hunt"];
    let ids = witcher.id.clone().unwrap();
    assert_eq!(ids.gog_extra, [1_207_664_643, 1_495_134_320]);
    assert_eq!(ids.steam_extra, [499_450]);
    assert_eq!(
        witcher.install_dir.keys().collect::<Vec<_>>(),
        ["The Witcher 3 Wild Hunt", "The Witcher 3 Wild Hunt GOTY"]
    );
    assert_eq!(m.games["DOOM (1993)"].install_dir.len(), 3);
    assert_eq!(
        m.games["2064: Read Only Memories"]
            .install_dir
            .keys()
            .collect::<Vec<_>>(),
        ["2064 Read Only Memories"]
    );

    let cloud = |g: &str| m.games[g].cloud.unwrap();
    assert!(cloud("Cyberpunk 2077").epic && cloud("Cyberpunk 2077").gog);
    assert!(cloud("Mass Effect Legendary Edition").origin);
    assert!(cloud("Far Cry 5").uplay);
    assert_eq!(cloud("DARK SOULS III"), CloudFlags::default());
    assert_eq!(m.games["Terraria"].gog, Some(GogRef { id: 1_207_665_503 }));
    assert_eq!(m.games["1849"].steam, Some(SteamRef { id: 1_037_920 }));
}

#[test]
fn synthetic_manifest_parses_completely_and_fast() {
    // ~4 MB, a tenth of the real manifest; debug build, so the bound is generous.
    let s = synthetic::synthetic_manifest(4 << 20);
    let start = Instant::now();
    let m = Manifest::parse(s.yaml.as_bytes(), ManifestSource::Cache).unwrap();
    let elapsed = start.elapsed();
    println!(
        "synthetic: {} bytes, {} games parsed in {:.3} s",
        s.yaml.len(),
        s.games,
        elapsed.as_secs_f64()
    );
    assert_eq!(m.meta.games, s.games);
    assert_eq!(m.games.values().filter(|g| g.is_alias()).count(), s.aliases);
    assert_eq!(
        m.games.values().map(|g| g.files.len()).sum::<usize>(),
        s.file_rules
    );
    let first = &m.games[&synthetic::game_name(0)];
    assert_eq!(first.steam, Some(SteamRef { id: 100_000 }));
    assert_eq!(first.gog, Some(GogRef { id: 1_200_000_000 }));
    assert_eq!(first.registry.len(), 1);
    assert_eq!(first.id.clone().unwrap().steam_extra, [200_000]);
    assert!(elapsed < Duration::from_secs(60), "{elapsed:?}");
}
