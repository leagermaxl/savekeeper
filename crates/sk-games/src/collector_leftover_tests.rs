//! Tests of the games collector: leftovers of games that are not installed,
//! registry entries, unmatched games, merging and launcher findings
//! (SPEC-05 §4.5 step 4, §4.7 steps 3, 4, 8, §5).

use sk_core::model::{EvidenceSource, IssueSeverity, RegHive, Target};
use sk_core::CancellationToken;
use time::{Date, Month};

use super::*;
use crate::manifest::{ManifestMeta, ManifestSource};

#[test]
fn leftovers_of_games_that_are_not_installed() {
    let yaml = r"
Hollow Knight:
  files:
    <winLocalAppDataLow>/Team Cherry/Hollow Knight/*.dat: { tags: [save] }
  registry:
    HKEY_CURRENT_USER/Software/Team Cherry/Hollow Knight: { tags: [config] }
  steam: { id: 367520 }
Terraria:
  files:
    <winDocuments>/My Games/Terraria/*.plr: { tags: [save] }
";
    let mut setup = Setup::new();
    setup
        .file("{LOCALLOW}/Team Cherry/Hollow Knight/user1.dat")
        .file("{DOCUMENTS}/My Games/Terraria/readme.txt");
    setup
        .registry
        .add_key(RegHive::Hkcu, r"Software\Team Cherry\Hollow Knight");
    let out = setup.run(yaml);
    assert_eq!(
        templates(&out),
        [
            r"{LOCALLOW}\Team Cherry\Hollow Knight",
            r"Software\Team Cherry\Hollow Knight"
        ]
    );
    let save = &out.findings[0];
    assert_eq!(save.tags, ["not-installed"]);
    assert_eq!(include(save), ["*.dat", "*.dat/**"]);
    assert_eq!(save.evidence.len(), 1);
    let app = save.app.as_ref().unwrap_or_else(|| panic!("no app"));
    assert_eq!(app.installed, Some(false));
    assert_eq!(app.id, "hollow-knight");
    let reg = &out.findings[1];
    assert_eq!(reg.category, Category::GameConfig);
    assert_eq!(
        reg.target,
        Target::Registry {
            hive: RegHive::Hkcu,
            key: r"Software\Team Cherry\Hollow Knight".to_owned(),
            recursive: true,
        }
    );
    assert_eq!(
        out.claimed_paths,
        [setup.path("{LOCALLOW}/Team Cherry/Hollow Knight")]
    );
}

#[test]
fn hklm_keys_are_reported_for_installed_games_only() {
    let yaml = r"
Game:
  registry:
    HKEY_LOCAL_MACHINE/SOFTWARE/Studio/Game: { tags: [config] }
    HKEY_CURRENT_USER/Software/Studio/Game: { tags: [config] }
  installDir:
    Game: {}
Other:
  registry:
    HKEY_LOCAL_MACHINE/SOFTWARE/Studio/Other: {}
";
    let mut setup = Setup::new();
    setup.game("gog", "1", "Game", "{PROGRAMFILES}/GOG Games/Game");
    setup
        .registry
        .add_key(RegHive::Hklm, r"SOFTWARE\Studio\Game")
        .add_key(RegHive::Hklm, r"SOFTWARE\Studio\Other");
    let out = setup.run(yaml);
    assert_eq!(out.findings.len(), 1, "only the install folder");
    assert_eq!(out.issues.len(), 1);
    let issue = &out.issues[0];
    assert_eq!(issue.severity, IssueSeverity::Info);
    assert_eq!(issue.source, "games");
    assert_eq!(issue.message_key, "issue.games.registry_hklm_skipped");
    assert_eq!(issue.message_args["game"], "Game");
    assert_eq!(issue.message_args["key"], r"HKLM\SOFTWARE\Studio\Game");
}

#[test]
fn registry_paths_are_parsed() {
    use super::registry::parse_key;
    assert_eq!(
        parse_key("HKEY_CURRENT_USER/Software//Game/"),
        Some((RegHive::Hkcu, r"Software\Game".to_owned()))
    );
    assert_eq!(
        parse_key(r"HKEY_LOCAL_MACHINE\SOFTWARE\Game"),
        Some((RegHive::Hklm, r"SOFTWARE\Game".to_owned()))
    );
    assert_eq!(parse_key("HKEY_CLASSES_ROOT/Game"), None);
    assert_eq!(parse_key("HKEY_CURRENT_USER"), None);
    assert_eq!(parse_key("HKEY_CURRENT_USER/Software/<storeUserId>"), None);
}

#[test]
fn unmatched_game_keeps_its_folder_unclaimed() {
    let mut setup = Setup::new();
    setup.steam(&[("1", "A")]).game(
        "steam",
        "999",
        "Indie Thing",
        "{PROGRAMFILES_X86}/Steam/steamapps/common/IndieThing",
    );
    setup
        .dir("{STEAM}/userdata/1")
        .dir("{STEAM}/config")
        .file("{STEAM}/steam.exe")
        .file("{STEAM}/steamapps/appmanifest_999.acf");
    let out = setup.run(ELDEN);
    assert_eq!(out.findings.len(), 1);
    let install = &out.findings[0];
    assert_eq!(install.category, Category::Reinstallable);
    assert_eq!(install.tags, ["steam", "game-unmatched"]);
    assert_eq!(install.title, "Indie Thing — games.title.install_dir");
    let app = install.app.as_ref().unwrap_or_else(|| panic!("no app"));
    assert_eq!(app.id, "indie-thing");
    assert_eq!(app.source_ids["steam"], "999");
    // The Steam folder is claimed, except `userdata` and `steamapps\common`.
    assert_eq!(
        out.claimed_paths,
        [
            setup.path("{STEAM}/config"),
            setup.path("{STEAM}/steam.exe"),
            setup.path("{STEAM}/steamapps/appmanifest_999.acf"),
        ]
    );
}

#[test]
fn a_path_of_several_games_is_one_finding() {
    let yaml = r"
Alpha:
  files:
    <winAppData>/Shared Engine/Saves: { tags: [save] }
Beta:
  files:
    <winAppData>/Shared Engine/Saves: { tags: [save] }
";
    let mut setup = Setup::new();
    setup.file("{APPDATA}/Shared Engine/Saves/a.sav");
    let out = setup.run(yaml);
    assert_eq!(out.findings.len(), 1);
    let finding = &out.findings[0];
    assert_eq!(finding.tags, ["not-installed", "multi-game"]);
    let games: Vec<&EvidenceSource> = finding.evidence.iter().map(|e| &e.source).collect();
    assert_eq!(games.len(), 2);
    assert_eq!(finding.app.as_ref().map(|a| a.name.as_str()), Some("Alpha"));
}

#[test]
fn launcher_findings_are_added() {
    let mut setup = Setup::new();
    let ubisoft = setup.path("{PROGRAMFILES_X86}/Ubisoft/Ubisoft Game Launcher");
    setup.dir("{PROGRAMFILES_X86}/Ubisoft/Ubisoft Game Launcher/savegames/abc/1");
    setup.launcher("ubisoft").root = Some(ubisoft.clone());
    let out = setup.run("{}");
    assert_eq!(out.findings.len(), 1);
    assert_eq!(
        out.findings[0].evidence[0].message_key,
        "evidence.games.ubisoft_savegames"
    );
    assert!(out.claimed_paths.contains(&ubisoft));
}

#[test]
fn cancelled_scan_fails() {
    let mut setup = Setup::new();
    setup.game("epic", "Fox", "Game", "{PROGRAMFILES}/Epic Games/Game");
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        setup.try_run(ELDEN, &cancel),
        Err(GamesError::Cancelled)
    ));
}

#[test]
fn manifest_version_names_the_manifest() {
    let meta = |source, etag: Option<&str>, fetched_at| ManifestMeta {
        source,
        etag: etag.map(str::to_owned),
        fetched_at,
        games: 1,
    };
    let embedded = ManifestSource::Embedded {
        snapshot_date: "2026-10-01".to_owned(),
    };
    assert_eq!(manifest_version(&meta(embedded, None, None)), "2026-10-01");
    assert_eq!(
        manifest_version(&meta(ManifestSource::Cache, Some("\"abc\""), None)),
        "\"abc\""
    );
    let date =
        Date::from_calendar_date(2026, Month::September, 30).unwrap_or_else(|e| panic!("{e}"));
    let at = Some(date.midnight().assume_utc());
    assert_eq!(
        manifest_version(&meta(ManifestSource::Cache, None, at)),
        "2026-09-30"
    );
    assert_eq!(
        manifest_version(&meta(ManifestSource::Downloaded, None, None)),
        "unknown"
    );
}

/// Time of a collection over the real manifest with 200 installed games on
/// an empty file system (manual check of NFR-05-02):
/// `cargo test -p sk-games --release real_manifest_collect -- --ignored --nocapture`.
#[test]
#[ignore = "parses the full embedded-snapshot source (~17 MB)"]
fn real_manifest_collect() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../third_party/ludusavi/manifest.yaml");
    let yaml = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let manifest =
        Manifest::parse(yaml.as_bytes(), ManifestSource::Cache).unwrap_or_else(|e| panic!("{e}"));
    let mut keys: Vec<&String> = manifest.games.keys().collect();
    keys.sort_unstable();
    let mut setup = Setup::new();
    setup.steam(&[("1", "A"), ("2", "B")]);
    for (i, key) in keys.iter().step_by(keys.len() / 200).take(200).enumerate() {
        let dir = format!("{{PROGRAMFILES_X86}}/Steam/steamapps/common/Game{i}");
        setup.game("steam", &i.to_string(), key, &dir);
    }
    let started = std::time::Instant::now();
    let scan = Scan {
        manifest: &manifest,
        version: "real".to_owned(),
        env: &setup.env,
        fs: &setup.fs,
        registry: &setup.registry,
        cancel: &CancellationToken::new(),
        max_depth: 32,
    };
    let out = run(&scan).unwrap_or_else(|e| panic!("{e}"));
    println!(
        "{} findings, {} claimed in {:?}",
        out.findings.len(),
        out.claimed_paths.len(),
        started.elapsed()
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}
