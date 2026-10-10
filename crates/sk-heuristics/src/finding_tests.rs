//! Tests of the finding helpers (SPEC-07 §4.8, §5).

use std::path::{Path, PathBuf};

use sk_core::env::{Environment, InstalledProgram, KnownFolder, ProgramSource};
use sk_core::model::{
    AppKind, Category, EvidenceSource, FolderSummary, Marker, Sensitivity, Target,
};
use sk_core::template::PathTemplate;

use super::*;
use crate::verdict::MarkerVerdict;

fn env() -> Environment {
    Environment::fake(Path::new(if cfg!(windows) { r"C:\fake" } else { "/fake" }))
}

fn under(folder: KnownFolder, rel: &[&str]) -> PathBuf {
    let base = env().known_folder(folder).unwrap().to_path_buf();
    rel.iter().fold(base, |p, c| p.join(c))
}

fn summary(markers: Vec<Marker>, total_bytes: u64) -> FolderSummary {
    FolderSummary {
        path: PathTemplate::parse(r"{APPDATA}\Tool").unwrap(),
        total_bytes,
        file_count: 3,
        dir_count: 0,
        max_depth: 1,
        newest_mtime: None,
        oldest_mtime: None,
        ext_histogram: vec![],
        sample_names: vec![],
        top_children: vec![],
        markers,
        truncated: false,
    }
}

fn program(name: &str, publisher: Option<&str>) -> InstalledProgram {
    InstalledProgram {
        name: name.to_owned(),
        publisher: publisher.map(str::to_owned),
        version: None,
        install_location: None,
        install_date: None,
        estimated_size_kb: None,
        source: ProgramSource::Hkcu,
        uninstall_key: name.to_owned(),
    }
}

#[test]
fn opaque_names_are_guids_and_hashes() {
    for name in [
        "{3F2504E0-4F89-11D3-9A0C-0305E82C3301}",
        "3f2504e0-4f89-11d3-9a0c-0305e82c3301",
        "a8f3c9d2e1b04f5a",
        "0123456789abcdef0123",
    ] {
        assert!(is_opaque_name(name), "{name}");
    }
    for name in [
        "Discord",
        "deadbeefdeadbeef", // letters only
        "12345678901234567890",
        "a8f3c9d2e1b04f5", // 15 characters
        "3F2504E0-4F89-11D3-9A0C",
        "{not-a-guid}",
    ] {
        assert!(!is_opaque_name(name), "{name}");
    }
}

#[test]
fn titles_name_the_folder() {
    let env = env();
    assert_eq!(
        display_name(&under(KnownFolder::AppData, &["Tool"]), &env),
        "Tool"
    );
    // `{LOCALLOW}\Company\Product`.
    assert_eq!(
        display_name(&under(KnownFolder::LocalLow, &["Studio", "Game"]), &env),
        "Studio / Game"
    );
    assert_eq!(
        display_name(&under(KnownFolder::AppData, &["a8f3c9d2e1b04f5a"]), &env),
        "a8f3c9d2e1b04f5a — heuristic.title.unrecognized"
    );
}

#[test]
fn slugs_are_lowercase_words() {
    assert_eq!(slug("OBS Studio"), "obs-studio");
    assert_eq!(slug("  Foo__Bar!! "), "foo-bar");
    assert_eq!(slug("Сохранения 2"), "сохранения-2");
    assert_eq!(slug("..."), "");
}

#[test]
fn installed_programs_match_by_tokens() {
    let mut env = env();
    assert_eq!(match_installed("Tool", &env), None);
    env.installed_programs = vec![
        program("Obsidian", Some("Obsidian.md")),
        program("Visual Studio Code", Some("Microsoft Corporation")),
    ];
    assert_eq!(match_installed("obsidian", &env), Some(true));
    // By the publisher: {obsidian, md}.
    assert_eq!(match_installed("Obsidian.md", &env), Some(true));
    // {code} against {visual, studio, code}: 1/3 < 0.8.
    assert_eq!(match_installed("Code", &env), None);
    assert_eq!(match_installed("---", &env), None);
}

#[test]
fn app_is_guessed_from_the_name() {
    let mut env = env();
    env.installed_programs = vec![program("Tool", None)];
    let app = guess_app(&under(KnownFolder::AppData, &["Tool"]), &env).unwrap();
    assert_eq!(app.id, "tool");
    assert_eq!(app.name, "Tool");
    assert_eq!(app.kind, AppKind::Application);
    assert_eq!(app.installed, Some(true));
    assert!(guess_app(&under(KnownFolder::AppData, &["___"]), &env).is_none());
}

#[test]
fn tags_come_from_markers_and_size() {
    let s = summary(
        vec![
            Marker::GitRepo,
            Marker::UnityGame,
            Marker::ConfigLike,
            Marker::CloudSynced,
        ],
        10,
    );
    assert_eq!(tags_from(&s), ["git", "unity", "cloud-synced"]);
    let huge = summary(vec![], 50 * 1024 * 1024 * 1024 + 1);
    assert_eq!(tags_from(&huge), ["huge"]);
    let limit = summary(vec![], 50 * 1024 * 1024 * 1024);
    assert!(tags_from(&limit).is_empty());
}

#[test]
fn make_finding_follows_the_verdict() {
    let env = env();
    let root = under(KnownFolder::AppData, &["Tool"]);
    let verdict = MarkerVerdict {
        category: Category::AppConfig,
        confidence: 0.6,
        reason_key: "heuristic.config_like",
        include: vec!["*.json".to_owned()],
        exclude: vec!["logs".to_owned()],
    };
    let s = summary(vec![Marker::ConfigLike], 2048);
    let f = make_finding(&root, &verdict, &s, "h-unk", &env);
    let Target::FileSet {
        root: template,
        resolved,
        include,
        exclude,
    } = &f.target
    else {
        panic!("not a file set: {:?}", f.target);
    };
    assert_eq!(template.as_str(), r"{APPDATA}\Tool");
    assert_eq!(resolved, &root);
    assert_eq!(include, &["*.json"]);
    assert_eq!(exclude, &["logs"]);
    assert_eq!(f.id, FindingId::for_target(&f.target));
    assert_eq!(f.category, Category::AppConfig);
    assert_eq!(f.title, "Tool");
    assert_eq!(f.sensitivity, Sensitivity::None);
    assert!(f.stats.is_none() && f.score.is_none() && !f.default_selected);
    let [evidence] = f.evidence.as_slice() else {
        panic!("{:?}", f.evidence);
    };
    assert_eq!(
        evidence.source,
        EvidenceSource::Heuristic {
            heuristic_id: "h-unk".to_owned()
        }
    );
    assert_eq!(evidence.message_key, "heuristic.config_like");
    assert_eq!(evidence.confidence, 0.6);
    assert_eq!(evidence.message_args["files"], "3");
    assert_eq!(evidence.message_args["bytes"], "2048");
    assert_eq!(sensitivity_for(Category::Credentials), Sensitivity::High);
}
