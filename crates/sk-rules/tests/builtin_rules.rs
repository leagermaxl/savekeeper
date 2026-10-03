//! Built-in rules of SPEC-04 §4.7.3–§4.7.7 (T-04-08) on
//! `fixtures/fs/profile-apps`: every rule is embedded, and each rule file
//! gives the expected findings, claimed paths and issues (one snapshot per
//! file, SPEC-04 §8 «покрыты snapshot-тестом хотя бы по группам»).

// Helpers outside `#[test]` functions are not covered by `allow-unwrap-in-tests`.
#![allow(clippy::unwrap_used)]

mod common;

use std::collections::BTreeSet;

use common::{finding_rules, findings_of, load_file, measure_finding, root, s, snapshot, under};
use sk_core::collector::CollectOutput;
use sk_core::env::{Environment, KnownFolder};
use sk_core::model::{RegHive, Target};
use sk_core::registry::MemRegistry;
use sk_rules::{RuleSet, RuleSource};
use sk_scan::MemFs;

/// The rule files of this task with the ids of their rules (SPEC-04 §4.7.3–§4.7.7).
const GROUPS: [(&str, &str, &[&str]); 5] = [
    (
        "media-streaming.yaml",
        include_str!("../../../rules/media-streaming.yaml"),
        &[
            "obs.config",
            "sharex.config",
            "sharex.screenshots",
            "vlc.config",
            "mpc-hc.settings",
            "foobar2000.config",
            "spotify.none",
        ],
    ),
    (
        "messengers.yaml",
        include_str!("../../../rules/messengers.yaml"),
        &[
            "telegram.tdata",
            "discord.settings",
            "slack.none",
            "whatsapp.none",
            "skype.none",
        ],
    ),
    (
        "productivity.yaml",
        include_str!("../../../rules/productivity.yaml"),
        &[
            "obsidian.config",
            "obsidian.vaults",
            "keepass.config",
            "keepassxc.config",
            "notepadpp.config",
            "sublime.config",
            "office.templates",
            "outlook.pst",
            "autohotkey.scripts",
            "sevenzip.settings",
            "total-commander.config",
            "everything.config",
            "qbittorrent.config",
            "figma.settings",
            "adobe.settings",
        ],
    ),
    (
        "hardware-tuning.yaml",
        include_str!("../../../rules/hardware-tuning.yaml"),
        &[
            "msi-afterburner.profiles",
            "rivatuner.profiles",
            "rainmeter.skins",
            "wallpaper-engine.config",
            "logitech-ghub.settings",
            "razer-synapse.none",
        ],
    ),
    (
        "windows-shell.yaml",
        include_str!("../../../rules/windows-shell.yaml"),
        &[
            "windows.start-taskbar-pins",
            "windows.sendto",
            "windows.explorer-quickaccess",
            "windows.sticky-notes",
            "windows.snipping-screenshots",
            "windows.powertoys",
        ],
    ),
];

/// Rules of [`GROUPS`] that give findings on the fixture; the others find
/// nothing there (the application is absent or keeps everything in the cloud).
const WITH_FINDINGS: &[&str] = &[
    "adobe.settings",
    "discord.settings",
    "everything.config",
    "foobar2000.config",
    "keepassxc.config",
    "mpc-hc.settings",
    "msi-afterburner.profiles",
    "notepadpp.config",
    "obs.config",
    "obsidian.config",
    "obsidian.vaults",
    "outlook.pst",
    "qbittorrent.config",
    "rainmeter.skins",
    "sevenzip.settings",
    "sharex.config",
    "sharex.screenshots",
    "sublime.config",
    "telegram.tdata",
    "vlc.config",
    "wallpaper-engine.config",
    "windows.explorer-quickaccess",
    "windows.powertoys",
    "windows.sendto",
    "windows.snipping-screenshots",
    "windows.start-taskbar-pins",
    "windows.sticky-notes",
];

