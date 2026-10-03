//! Built-in rules of SPEC-04 §4.7.8–§4.7.9 (T-04-09: cloud claims, games
//! outside the Ludusavi manifest and emulators) on `fixtures/fs/profile-games`:
//! every rule is embedded, the globs pick the right files of real layouts,
//! Steam rules give one finding per account, drive-root folders are found
//! with their drive letter, and each rule file gives the expected findings,
//! claimed paths and issues (one snapshot per file).

// Helpers outside `#[test]` functions are not covered by `allow-unwrap-in-tests`.
#![allow(clippy::unwrap_used)]

mod common;

use std::collections::BTreeSet;
use std::path::PathBuf;

use common::{finding_rules, findings_of, load_file, measure_finding, root, s, snapshot, under};
use sk_core::collector::CollectOutput;
use sk_core::env::{Environment, InstalledProgram, KnownFolder, ProgramSource};
use sk_core::model::{Category, Finding, Sensitivity, Target};
use sk_core::registry::MemRegistry;
use sk_core::template::{PathTemplate, ResolveContext};
use sk_rules::{RuleSet, RuleSource};
use sk_scan::MemFs;

const KIB: u64 = 1024;

const CLOUD: &str = include_str!("../../../rules/cloud-claims.yaml");
const GAMES: &str = include_str!("../../../rules/games-extra.yaml");
const EMULATORS: &str = include_str!("../../../rules/emulators.yaml");

/// The rule files of this task with the ids of their rules (SPEC-04 §4.7.8–§4.7.9).
const GROUPS: [(&str, &str, &[&str]); 3] = [
    (
        "cloud-claims.yaml",
        CLOUD,
        &[
            "onedrive.none",
            "dropbox.none",
            "google-drive.none",
            "icloud.none",
            "yandex-disk.none",
        ],
    ),
    (
        "games-extra.yaml",
        GAMES,
        &[
            "steam.userdata-config",
            "steam.screenshots",
            "minecraft.java",
            "prismlauncher.instances",
        ],
    ),
    (
        "emulators.yaml",
        EMULATORS,
        &[
            "retroarch.saves",
            "dolphin.user",
            "pcsx2.user",
            "ppsspp.user",
            "yuzu-ryujinx.user",
            "duckstation.user",
            "rpcs3.user",
            "cemu.user",
        ],
    ),
];

/// Rules of [`GROUPS`] that give findings on the fixture. Cloud rules only
/// claim; RPCS3 lives at a drive root, which the fixture cannot describe.
const WITH_FINDINGS: &[&str] = &[
    "cemu.user",
    "dolphin.user",
    "duckstation.user",
    "minecraft.java",
    "pcsx2.user",
    "ppsspp.user",
    "prismlauncher.instances",
    "retroarch.saves",
    "steam.screenshots",
    "steam.userdata-config",
    "yuzu-ryujinx.user",
];

/// Package family name of iCloud for Windows from the Microsoft Store.
const ICLOUD_PACKAGE: &str = "AppleInc.iCloud_nzyj5cx40ttqa";

/// The fixture plus what it cannot describe: the iCloud Store package.
fn setup() -> (MemFs, Environment, MemRegistry) {
    let (mut fs, mut env) = sk_testkit::mem_fixture("profile-games", &root());
    let cache = under(
        &env,
        KnownFolder::LocalAppData,
        &format!("Packages/{ICLOUD_PACKAGE}/LocalCache/Local/Apple Inc/CloudKit/db.sqlite"),
    );
    fs.add_file(&s(&cache), 512 * KIB, "-1d", None);
    env.store_packages = vec![ICLOUD_PACKAGE.to_owned()];
    (fs, env, MemRegistry::new())
}

/// Runs `set` through `RulesCollector` on the fixture.
async fn collect(set: RuleSet) -> CollectOutput {
    let (fs, env, registry) = setup();
    common::collect(set, fs, env, registry).await
}

/// Root template of a file-set or file finding.
fn template(finding: &Finding) -> &str {
    match &finding.target {
        Target::FileSet { root, .. } => root.as_str(),
        Target::File { path, .. } => path.as_str(),
        other => panic!("not a path target: {other:?}"),
    }
}

