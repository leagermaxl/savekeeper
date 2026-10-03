//! `enrich`: all detectors into `Environment.launchers` (SPEC-05 T-05-05).

use std::collections::BTreeSet;

use sk_core::env::LauncherInfo;
use sk_core::model::ScanIssue;

use super::enrich::{enrich, enrich_with_registry};
use super::steam::ISSUE_LIBRARYFOLDERS;
use super::test_support::{keys, Setup};

fn launcher(id: &str) -> LauncherInfo {
    LauncherInfo {
        id: id.to_owned(),
        root: None,
        user_ids: Vec::new(),
        games: Vec::new(),
    }
}

fn ids(launchers: &[LauncherInfo]) -> Vec<&str> {
    launchers.iter().map(|l| l.id.as_str()).collect()
}

/// Steam without `libraryfolders.vdf`, Epic without games and an Xbox package
/// with `wgs`.
fn installed() -> Setup {
    let mut setup = Setup::new();
    setup
        .dir("Program Files (x86)/Steam")
        .dir("ProgramData/Epic/EpicGamesLauncher")
        .dir("Users/user/AppData/Local/Packages/Studio.Game_abc/SystemAppData/wgs");
    setup
}

#[test]
fn fills_launchers_and_returns_detector_issues() {
    let mut setup = installed();
    // A launcher of another id is kept; a stale entry of a detector that finds
    // nothing now is removed.
    setup.env.launchers = vec![launcher("custom"), launcher("gog")];
    let registry = setup.registry();
    let issues = enrich_with_registry(&mut setup.env, &setup.fs, registry);

    assert_eq!(
        ids(&setup.env.launchers),
        ["custom", "steam", "epic", "xbox"]
    );
    let steam = &setup.env.launchers[1];
    assert_eq!(
        steam.root.as_deref(),
        Some(setup.path("Program Files (x86)/Steam").as_path())
    );
    assert_eq!(
        keys(&issues, "games.steam"),
        [(ISSUE_LIBRARYFOLDERS, "not_found")]
    );
}

#[test]
fn repeated_call_gives_the_same_result() {
    let mut setup = installed();
    let registry = setup.registry();
    let first = enrich_with_registry(&mut setup.env, &setup.fs, registry.clone());
    let launchers = setup.env.launchers.clone();
    let second = enrich_with_registry(&mut setup.env, &setup.fs, registry);
    assert_eq!(setup.env.launchers, launchers);
    // Issue paths are templates of the env seen by the detectors: `{STEAM}`
    // only once Steam is in it, so only the keys are compared.
    let key = |i: &ScanIssue| {
        (
            i.source.clone(),
            i.message_key.clone(),
            i.message_args.clone(),
        )
    };
    assert_eq!(
        second.iter().map(key).collect::<Vec<_>>(),
        first.iter().map(key).collect::<Vec<_>>()
    );
}

#[test]
fn nothing_installed_gives_no_launchers_and_no_issues() {
    let mut setup = Setup::new();
    let registry = setup.registry();
    let issues = enrich_with_registry(&mut setup.env, &setup.fs, registry);
    assert!(setup.env.launchers.is_empty());
    assert!(issues.is_empty());
}

/// `enrich` reads the registry of this machine, so only what does not depend
/// on it is checked here.
#[test]
fn enrich_with_the_system_registry_keeps_other_launchers() {
    let mut setup = Setup::new();
    setup.env.launchers = vec![launcher("custom")];
    enrich(&mut setup.env, &setup.fs);
    let all = ids(&setup.env.launchers);
    assert_eq!(all.first(), Some(&"custom"));
    let unique: BTreeSet<&str> = all.iter().copied().collect();
    assert_eq!(unique.len(), all.len());
    if !cfg!(windows) {
        assert_eq!(all, ["custom"]);
    }
}
