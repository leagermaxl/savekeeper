//! `ScanReport` contract and snapshots of the environment (SPEC-02 §6).

// Helpers outside `#[test]` functions are not covered by `allow-unwrap-in-tests`.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::json;
use sk_core::env::{
    CloudProvider, CloudRoot, DriveKind, Environment, InstalledGame, KnownFolder, LauncherInfo,
    OsInfo,
};
use sk_core::model::{
    Category, CategoryTotals, CollectorToggles, DriveSnapshot, EnvironmentSnapshot, Evidence,
    EvidenceSource, Finding, FindingId, IssueSeverity, LauncherSnapshot, LlmMode, ReportError,
    ScanIssue, ScanOptionsSnapshot, ScanReport, Sensitivity, Target, Totals,
};
use sk_core::template::PathTemplate;
use time::macros::datetime;
use uuid::Uuid;

fn tpl(s: &str) -> PathTemplate {
    PathTemplate::parse(s).unwrap()
}

fn report() -> ScanReport {
    let target = Target::FileSet {
        root: tpl(r"{APPDATA}\EldenRing"),
        resolved: PathBuf::from(r"C:\Users\max\AppData\Roaming\EldenRing"),
        include: vec![],
        exclude: vec![],
    };
    let finding = Finding {
        id: FindingId::for_target(&target),
        target,
        category: Category::GameSave,
        app: None,
        title: "ELDEN RING — сохранения".to_owned(),
        evidence: vec![Evidence {
            source: EvidenceSource::Rule {
                rule_id: "elden-ring".to_owned(),
            },
            message_key: "evidence.rule_match".to_owned(),
            message_args: BTreeMap::new(),
            confidence: 1.0,
            importance: None,
        }],
        stats: None,
        sensitivity: Sensitivity::None,
        score: None,
        default_selected: true,
        requires_elevation: false,
        tags: vec![],
        children: vec![],
        notes_key: None,
    };
    let game_saves = CategoryTotals {
        count: 1,
        bytes: 28_311_552,
        selected_count: 1,
        selected_bytes: 28_311_552,
    };
    ScanReport {
        schema_version: ScanReport::SCHEMA_VERSION,
        scan_id: Uuid::from_u128(0xa1b2_c3d4_e5f6_4711_8899_aabb_ccdd_eeff),
        app_version: "0.1.0+g1a2b3c4".to_owned(),
        started_at: datetime!(2026-09-28 14:30:12 UTC),
        finished_at: datetime!(2026-09-28 14:31:40.25 UTC),
        environment: EnvironmentSnapshot {
            os: OsInfo {
                product: "Windows 11 Pro".to_owned(),
                display_version: Some("24H2".to_owned()),
                build: "26100.2033".to_owned(),
                arch: "x86_64".to_owned(),
                ui_language: "ru-RU".to_owned(),
            },
            machine_name: "DESKTOP-01".to_owned(),
            known_folders: BTreeMap::from([
                (KnownFolder::Documents, tpl(r"{ONEDRIVE}\Документы")),
                (KnownFolder::Home, tpl(r"{DRIVE:C}\Users\<redacted>")),
                (KnownFolder::SavedGames, tpl(r"{HOME}\Saved Games")),
            ]),
            drives: vec![DriveSnapshot {
                letter: 'C',
                kind: DriveKind::Fixed,
                fs: Some("NTFS".to_owned()),
                label: None,
                volume_serial: Some(305_419_896),
            }],
            launchers: vec![LauncherSnapshot {
                id: "steam".to_owned(),
                root: Some(tpl(r"{PROGRAMFILES_X86}\Steam")),
                game_count: 12,
            }],
            is_elevated: false,
        },
        options: ScanOptionsSnapshot {
            roots: vec![tpl("{DRIVE:D}")],
            collectors: CollectorToggles::default(),
            llm: LlmMode::Off,
            max_depth: None,
        },
        findings: vec![finding],
        unknown_summaries: vec![],
        issues: vec![ScanIssue {
            severity: IssueSeverity::Info,
            source: "environment".to_owned(),
            path: None,
            message_key: "issue.onedrive_missing".to_owned(),
            message_args: BTreeMap::new(),
        }],
        totals: Totals {
            by_category: BTreeMap::from([(Category::GameSave, game_saves)]),
            all: game_saves,
            sensitive_selected: 0,
            unknown_count: 0,
            needs_elevation_count: 0,
            too_large_count: 0,
        },
    }
}

#[test]
fn report_round_trips() {
    let r = report();
    let json = serde_json::to_string(&r).unwrap();
    assert_eq!(ScanReport::from_json(&json).unwrap(), r);
}

#[test]
fn snapshot_report() {
    // insta cannot write enum map keys; serde_json writes them as strings. Going
    // through a string keeps f32 values short.
    let text = serde_json::to_string(&report()).unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    insta::assert_json_snapshot!("scan_report", json);
}