/// Absolute path of a template without multi-valued tokens.
fn resolved(env: &Environment, template: &str) -> PathBuf {
    let parsed = PathTemplate::parse(template).unwrap();
    parsed.resolve(env, &ResolveContext::default()).remove(0)
}

/// Every rule of §4.7.8–§4.7.9 is built in, from its file.
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

/// The embedded set gives findings exactly from the expected rules; Steam
/// rules give one finding per account that has the folder; the Switch keys
/// are `credentials` with sensitivity `high` and carry the warning note;
/// cloud caches are claimed but the synced Dropbox folder is not.
#[tokio::test]
async fn builtin_set_on_profile_games() {
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

    let templates =
        |rule: &str| -> Vec<&str> { findings_of(&out, rule).into_iter().map(template).collect() };
    assert_eq!(
        templates("steam.userdata-config"),
        [
            r"{STEAM}\userdata\12345678\config",
            r"{STEAM}\userdata\87654321\config"
        ]
    );
    assert_eq!(
        templates("steam.screenshots"),
        [r"{STEAM}\userdata\12345678\760\remote"]
    );

    let keys: Vec<&Finding> = findings_of(&out, "yuzu-ryujinx.user")
        .into_iter()
        .filter(|f| f.category == Category::Credentials)
        .collect();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].sensitivity, Sensitivity::High);
    assert_eq!(
        keys[0].title,
        "rules.yuzu_ryujinx.user — rules.yuzu_ryujinx.label_ryujinx_keys"
    );
    assert_eq!(
        keys[0].notes_key.as_deref(),
        Some("rules.yuzu_ryujinx.notes")
    );

    let (_, env, _) = setup();
    let claimed: BTreeSet<&PathBuf> = out.claimed_paths.iter().collect();
    let local = |rel: &str| under(&env, KnownFolder::LocalAppData, rel);
    let home = |rel: &str| under(&env, KnownFolder::Home, rel);
    for path in [
        local("Microsoft/OneDrive"),
        local("Dropbox"),
        home("Dropbox/.dropbox.cache"),
        local("Google/DriveFS"),
        local(&format!("Packages/{ICLOUD_PACKAGE}")),
    ] {
        assert!(claimed.contains(&path), "{} is not claimed", s(&path));
    }
    for path in [
        home("Dropbox"),
        under(&env, KnownFolder::AppData, "Yandex/YandexDisk2"),
    ] {
        assert!(!claimed.contains(&path), "{} is claimed", s(&path));
    }
}

/// The include and exclude globs pick the right files of the real layouts:
/// Minecraft without logs and the downloaded game, emulators without BIOS
/// images, caches, logs and firmware, the Ryujinx keys without other files
/// of its `system` folder.
#[tokio::test]
async fn builtin_file_sets_pick_their_files() {
    let out = collect(RuleSet::builtin().unwrap()).await;
    let (fs, env, _) = setup();

    let cases: [(&str, &[(u64, u64)]); 11] = [
        ("steam.userdata-config", &[(1, 16 * KIB), (3, 250 * KIB)]),
        ("steam.screenshots", &[(4, 840 * KIB)]),
        ("minecraft.java", &[(7, 3383 * KIB)]),
        ("prismlauncher.instances", &[(2, 5 * KIB)]),
        ("retroarch.saves", &[(4, 503 * KIB)]),
        ("dolphin.user", &[(4, 2132 * KIB)]),
        ("pcsx2.user", &[(3, 11272 * KIB)]),
        (
            "ppsspp.user",
            &[(1, 128 * KIB), (1, 2048 * KIB), (2, 8 * KIB)],
        ),
        ("yuzu-ryujinx.user", &[(1, 256 * KIB), (2, 12 * KIB)]),
        ("duckstation.user", &[(3, 4236 * KIB)]),
        ("cemu.user", &[(1, 512 * KIB)]),
    ];
    for (rule, expected) in cases {
        let mut sizes: Vec<(u64, u64)> = findings_of(&out, rule)
            .into_iter()
            .map(|f| measure_finding(&fs, &env, f))
            .collect();
        sizes.sort_unstable();
        assert_eq!(sizes, expected, "{rule}");
    }
}

