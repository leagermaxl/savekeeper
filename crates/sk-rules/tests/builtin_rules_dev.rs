//! Built-in rules of SPEC-04 §4.7.1–§4.7.2 (T-04-07: browsers and the
//! developer environment, including `unityhub.projects`) on
//! `fixtures/fs/profile-dev`: every rule is embedded, the globs pick the
//! right files of real layouts, and each rule file gives the expected
//! findings, claimed paths and issues (one snapshot per file).

// Helpers outside `#[test]` functions are not covered by `allow-unwrap-in-tests`.
#![allow(clippy::unwrap_used)]

mod common;

use std::collections::BTreeSet;

use common::{finding_rules, findings_of, load_file, measure_finding, root, s, snapshot, under};
use serde_json::{json, Map, Value};
use sk_core::collector::CollectOutput;
use sk_core::env::{Environment, KnownFolder};
use sk_core::model::{Category, IssueSeverity, RegHive, Sensitivity};
use sk_rules::{MemRegistry, RuleSet, RuleSource};
use sk_scan::MemFs;

const KIB: u64 = 1024;

/// The rule files of this task with the ids of their rules (SPEC-04 §4.7.1–§4.7.2).
const GROUPS: [(&str, &str, &[&str]); 2] = [
    (
        "browsers.yaml",
        include_str!("../../../rules/browsers.yaml"),
        &[
            "chrome.profiles",
            "edge.profiles",
            "brave.profiles",
            "vivaldi.profiles",
            "opera.profiles",
            "yandex.profiles",
            "firefox.profiles",
            "thunderbird.profiles",
        ],
    ),
    (
        "dev.yaml",
        include_str!("../../../rules/dev.yaml"),
        &[
            "ssh.keys",
            "git.config",
            "vscode.user-settings",
            "vscode.extensions-list",
            "vscode-insiders.user-settings",
            "cursor.user-settings",
            "jetbrains.config",
            "visualstudio.settings",
            "windows-terminal.settings",
            "powershell.profile",
            "npm.config",
            "cargo.config",
            "docker.config",
            "aws.config",
            "kube.config",
            "putty.sessions",
            "winscp.config",
            "dbeaver.config",
            "unityhub.projects",
        ],
    ),
];

/// Rules of [`GROUPS`] that give findings on the fixture; Brave, Vivaldi and
/// VS Code Insiders are absent there.
const WITH_FINDINGS: &[&str] = &[
    "aws.config",
    "cargo.config",
    "chrome.profiles",
    "cursor.user-settings",
    "dbeaver.config",
    "docker.config",
    "edge.profiles",
    "firefox.profiles",
    "git.config",
    "jetbrains.config",
    "kube.config",
    "npm.config",
    "opera.profiles",
    "powershell.profile",
    "putty.sessions",
    "ssh.keys",
    "thunderbird.profiles",
    "unityhub.projects",
    "visualstudio.settings",
    "vscode.extensions-list",
    "vscode.user-settings",
    "windows-terminal.settings",
    "winscp.config",
    "yandex.profiles",
];

/// The Unity Hub project list: a project of the fixture and one that was
/// deleted from the disk.
fn unity_projects(env: &Environment) -> String {
    let mut data = Map::new();
    for name in ["Space Game", "Deleted Project"] {
        let path = s(&under(env, KnownFolder::Home, &format!("Unity/{name}")));
        let parent = s(&under(env, KnownFolder::Home, "Unity"));
        let entry = json!({
            "title": name,
            "lastModified": 1727800000000_u64,
            "isCustomEditor": false,
            "path": path,
            "containingFolderPath": parent,
            "version": "2022.3.45f1",
            "architecture": "x86_64",
            "isFavorite": false,
        });
        data.insert(path, entry);
    }
    json!({ "schema_version": "v1", "data": Value::Object(data) }).to_string()
}

/// The fixture plus what it cannot describe: the Unity Hub project list with
/// absolute paths, the Windows Terminal Store package, running browsers and
/// the registry keys of PuTTY and the VS Code protocol handler.
fn setup() -> (MemFs, Environment, MemRegistry) {
    let (mut fs, mut env) = sk_testkit::mem_fixture("profile-dev", &root());
    let config = unity_projects(&env);
    let config_path = under(&env, KnownFolder::AppData, "UnityHub/projects-v1.json");
    let bytes = config.as_bytes();
    fs.add_file(&s(&config_path), bytes.len() as u64, "-1d", Some(bytes));

    env.store_packages = vec!["Microsoft.WindowsTerminal_8wekyb3d8bbwe".to_owned()];
    env.running_processes = vec![
        "explorer.exe".to_owned(),
        "chrome.exe".to_owned(),
        "firefox.exe".to_owned(),
    ];
    let mut registry = MemRegistry::new();
    registry
        .add_key(
            RegHive::Hkcu,
            "Software\\SimonTatham\\PuTTY\\Sessions\\Default%20Settings",
        )
        .add_key(
            RegHive::Hkcu,
            "Software\\Classes\\vscode\\shell\\open\\command",
        );
    (fs, env, registry)
}

