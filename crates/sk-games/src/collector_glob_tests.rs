//! Folder globs and context values in finding templates (T-05-13).

use std::sync::Arc;

use sk_scan::{measure, DirStatsCache, ExcludeSet, MeasureOptions};

use super::*;

/// Files `measure` counts for a finding, as the Measure phase does.
fn measured_files(setup: &Setup, finding: &Finding) -> u64 {
    let opts = MeasureOptions::new(Arc::new(ExcludeSet::builtin(&setup.env)), 32);
    let stats = measure(
        &setup.fs,
        &finding.target,
        &DirStatsCache::new(),
        &opts,
        &CancellationToken::new(),
    );
    let stats = stats.unwrap_or_else(|e| panic!("{e}"));
    stats.map_or(0, |s| s.file_count)
}

#[test]
fn folder_globs_take_the_contents_of_matching_folders() {
    let yaml = r"
Game:
  files:
    <winAppData>/Game/*/saves: { tags: [save] }
    <winDocuments>/Game/**/Saved: { tags: [save] }
  installDir:
    Game: {}
";
    let mut setup = Setup::new();
    setup
        .game("epic", "Fox", "Game", "{PROGRAMFILES}/Epic Games/Game")
        .file("{APPDATA}/Game/profile1/saves/slot1.sav")
        .file("{APPDATA}/Game/profile1/saves/auto/slot2.sav")
        .file("{APPDATA}/Game/profile1/log.txt")
        .file("{DOCUMENTS}/Game/a/b/Saved/sub/x.sav")
        .file("{DOCUMENTS}/Game/Saved/y.sav")
        .file("{DOCUMENTS}/Game/other.txt");
    let out = setup.run(yaml);
    let roaming = by_template(&out, r"{APPDATA}\Game");
    assert_eq!(include(roaming), ["*/saves", "*/saves/**"]);
    assert_eq!(measured_files(&setup, roaming), 2);
    let docs = by_template(&out, r"{DOCUMENTS}\Game");
    assert_eq!(include(docs), ["**/Saved", "**/Saved/**"]);
    assert_eq!(measured_files(&setup, docs), 2);
}

#[test]
fn folder_glob_finds_leftovers_of_a_removed_game() {
    let yaml = r"
Old Game:
  files:
    <winAppData>/Old Studio/Old Game/*/saves: { tags: [save] }
";
    let mut setup = Setup::new();
    setup.file("{APPDATA}/Old Studio/Old Game/user/saves/1.sav");
    let out = setup.run(yaml);
    let save = by_template(&out, r"{APPDATA}\Old Studio\Old Game");
    assert_eq!(save.tags, ["not-installed"]);
    assert_eq!(measured_files(&setup, save), 1);
}

#[test]
fn game_and_store_game_id_are_text_in_finding_templates() {
    let yaml = r"
Alpha:
  files:
    <winAppData>/Engine/<game>/<storeGameId>_save: { tags: [save] }
  installDir:
    AlphaDir: {}
Beta:
  files:
    <winAppData>/Engine/<game>/<storeGameId>_save: { tags: [save] }
  installDir:
    BetaDir: {}
";
    let mut setup = Setup::new();
    setup
        .game("epic", "Fox", "Alpha", "{PROGRAMFILES}/Epic Games/AlphaDir")
        .game("epic", "Owl", "Beta", "{PROGRAMFILES}/Epic Games/BetaDir")
        .file("{APPDATA}/Engine/AlphaDir/Fox_save/s.sav")
        .file("{APPDATA}/Engine/BetaDir/Owl_save/s.sav");
    let out = setup.run(yaml);
    let alpha = by_template(&out, r"{APPDATA}\Engine\AlphaDir\Fox_save");
    let beta = by_template(&out, r"{APPDATA}\Engine\BetaDir\Owl_save");
    assert_ne!(alpha.id, beta.id);
    assert_eq!(alpha.id, FindingId::for_target(&alpha.target));
    assert!(!alpha.tags.iter().any(|t| t == "multi-game"));
    // The finding resolves without the game context.
    let Target::FileSet { root, resolved, .. } = &alpha.target else {
        panic!("{:?}", alpha.target);
    };
    let paths = root.resolve(&setup.env, &ResolveContext::default());
    assert_eq!(paths, std::slice::from_ref(resolved));
}