/// A RetroArch folder at a drive root counts only when RetroArch is
/// installed (its Roaming folder is absent here); the finding gets the drive
/// letter and the cores next to it are claimed.
#[tokio::test]
async fn retroarch_at_a_drive_root_needs_the_installed_program() {
    let env = Environment::fake(&root());
    let install = r"{DRIVE:C}\RetroArch-Win64";
    let make_fs = || {
        let mut fs = MemFs::new();
        for (rel, size) in [
            (r"\retroarch.cfg", 100 * KIB),
            (r"\saves\Super Mario World.srm", 2 * KIB),
            (r"\cores\snes9x_libretro.dll", 4096 * KIB),
        ] {
            let path = resolved(&env, &format!("{install}{rel}"));
            fs.add_file(&s(&path), size, "-1d", None);
        }
        fs
    };

    let out = common::collect(
        load_file("emulators.yaml", EMULATORS),
        make_fs(),
        env.clone(),
        MemRegistry::new(),
    )
    .await;
    assert!(findings_of(&out, "retroarch.saves").is_empty());
    assert_eq!(out.claimed_paths, Vec::<PathBuf>::new());

    let mut installed = env.clone();
    installed.installed_programs.push(InstalledProgram {
        name: "RetroArch 1.19.1".to_owned(),
        publisher: Some("libretro".to_owned()),
        version: Some("1.19.1".to_owned()),
        install_location: Some(resolved(&env, install)),
        install_date: None,
        estimated_size_kb: None,
        source: ProgramSource::Hkcu,
        uninstall_key: "RetroArch".to_owned(),
    });
    let out = common::collect(
        load_file("emulators.yaml", EMULATORS),
        make_fs(),
        installed.clone(),
        MemRegistry::new(),
    )
    .await;
    let findings = findings_of(&out, "retroarch.saves");
    assert_eq!(
        findings.iter().map(|f| template(f)).collect::<Vec<_>>(),
        [install]
    );
    assert_eq!(
        measure_finding(&make_fs(), &installed, findings[0]),
        (2, 102 * KIB)
    );
    let cores = resolved(&env, &format!(r"{install}\cores"));
    assert!(
        out.claimed_paths.contains(&cores),
        "{:?}",
        out.claimed_paths
    );
}

/// Portable RPCS3 folders at a drive root give one finding per emulator
/// user; a file with `rpcs3` in the name is not a folder and does not match.
#[tokio::test]
async fn rpcs3_at_a_drive_root() {
    let env = Environment::fake(&root());
    let mut fs = MemFs::new();
    for (template, size) in [
        (
            r"{DRIVE:C}\rpcs3-v0.0.33-win64\dev_hdd0\home\00000001\savedata\BLUS30443\PARAM.SFO",
            4 * KIB,
        ),
        (
            r"{DRIVE:C}\rpcs3-v0.0.33-win64\dev_hdd0\home\00000002\savedata\NPUB30910\SAVE.DAT",
            64 * KIB,
        ),
        (
            r"{DRIVE:C}\rpcs3-v0.0.33-win64\dev_hdd0\home\00000003\trophy\NPWR00001\TROPCONF.SFM",
            KIB,
        ),
        (r"{DRIVE:C}\rpcs3-v0.0.33-win64.7z", 40 * 1024 * KIB),
    ] {
        fs.add_file(&s(&resolved(&env, template)), size, "-1d", None);
    }

    let out = common::collect(
        load_file("emulators.yaml", EMULATORS),
        fs,
        env,
        MemRegistry::new(),
    )
    .await;
    let templates: Vec<&str> = findings_of(&out, "rpcs3.user")
        .into_iter()
        .map(template)
        .collect();
    assert_eq!(
        templates,
        [
            r"{DRIVE:C}\rpcs3-v0.0.33-win64\dev_hdd0\home\00000001\savedata",
            r"{DRIVE:C}\rpcs3-v0.0.33-win64\dev_hdd0\home\00000002\savedata",
        ]
    );
    assert_eq!(out.issues, vec![]);
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
