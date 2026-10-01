//! JSON contract of the domain model (SPEC-02 §2, §8): round-trip and snapshots.

// Helpers outside `#[test]` functions are not covered by `allow-unwrap-in-tests`.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::path::PathBuf;

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::json;
use sk_core::model::{
    AppKind, AppRef, Category, CollectorToggles, Evidence, EvidenceSource, Finding, FindingId,
    IssueSeverity, LlmMode, RegHive, ScanIssue, Score, Sensitivity, Target, TargetStats,
};
use sk_core::template::PathTemplate;
use time::macros::datetime;

fn tpl(s: &str) -> PathTemplate {
    serde_json::from_value(json!(s)).unwrap()
}

fn fid(s: &str) -> FindingId {
    serde_json::from_value(json!(s)).unwrap()
}

fn args(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn round_trip<T: Serialize + DeserializeOwned + PartialEq + Debug>(value: &T) {
    let json = serde_json::to_string(value).unwrap();
    let back: T = serde_json::from_str(&json).unwrap();
    assert_eq!(&back, value, "round-trip changed the value: {json}");
}

fn game_save() -> Finding {
    Finding {
        id: fid("3fa2c9e01b7d44aa"),
        target: Target::FileSet {
            root: tpl(r"{APPDATA}\EldenRing"),
            resolved: PathBuf::from(r"C:\Users\max\AppData\Roaming\EldenRing"),
            include: vec![],
            exclude: vec!["*.bak".to_owned()],
        },
        category: Category::GameSave,
        app: Some(AppRef {
            id: "elden-ring".to_owned(),
            name: "ELDEN RING".to_owned(),
            kind: AppKind::Game,
            source_ids: args(&[("steam", "1245620"), ("ludusavi", "ELDEN RING")]),
            installed: Some(true),
            process_names: vec!["eldenring.exe".to_owned()],
        }),
        title: "ELDEN RING — сохранения".to_owned(),
        evidence: vec![
            Evidence {
                source: EvidenceSource::Ludusavi {
                    game: "ELDEN RING".to_owned(),
                    manifest_version: "2026-09-01".to_owned(),
                },
                message_key: "evidence.ludusavi_match".to_owned(),
                message_args: args(&[("game", "ELDEN RING")]),
                confidence: 0.95,
                importance: None,
            },
            Evidence {
                source: EvidenceSource::Launcher {
                    launcher: "steam".to_owned(),
                },
                message_key: "evidence.launcher_installed".to_owned(),
                message_args: BTreeMap::new(),
                confidence: 0.8,
                importance: None,
            },
        ],
        stats: Some(TargetStats {
            total_bytes: 28_311_552,
            file_count: 4,
            dir_count: 1,
            newest_mtime: Some(datetime!(2026-09-27 21:14:05 UTC)),
            oldest_mtime: Some(datetime!(2024-03-02 10:00:00.5 UTC)),
            locked_files: 1,
            cloud_only_bytes: 0,
            cloud_only_files: 0,
            largest_file_bytes: Some(28_000_000),
            truncated: false,
        }),
        sensitivity: Sensitivity::None,
        score: Some(Score {
            value: 0.92,
            components: [
                ("irreplaceability".to_owned(), 0.9),
                ("recency".to_owned(), 0.1),
            ]
            .into_iter()
            .collect(),
        }),
        default_selected: true,
        requires_elevation: false,
        tags: vec!["steam".to_owned()],
        children: vec![fid("0011223344556677")],
        notes_key: None,
    }
}

fn registry() -> Finding {
    Finding {
        id: fid("5c1e0a9d3b2f7e61"),
        target: Target::Registry {
            hive: RegHive::Hkcu,
            key: r"Software\SimonTatham\PuTTY".to_owned(),
            recursive: true,
        },
        category: Category::AppConfig,
        app: None,
        title: "PuTTY — сессии".to_owned(),
        evidence: vec![Evidence {
            source: EvidenceSource::Rule {
                rule_id: "putty".to_owned(),
            },
            message_key: "evidence.rule_match".to_owned(),
            message_args: BTreeMap::new(),
            confidence: 1.0,
            importance: None,
        }],
        stats: None,
        sensitivity: Sensitivity::Low,
        score: None,
        default_selected: false,
        requires_elevation: false,
        tags: vec![],
        children: vec![],
        notes_key: Some("notes.putty_keys".to_owned()),
    }
}

fn system_export() -> Finding {
    Finding {
        id: fid("9d8c7b6a5f4e3d2c"),
        target: Target::SystemExport {
            exporter_id: "wifi".to_owned(),
            params: json!({ "include_keys": true, "profiles": ["Home", "Office"] }),
        },
        category: Category::SystemSettings,
        app: None,
        title: "Wi-Fi profiles".to_owned(),
        evidence: vec![Evidence {
            source: EvidenceSource::System {
                exporter_id: "wifi".to_owned(),
            },
            message_key: "evidence.system_export".to_owned(),
            message_args: BTreeMap::new(),
            confidence: 1.0,
            importance: Some(0.7),
        }],
        stats: None,
        sensitivity: Sensitivity::High,
        score: None,
        default_selected: true,
        requires_elevation: true,
        tags: vec![],
        children: vec![],
        notes_key: None,
    }
}

fn single_file() -> Finding {
    Finding {
        id: fid("1a2b3c4d5e6f7a8b"),
        target: Target::File {
            path: tpl(r"{HOME}\.gitconfig"),
            resolved: PathBuf::from(r"C:\Users\max\.gitconfig"),
        },
        category: Category::DevEnvironment,
        app: None,
        title: "Git — global config".to_owned(),
        evidence: vec![Evidence {
            source: EvidenceSource::Llm {
                provider: "ollama".to_owned(),
                model: "qwen2.5:7b-instruct".to_owned(),
            },
            message_key: "evidence.llm".to_owned(),
            message_args: args(&[("reason", "Global git settings")]),
            confidence: 0.75,
            importance: Some(0.6),
        }],
        stats: Some(TargetStats {
            total_bytes: 512,
            file_count: 1,
            dir_count: 0,
            newest_mtime: None,
            oldest_mtime: None,
            locked_files: 0,
            cloud_only_bytes: 0,
            cloud_only_files: 0,
            largest_file_bytes: None,
            truncated: false,
        }),
        sensitivity: Sensitivity::Low,
        score: None,
        default_selected: true,
        requires_elevation: false,
        tags: vec![],
        children: vec![],
        notes_key: None,
    }
}

#[test]
fn findings_round_trip() {
    for finding in [game_save(), registry(), system_export(), single_file()] {
        round_trip(&finding);
    }
}

#[test]
fn snapshot_findings() {
    insta::assert_json_snapshot!("finding_file_set", game_save());
    insta::assert_json_snapshot!("finding_registry", registry());
    insta::assert_json_snapshot!("finding_system_export", system_export());
    insta::assert_json_snapshot!("finding_file", single_file());
}

#[test]
fn contract_field_formats() {
    let json = serde_json::to_value(game_save()).unwrap();
    assert_eq!(json["category"], "game_save");
    assert_eq!(json["target"]["kind"], "file_set");
    assert_eq!(json["target"]["root"], r"{APPDATA}\EldenRing");
    assert_eq!(json["app"]["kind"], "game");
    assert_eq!(json["evidence"][0]["source"]["kind"], "ludusavi");
    assert_eq!(json["stats"]["newest_mtime"], "2026-09-27T21:14:05Z");
    assert_eq!(json["sensitivity"], "none");

    let json = serde_json::to_value(registry()).unwrap();
    assert_eq!(json["target"]["hive"], "hkcu");
}

#[test]
fn missing_optional_timestamps_deserialize_as_none() {
    let mut json = serde_json::to_value(single_file().stats).unwrap();
    let stats = json.as_object_mut().unwrap();
    stats.remove("newest_mtime");
    stats.remove("oldest_mtime");
    let back: TargetStats = serde_json::from_value(json).unwrap();
    assert_eq!(back.newest_mtime, None);
    assert_eq!(back.oldest_mtime, None);
}

#[test]
fn snapshot_enum_names() {
    let categories = [
        Category::GameSave,
        Category::GameConfig,
        Category::AppConfig,
        Category::AppData,
        Category::UserFiles,
        Category::DevEnvironment,
        Category::Credentials,
        Category::SystemSettings,
        Category::Reinstallable,
        Category::Cache,
        Category::Unknown,
    ];
    let sources = [
        EvidenceSource::Rule {
            rule_id: "r".to_owned(),
        },
        EvidenceSource::Ludusavi {
            game: "g".to_owned(),
            manifest_version: "v".to_owned(),
        },
        EvidenceSource::Launcher {
            launcher: "l".to_owned(),
        },
        EvidenceSource::System {
            exporter_id: "e".to_owned(),
        },
        EvidenceSource::Heuristic {
            heuristic_id: "h".to_owned(),
        },
        EvidenceSource::Llm {
            provider: "p".to_owned(),
            model: "m".to_owned(),
        },
        EvidenceSource::User,
    ];
    let issue = ScanIssue {
        severity: IssueSeverity::Warning,
        source: "scan".to_owned(),
        path: Some(r"{LOCALAPPDATA}\Foo".to_owned()),
        message_key: "issue.locked".to_owned(),
        message_args: args(&[("count", "3")]),
    };
    for c in categories {
        round_trip(&c);
    }
    for s in &sources {
        round_trip(s);
    }
    round_trip(&issue);
    for v in [
        IssueSeverity::Info,
        IssueSeverity::Warning,
        IssueSeverity::Error,
    ] {
        round_trip(&v);
    }
    for v in [
        AppKind::Game,
        AppKind::Application,
        AppKind::System,
        AppKind::DevTool,
    ] {
        round_trip(&v);
    }
    for v in [LlmMode::Off, LlmMode::Local, LlmMode::Cloud] {
        round_trip(&v);
    }
    insta::assert_json_snapshot!(
        "enum_names",
        json!({
            "category": categories,
            "evidence_source": sources,
            "severity": [IssueSeverity::Info, IssueSeverity::Warning, IssueSeverity::Error],
            "app_kind": [AppKind::Game, AppKind::Application, AppKind::System, AppKind::DevTool],
            "sensitivity": [Sensitivity::None, Sensitivity::Low, Sensitivity::High],
            "reg_hive": [RegHive::Hkcu, RegHive::Hklm],
            "llm_mode": [LlmMode::Off, LlmMode::Local, LlmMode::Cloud],
            "scan_issue": issue,
        })
    );
}

#[test]
fn scan_option_defaults() {
    assert_eq!(LlmMode::default(), LlmMode::Off);
    let toggles = CollectorToggles::default();
    assert!(toggles.rules && toggles.games && toggles.system && toggles.heuristics);
    round_trip(&toggles);
    assert_eq!(
        serde_json::to_value(toggles).unwrap(),
        json!({ "rules": true, "games": true, "system": true, "heuristics": true })
    );
}

#[test]
fn category_order_is_declaration_order() {
    assert!(Category::GameSave < Category::GameConfig);
    assert!(Category::Cache < Category::Unknown);
}