/// Runs `set` through `RulesCollector` on the fixture.
async fn collect(set: RuleSet) -> CollectOutput {
    let (fs, env, registry) = setup();
    common::collect(set, fs, env, registry).await
}

/// Every rule of §4.7.1–§4.7.2 is built in, from its file.
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
/// groups. Running Chrome and Firefox tag their findings `app-running`.
/// Browser profiles are sensitive and carry their note; keys and cloud
/// credentials are `credentials` with sensitivity `high`. The only issue is
/// the Unity project deleted from the disk.
#[tokio::test]
async fn builtin_set_on_profile_dev() {
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

    for finding in &out.findings {
        let app = finding.app.as_ref().map(|a| a.id.as_str()).unwrap();
        let running = finding.tags.iter().any(|t| t == "app-running");
        assert_eq!(
            running,
            matches!(app, "chrome" | "firefox"),
            "{}",
            finding.title
        );
        if finding.category == Category::Credentials {
            assert_eq!(finding.sensitivity, Sensitivity::High, "{}", finding.title);
        }
    }

    for rule in GROUPS[0].2 {
        for finding in findings_of(&out, rule) {
            let app = &finding.app.as_ref().unwrap().id;
            assert_eq!(finding.category, Category::AppData, "{rule}");
            assert_eq!(finding.sensitivity, Sensitivity::High, "{rule}");
            assert_eq!(
                finding.notes_key.as_deref(),
                Some(format!("rules.{app}.notes").as_str()),
                "{rule}"
            );
        }
    }

    let credentials: BTreeSet<&str> = out
        .findings
        .iter()
        .filter(|f| f.category == Category::Credentials)
        .map(|f| f.title.as_str())
        .collect();
    let expected: BTreeSet<&str> = [
        "rules.aws.config",
        "rules.cargo.config — rules.cargo.label_credentials",
        "rules.kube.config",
        "rules.ssh.keys",
    ]
    .into();
    assert_eq!(credentials, expected);

    assert_eq!(out.issues.len(), 1, "{:?}", out.issues);
    let issue = &out.issues[0];
    assert_eq!(issue.severity, IssueSeverity::Info);
    assert_eq!(issue.message_key, "issue.rules.from_json_skipped");
    assert_eq!(issue.message_args["rule_id"], "unityhub.projects");
    assert_eq!(issue.message_args["reason"], "missing");
}

/// The include and exclude globs pick the right files of the real layouts:
/// every Chromium profile without its caches and cookies, both Opera layouts,
/// the Firefox profile files of the spec (with extensions and the running
/// session), the whole Thunderbird profile without `cache2`, one finding per
/// JetBrains IDE version without the service entries next to them, the
/// Visual Studio instance, Windows Terminal from its Store package and a
/// Unity project without the folders the editor regenerates.
#[tokio::test]
async fn builtin_file_sets_pick_their_files() {
    let out = collect(RuleSet::builtin().unwrap()).await;
    let (fs, env, _) = setup();

    let cases: [(&str, &[(u64, u64)]); 15] = [
        ("chrome.profiles", &[(13, 2237 * KIB)]),
        ("edge.profiles", &[(2, 6 * KIB)]),
        ("opera.profiles", &[(3, 263 * KIB), (4, 34 * KIB)]),
        ("yandex.profiles", &[(3, 30 * KIB)]),
        ("firefox.profiles", &[(8, 8782 * KIB + 512)]),
        ("thunderbird.profiles", &[(4, 10257 * KIB + 256)]),
        ("ssh.keys", &[(4, 411 + 98 + 2 * KIB + 300)]),
        ("vscode.user-settings", &[(3, 6 * KIB)]),
        ("cursor.user-settings", &[(1, KIB)]),
        (
            "jetbrains.config",
            &[(2, 3 * KIB), (2, 4 * KIB), (2, 5 * KIB)],
        ),
        ("visualstudio.settings", &[(1, 120 * KIB)]),
        ("windows-terminal.settings", &[(2, 9 * KIB)]),
        ("powershell.profile", &[(2, 6 * KIB)]),
        ("dbeaver.config", &[(2, 5 * KIB)]),
        ("unityhub.projects", &[(3, 98 * KIB)]),
    ];
    for (rule, expected) in cases {
        let mut sizes: Vec<(u64, u64)> = findings_of(&out, rule)
            .into_iter()
            .filter(|f| !matches!(f.target, sk_core::model::Target::Registry { .. }))
            .map(|f| measure_finding(&fs, &env, f))
            .collect();
        sizes.sort_unstable();
        assert_eq!(sizes, expected, "{rule}");
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
        let name = format!("builtin_{}", file.trim_end_matches(".yaml"));
        insta::assert_json_snapshot!(name, snapshot(&out));
    }
}