/// The fixture plus what it cannot describe: the Obsidian config with the
/// absolute path of its vault, the Store packages, running processes and the
/// registry keys of MPC-HC and 7-Zip.
fn setup() -> (MemFs, Environment, MemRegistry) {
    let (mut fs, mut env) = sk_testkit::mem_fixture("profile-apps", &root());
    let vault = under(&env, KnownFolder::Documents, "Notes");
    let config = format!(
        r#"{{"vaults":{{"8f2c1a":{{"path":{},"ts":1727800000000,"open":true}}}}}}"#,
        serde_json::to_string(&s(&vault)).unwrap()
    );
    let config_path = under(&env, KnownFolder::AppData, "obsidian/obsidian.json");
    let bytes = config.as_bytes();
    fs.add_file(&s(&config_path), bytes.len() as u64, "-1d", Some(bytes));

    env.store_packages = vec![
        "5319275A.WhatsAppDesktop_cv1g1gvanyjgm".to_owned(),
        "Microsoft.MicrosoftStickyNotes_8wekyb3d8bbwe".to_owned(),
    ];
    env.running_processes = vec![
        "explorer.exe".to_owned(),
        "obs64.exe".to_owned(),
        "Telegram.exe".to_owned(),
    ];
    let mut registry = MemRegistry::new();
    registry
        .add_key(RegHive::Hkcu, "Software\\MPC-HC\\MPC-HC\\Settings")
        .add_key(RegHive::Hkcu, "Software\\7-Zip\\FM");
    (fs, env, registry)
}

/// Runs `set` through `RulesCollector` on the fixture.
async fn collect(set: RuleSet) -> CollectOutput {
    let (fs, env, registry) = setup();
    common::collect(set, fs, env, registry).await
}

/// Every rule of §4.7.3–§4.7.7 is built in, from its file.
#[test]
fn builtin_set_has_every_rule_of_the_groups() {
    let set = RuleSet::builtin().unwrap();
    for (file, _, ids) in GROUPS {
        for id in ids {
            assert!(set.get(id).is_some(), "built-in rule {id} is missing");
            assert_eq!(
                set.source(id),
                Some(&RuleSource::Builtin {
                    file: file.to_owned()
                }),
                "{id}"
            );
        }
    }
}

/// The embedded set gives findings exactly from the expected rules of the
/// groups; with OBS and Telegram running they are tagged `app-running`.
#[tokio::test]
async fn builtin_set_on_profile_apps() {
    let out = collect(RuleSet::builtin().unwrap()).await;
    let ours: BTreeSet<&str> = GROUPS
        .iter()
        .flat_map(|(_, _, ids)| *ids)
        .copied()
        .collect();
    let found: BTreeSet<String> = finding_rules(&out)
        .into_iter()
        .filter(|id| ours.contains(id.as_str()))
        .collect();
    let expected: BTreeSet<String> = WITH_FINDINGS.iter().map(|id| (*id).to_owned()).collect();
    assert_eq!(found, expected);
    assert_eq!(out.issues, vec![]);

    for finding in &out.findings {
        let running = finding.tags.iter().any(|t| t == "app-running");
        let app = finding.app.as_ref().map(|a| a.id.as_str());
        assert_eq!(
            running,
            matches!(app, Some("obs" | "telegram")),
            "{}",
            finding.title
        );
    }
}

/// The include and exclude globs of file-set rules pick the right files:
/// PowerToys keeps every module JSON (Keyboard Manager, FancyZones) but not
/// logs or downloaded updates; Sticky Notes keeps the WAL database with its
/// `-wal` and `-shm` files. Global exclusions are off, so only the rule's
/// own globs decide.
#[tokio::test]
async fn builtin_file_sets_pick_their_files() {
    let out = collect(RuleSet::builtin().unwrap()).await;
    let (fs, env, _) = setup();

    let cases = [
        ("windows.powertoys", 4, (2 + 3 + 1 + 1) * 1024),
        ("windows.sticky-notes", 3, (96 + 32 + 32) * 1024),
    ];
    for (rule, files, bytes) in cases {
        let finding = findings_of(&out, rule)[0];
        assert!(matches!(finding.target, Target::FileSet { .. }), "{rule}");
        assert_eq!(
            measure_finding(&fs, &env, finding),
            (files, bytes),
            "{rule}"
        );
    }
}

/// One snapshot per rule file: findings, claimed paths and issues of its
/// rules alone, loaded as they would be from `rules.d`.
#[tokio::test]
async fn builtin_groups_snapshots() {
    for (file, text, ids) in GROUPS {
        let set = load_file(file, text);
        assert_eq!(set.len(), ids.len(), "{file}");

        let out = collect(set).await;
        let name = format!(
            "builtin_{}",
            file.trim_end_matches(".yaml").replace('-', "_")
        );
        insta::assert_json_snapshot!(name, snapshot(&out));
    }
}