#[test]
fn from_json_checks_version() {
    let mut json = serde_json::to_value(report()).unwrap();
    json["schema_version"] = json!(0);
    assert!(ScanReport::from_json(&json.to_string()).is_ok());

    json["schema_version"] = json!(ScanReport::SCHEMA_VERSION + 1);
    match ScanReport::from_json(&json.to_string()) {
        Err(ReportError::UnsupportedVersion { found, supported }) => {
            assert_eq!((found, supported), (2, 1));
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        ScanReport::from_json("{}"),
        Err(ReportError::Json(_))
    ));
    assert!(matches!(
        ScanReport::from_json(r#"{"schema_version": 1}"#),
        Err(ReportError::Json(_))
    ));
}

fn fake_with_user(user: &str) -> (Environment, PathBuf) {
    let root = PathBuf::from(if cfg!(windows) { r"C:\fake" } else { "/fake" });
    let mut env = Environment::fake(&root);
    env.user_name = user.to_owned();
    let home = root.join("Users").join(user);
    for (folder, rel) in [
        (KnownFolder::Home, &[][..]),
        (KnownFolder::AppData, &["AppData", "Roaming"][..]),
        (KnownFolder::Documents, &["Documents"][..]),
        (KnownFolder::SavedGames, &["Saved Games"][..]),
    ] {
        let mut path = home.clone();
        path.extend(rel);
        env.known_folders.insert(folder, path);
    }
    (env, home)
}

#[test]
fn environment_snapshot_uses_bases_and_hides_user() {
    let (mut env, home) = fake_with_user("maxim");
    let onedrive = home.join("OneDrive");
    env.cloud_roots.push(CloudRoot {
        provider: CloudProvider::OneDrive,
        path: onedrive.clone(),
    });
    env.known_folders
        .insert(KnownFolder::Pictures, onedrive.join("Изображения"));
    let snap = EnvironmentSnapshot::from_env(&env);

    let get = |f| snap.known_folders[&f].as_str().to_owned();
    assert_eq!(get(KnownFolder::AppData), r"{HOME}\AppData\Roaming");
    assert_eq!(get(KnownFolder::Documents), r"{HOME}\Documents");
    assert_eq!(get(KnownFolder::SavedGames), r"{HOME}\Saved Games");
    assert_eq!(get(KnownFolder::Pictures), r"{ONEDRIVE}\Изображения");
    assert!(
        get(KnownFolder::Home).ends_with(r"Users\<redacted>"),
        "{}",
        get(KnownFolder::Home)
    );
    if cfg!(windows) {
        assert_eq!(get(KnownFolder::Home), r"{DRIVE:C}\fake\Users\<redacted>");
        assert_eq!(
            get(KnownFolder::ProgramFilesX86),
            r"{DRIVE:C}\fake\Program Files (x86)"
        );
    }
    assert_eq!(snap.known_folders.len(), env.known_folders.len());

    let json = serde_json::to_string(&snap).unwrap().to_lowercase();
    assert!(!json.contains("maxim"), "{json}");
    assert_eq!(snap.machine_name, env.machine_name);
    assert_eq!(snap.drives.len(), env.drives.len());
}

#[test]
fn launcher_root_is_not_steam_token() {
    let (mut env, _) = fake_with_user("user");
    let steam = env
        .known_folder(KnownFolder::ProgramFilesX86)
        .unwrap()
        .join("Steam");
    let game = |name: &str| InstalledGame {
        store_game_id: "1".to_owned(),
        name: name.to_owned(),
        install_dir: steam.join("steamapps").join("common").join(name),
        size_bytes: None,
        manifest_key: None,
    };
    env.launchers.push(LauncherInfo {
        id: "steam".to_owned(),
        root: Some(steam.clone()),
        user_ids: vec![],
        games: vec![game("A"), game("B")],
    });
    env.launchers.push(LauncherInfo {
        id: "epic".to_owned(),
        root: None,
        user_ids: vec![],
        games: vec![],
    });
    let snap = EnvironmentSnapshot::from_env(&env);
    assert_eq!(
        snap.launchers[0].root.as_ref().unwrap().as_str(),
        r"{PROGRAMFILES_X86}\Steam"
    );
    assert_eq!(snap.launchers[0].game_count, 2);
    assert_eq!(snap.launchers[1].root, None);
}

#[test]
fn drive_snapshot_drops_free_space() {
    let env = Environment::fake(Path::new("/fake"));
    let snap = DriveSnapshot::from(&env.drives[0]);
    let json = serde_json::to_value(&snap).unwrap();
    assert_eq!(
        json,
        json!({ "letter": "C", "kind": "fixed", "fs": "NTFS", "label": null, "volume_serial": 305419896 })
    );
}
