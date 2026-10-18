use sk_core::model::{Category, FolderSummary};
use sk_core::template::PathTemplate;

use super::*;
use crate::{ClassifyItem, LlmError};

fn item(id: &str) -> ClassifyItem {
    let path = match PathTemplate::parse("{APPDATA}/app") {
        Ok(t) => t,
        Err(e) => panic!("bad template: {e}"),
    };
    ClassifyItem {
        id: id.to_owned(),
        summary: FolderSummary {
            path,
            total_bytes: 1,
            file_count: 1,
            dir_count: 0,
            max_depth: 0,
            newest_mtime: None,
            oldest_mtime: None,
            ext_histogram: Vec::new(),
            sample_names: Vec::new(),
            top_children: Vec::new(),
            markers: Vec::new(),
            truncated: false,
        },
        peek: Vec::new(),
        hint: None,
    }
}

fn parse(json: &str, ids: &[&str]) -> ParsedResponse {
    let items: Vec<ClassifyItem> = ids.iter().map(|id| item(id)).collect();
    match parse_response(json, &items) {
        Ok(p) => p,
        Err(e) => panic!("parse failed: {e}"),
    }
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn valid_response_is_parsed() {
    let json = r#"{"results":[{"id":"a","category":"app_config","app_guess":"OBS Studio",
        "importance":0.7,"confidence":0.9,"reason":"Settings.","suggested_include":["*.json"],
        "suggested_exclude":["logs/**"]}]}"#;
    let p = parse(json, &["a"]);
    assert!(p.missing.is_empty());
    assert_eq!(
        p.results,
        vec![Classification {
            id: "a".to_owned(),
            category: Category::AppConfig,
            app_guess: Some("OBS Studio".to_owned()),
            importance: 0.7,
            confidence: 0.9,
            reason: "Settings.".to_owned(),
            suggested_include: strings(&["*.json"]),
            suggested_exclude: strings(&["logs/**"]),
            from_cache: false,
        }]
    );
}

#[test]
fn invalid_json_is_bad_response() {
    let items = vec![item("a")];
    for json in ["not json", "{\"results\": 5}", "{}", "[]"] {
        match parse_response(json, &items) {
            Err(LlmError::BadResponse(_)) => {}
            other => panic!("{json}: expected BadResponse, got {other:?}"),
        }
    }
}

#[test]
fn foreign_and_repeated_ids_are_ignored_missing_are_reported() {
    let json = r#"{"results":[
        {"id":"x","category":"cache","confidence":1},
        {"id":"b","category":"cache","confidence":0.8},
        {"id":"b","category":"app_data","confidence":0.9},
        {"category":"cache"},
        "garbage"
    ]}"#;
    let p = parse(json, &["a", "b", "c"]);
    assert_eq!(p.results.len(), 1);
    assert_eq!(p.results[0].id, "b");
    assert_eq!(p.results[0].category, Category::Cache);
    assert_eq!(p.missing, strings(&["a", "c"]));
}

#[test]
fn unknown_category_becomes_unknown() {
    assert_eq!(category("game_save"), Category::GameSave);
    assert_eq!(category(" credentials "), Category::Credentials);
    assert_eq!(category("GameSave"), Category::Unknown);
    assert_eq!(category("documents"), Category::Unknown);
    assert_eq!(category(""), Category::Unknown);
    let p = parse(r#"{"results":[{"id":"a","category":"music"}]}"#, &["a"]);
    assert_eq!(p.results[0].category, Category::Unknown);
}

#[test]
fn numbers_are_clamped_and_nan_is_zero() {
    assert_eq!(unit(f64::NAN), 0.0);
    assert_eq!(unit(-0.5), 0.0);
    assert_eq!(unit(1.7), 1.0);
    assert_eq!(unit(f64::INFINITY), 1.0);
    assert_eq!(unit(f64::NEG_INFINITY), 0.0);
    assert_eq!(unit(0.25), 0.25);
    let p = parse(
        r#"{"results":[{"id":"a","importance":12,"confidence":-3},{"id":"b","importance":null}]}"#,
        &["a", "b"],
    );
    assert_eq!(
        (p.results[0].importance, p.results[0].confidence),
        (1.0, 0.0)
    );
    assert_eq!(
        (p.results[1].importance, p.results[1].confidence),
        (0.0, 0.0)
    );
}

#[test]
fn reason_is_cleaned_cut_and_never_empty() {
    assert_eq!(reason("  Game\nsaves.\u{7}  "), "Game saves.");
    assert_eq!(reason(" \t\r\n "), EMPTY_REASON);
    assert_eq!(reason(""), EMPTY_REASON);
    let long: String = "я".repeat(300);
    let cut = reason(&long);
    assert_eq!(cut.chars().count(), MAX_REASON_CHARS);
    assert!(cut.chars().all(|c| c == 'я'));
    let p = parse(r#"{"results":[{"id":"a"}]}"#, &["a"]);
    assert_eq!(p.results[0].reason, EMPTY_REASON);
}

#[test]
fn app_guess_is_trimmed_cut_and_empty_is_none() {
    assert_eq!(app_guess("  OBS Studio "), Some("OBS Studio".to_owned()));
    assert_eq!(app_guess("   "), None);
    let long = "x".repeat(100);
    assert_eq!(
        app_guess(&long).map(|s| s.chars().count()),
        Some(MAX_APP_GUESS_CHARS)
    );
    let p = parse(r#"{"results":[{"id":"a","app_guess":""}]}"#, &["a"]);
    assert_eq!(p.results[0].app_guess, None);
}

#[test]
fn bad_globs_are_dropped() {
    let raw = strings(&[
        "logs/**",
        "*.json",
        "  cache/*  ",
        "",
        "../secrets/*",
        "data/../../x",
        "/etc/*",
        "\\server\\share",
        "C:/Users/*",
        "c:foo",
        "a[b",
        "{unclosed",
        "*.json",
    ]);
    assert_eq!(globs(&raw), strings(&["logs/**", "*.json", "cache/*"]));
}

#[test]
fn at_most_twenty_globs_of_each_kind() {
    let raw: Vec<String> = (0..30).map(|i| format!("dir{i}/**")).collect();
    let kept = globs(&raw);
    assert_eq!(kept.len(), MAX_GLOBS);
    assert_eq!(kept[19], "dir19/**");
    let json = format!(
        r#"{{"results":[{{"id":"a","suggested_include":{0},"suggested_exclude":{0}}}]}}"#,
        serde_json::to_string(&raw).unwrap_or_default()
    );
    let p = parse(&json, &["a"]);
    assert_eq!(p.results[0].suggested_include.len(), MAX_GLOBS);
    assert_eq!(p.results[0].suggested_exclude.len(), MAX_GLOBS);
}
