use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sk_core::model::{Category, FolderSummary, Marker};
use sk_core::template::PathTemplate;
use tempfile::TempDir;
use time::{Duration, OffsetDateTime};

use super::*;
use crate::{ClassifierInfo, ClassifyItem, ContentPeek};

/// A fresh temporary folder under `<workspace>/target/tmp`.
fn temp_dir() -> TempDir {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
    fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new()
        .prefix("sk-llm-cache-")
        .tempdir_in(&base)
        .unwrap()
}

fn cache_path(dir: &TempDir) -> PathBuf {
    LlmCache::file_path(&dir.path().join("savekeeper-data"))
}

fn summary(newest_unix: i64) -> FolderSummary {
    FolderSummary {
        path: PathTemplate::parse(r"{APPDATA}\App").unwrap(),
        total_bytes: 2048,
        file_count: 3,
        dir_count: 1,
        max_depth: 1,
        newest_mtime: Some(OffsetDateTime::from_unix_timestamp(newest_unix).unwrap()),
        oldest_mtime: None,
        ext_histogram: Vec::new(),
        sample_names: vec!["settings.json".to_owned()],
        top_children: Vec::new(),
        markers: vec![Marker::ConfigLike],
        truncated: false,
    }
}

fn item(id: &str) -> ClassifyItem {
    ClassifyItem {
        id: id.to_owned(),
        // 2026-10-18T13:45:07Z
        summary: summary(1_792_331_107),
        peek: Vec::new(),
        hint: None,
    }
}

fn info(model: &str) -> ClassifierInfo {
    ClassifierInfo {
        provider: "mock",
        model: model.to_owned(),
        is_local: true,
        max_batch: 8,
        max_concurrency: 1,
    }
}

fn result(id: &str, reason: &str) -> Classification {
    Classification {
        id: id.to_owned(),
        category: Category::AppConfig,
        app_guess: Some("App".to_owned()),
        importance: 0.6,
        confidence: 0.9,
        reason: reason.to_owned(),
        suggested_include: vec!["*.json".to_owned()],
        suggested_exclude: vec!["Cache/**".to_owned()],
        from_cache: false,
    }
}

/// A raw cache line with the given key, age and prompt version.
fn raw_line(key: &str, created_at: OffsetDateTime, prompt_version: &str, reason: &str) -> String {
    let created_at = created_at
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    json!({
        "key": key, "created_at": created_at, "provider": "mock", "model": "m",
        "prompt_version": prompt_version,
        "result": {
            "category": "app_data", "app_guess": null, "importance": 0.5, "confidence": 0.5,
            "reason": reason, "suggested_include": [], "suggested_exclude": []
        }
    })
    .to_string()
}

fn write_lines(path: &Path, lines: &[String]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut text = lines.join("\n");
    text.push('\n');
    fs::write(path, text).unwrap();
}

fn file_lines(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn key_follows_the_formula() {
    let it = item("a");
    let mut input = String::new();
    canonical_json(&summary_for_prompt(&it.summary), &mut input);
    input.push_str(&format!("\n\nmock:m\n{PROMPT_VERSION}"));
    let expected = blake3::hash(input.as_bytes()).to_hex().to_string();
    assert_eq!(cache_key(&it, "mock", "m"), expected);
    assert_eq!(expected.len(), 64);
}

#[test]
fn key_ignores_id_hint_and_seconds() {
    let a = item("a");
    let mut b = item("b");
    b.hint = Some("electron".to_owned());
    // Same day, a few hours later.
    b.summary = summary(1_792_331_107 + 3_600);
    assert_eq!(cache_key(&a, "mock", "m"), cache_key(&b, "mock", "m"));
}

#[test]
fn key_depends_on_summary_peek_provider_and_model() {
    let a = item("a");
    let base = cache_key(&a, "ollama", "m");
    assert_ne!(base, cache_key(&a, "ollama", "other"));
    assert_ne!(base, cache_key(&a, "anthropic", "m"));

    let mut next_day = item("a");
    next_day.summary = summary(1_792_331_107 + 86_400);
    assert_ne!(base, cache_key(&next_day, "ollama", "m"));

    let mut peeked = item("a");
    peeked.peek = vec![ContentPeek {
        file: "a.ini".to_owned(),
        head: "x=1".to_owned(),
    }];
    let with_peek = cache_key(&peeked, "ollama", "m");
    assert_ne!(base, with_peek);
    peeked.peek[0].head = "x=2".to_owned();
    assert_ne!(with_peek, cache_key(&peeked, "ollama", "m"));
}

#[test]
fn canonical_json_sorts_keys_recursively() {
    let mut out = String::new();
    canonical_json(&json!({"b": [{"d": 1, "c": "x"}], "a": null}), &mut out);
    assert_eq!(out, r#"{"a":null,"b":[{"c":"x","d":1}]}"#);
}

#[test]
fn missing_file_is_an_empty_cache() {
    let dir = temp_dir();
    let cache = LlmCache::open(&cache_path(&dir)).unwrap();
    assert!(cache.is_empty());
    assert!(cache.get("k", "a").is_none());
}

#[test]
fn write_and_read_back() {
    let dir = temp_dir();
    let path = cache_path(&dir);
    let it = item("a");
    let key = cache_key(&it, "mock", "m");
    let mut cache = LlmCache::open(&path).unwrap();
    cache.insert(key.clone(), &info("m"), &result("a", "settings"));
    // Not written before flush, but visible in memory.
    assert!(!path.exists());
    assert!(cache.get(&key, "a").is_some());
    // `flush` creates the `cache` folder.
    cache.flush().unwrap();

    let lines = file_lines(&path);
    assert_eq!(lines.len(), 1);
    let line = &lines[0];
    assert_eq!(line["key"], json!(key));
    assert_eq!(line["provider"], json!("mock"));
    assert_eq!(line["model"], json!("m"));
    assert_eq!(line["prompt_version"], json!(PROMPT_VERSION));
    assert!(line["created_at"].as_str().unwrap().ends_with('Z'));
    assert!(line["result"].get("id").is_none());
    assert!(line["result"].get("from_cache").is_none());
    assert_eq!(line["result"]["category"], json!("app_config"));

    let reopened = LlmCache::open(&path).unwrap();
    assert_eq!(reopened.len(), 1);
    let got = reopened.get(&key, "other-id").unwrap();
    let mut expected = result("other-id", "settings");
    expected.from_cache = true;
    assert_eq!(got, expected);
}

#[test]
fn flush_appends() {
    let dir = temp_dir();
    let path = cache_path(&dir);
    let mut cache = LlmCache::open(&path).unwrap();
    for (i, reason) in ["one", "two", "three"].iter().enumerate() {
        cache.insert(format!("k{i}"), &info("m"), &result("a", reason));
        cache.flush().unwrap();
    }
    let reasons: Vec<_> = file_lines(&path)
        .iter()
        .map(|l| l["result"]["reason"].clone())
        .collect();
    assert_eq!(reasons, [json!("one"), json!("two"), json!("three")]);
}

#[test]
fn last_line_of_a_key_wins() {
    let dir = temp_dir();
    let path = cache_path(&dir);
    let now = OffsetDateTime::now_utc();
    let lines: Vec<_> = ["first", "second", "third"]
        .iter()
        .map(|r| raw_line("k", now, PROMPT_VERSION, r))
        .chain((0..10).map(|i| raw_line(&format!("o{i}"), now, PROMPT_VERSION, "x")))
        .collect();
    write_lines(&path, &lines);
    let cache = LlmCache::open(&path).unwrap();
    assert_eq!(cache.get("k", "a").unwrap().reason, "third");
    assert_eq!(cache.len(), 11);
}

#[test]
fn broken_lines_are_skipped() {
    let dir = temp_dir();
    let path = cache_path(&dir);
    let now = OffsetDateTime::now_utc();
    let mut lines: Vec<_> = (0..10)
        .map(|i| raw_line(&format!("k{i}"), now, PROMPT_VERSION, "ok"))
        .collect();
    lines.insert(3, "{not json".to_owned());
    lines.insert(5, String::new());
    write_lines(&path, &lines);
    let mut bytes = fs::read(&path).unwrap();
    bytes.extend_from_slice(b"\xff\xfe broken utf-8\n");
    fs::write(&path, bytes).unwrap();

    let cache = LlmCache::open(&path).unwrap();
    assert_eq!(cache.len(), 10);
    assert_eq!(cache.get("k9", "a").unwrap().reason, "ok");
}

#[test]
fn other_prompt_version_is_a_miss() {
    let dir = temp_dir();
    let path = cache_path(&dir);
    let now = OffsetDateTime::now_utc();
    let mut lines = vec![raw_line("old", now, "v0", "old prompt")];
    lines.extend((0..10).map(|i| raw_line(&format!("k{i}"), now, PROMPT_VERSION, "x")));
    write_lines(&path, &lines);
    let cache = LlmCache::open(&path).unwrap();
    assert!(cache.get("old", "a").is_none());
    assert_eq!(cache.len(), 10);
}

#[test]
fn entries_older_than_ttl_are_misses() {
    let dir = temp_dir();
    let path = cache_path(&dir);
    let now = OffsetDateTime::now_utc();
    let mut lines = vec![
        raw_line(
            "old",
            now - Duration::days(CACHE_TTL_DAYS + 1),
            PROMPT_VERSION,
            "old",
        ),
        raw_line(
            "young",
            now - Duration::days(CACHE_TTL_DAYS - 1),
            PROMPT_VERSION,
            "y",
        ),
    ];
    lines.extend((0..10).map(|i| raw_line(&format!("k{i}"), now, PROMPT_VERSION, "x")));
    write_lines(&path, &lines);
    let cache = LlmCache::open(&path).unwrap();
    assert!(cache.get("old", "a").is_none());
    assert!(cache.get("young", "a").is_some());
}

#[test]
fn stale_lines_trigger_compaction_on_open() {
    let dir = temp_dir();
    let path = cache_path(&dir);
    let now = OffsetDateTime::now_utc();
    // 4 live keys, 6 stale lines (duplicates, old prompt, broken): 60% stale.
    let mut lines: Vec<_> = (0..4)
        .map(|i| raw_line(&format!("k{i}"), now, PROMPT_VERSION, "dup"))
        .collect();
    lines.extend((0..4).map(|i| raw_line(&format!("k{i}"), now, PROMPT_VERSION, "live")));
    lines.push(raw_line("old", now, "v0", "x"));
    lines.push("garbage".to_owned());
    write_lines(&path, &lines);

    let cache = LlmCache::open(&path).unwrap();
    assert_eq!(cache.len(), 4);
    let compacted = file_lines(&path);
    assert_eq!(compacted.len(), 4);
    assert!(compacted
        .iter()
        .all(|l| l["result"]["reason"] == json!("live")));
    assert!(!tmp_path(&path).exists());
}

#[test]
fn few_stale_lines_do_not_rewrite_the_file() {
    let dir = temp_dir();
    let path = cache_path(&dir);
    let now = OffsetDateTime::now_utc();
    // 1 stale line of 11: below 30%.
    let mut lines: Vec<_> = (0..10)
        .map(|i| raw_line(&format!("k{i}"), now, PROMPT_VERSION, "x"))
        .collect();
    lines.push(raw_line("k0", now, PROMPT_VERSION, "newer"));
    write_lines(&path, &lines);
    LlmCache::open(&path).unwrap();
    assert_eq!(file_lines(&path).len(), 11);
}

#[test]
fn reinserting_keys_compacts_after_flush() {
    let dir = temp_dir();
    let path = cache_path(&dir);
    let mut cache = LlmCache::open(&path).unwrap();
    for i in 0..10 {
        cache.insert(format!("k{i}"), &info("m"), &result("a", "v1"));
    }
    cache.flush().unwrap();
    for i in 0..3 {
        cache.insert(format!("k{i}"), &info("m"), &result("a", "v2"));
    }
    cache.flush().unwrap();
    // 3 stale of 13 lines: below 30%, no rewrite yet.
    assert_eq!(file_lines(&path).len(), 13);
    for i in 3..5 {
        cache.insert(format!("k{i}"), &info("m"), &result("a", "v2"));
    }
    cache.flush().unwrap();
    // 5 stale of 15 lines: above 30%, rewritten with the live entries only.
    let lines = file_lines(&path);
    assert_eq!(lines.len(), 10);
    let reopened = LlmCache::open(&path).unwrap();
    assert_eq!(reopened.get("k0", "a").unwrap().reason, "v2");
    assert_eq!(reopened.get("k9", "a").unwrap().reason, "v1");
}

#[test]
fn too_many_lines_trigger_compaction() {
    let dir = temp_dir();
    let path = cache_path(&dir);
    let now = OffsetDateTime::now_utc();
    // COMPACT_MAX_LINES + 1 lines with one duplicate: few stale, but too long.
    let mut lines: Vec<_> = (0..COMPACT_MAX_LINES)
        .map(|i| raw_line(&format!("k{i}"), now, PROMPT_VERSION, "x"))
        .collect();
    lines.push(raw_line("k0", now, PROMPT_VERSION, "newer"));
    write_lines(&path, &lines);
    let cache = LlmCache::open(&path).unwrap();
    assert_eq!(cache.len(), COMPACT_MAX_LINES);
    assert_eq!(file_lines(&path).len(), COMPACT_MAX_LINES);
    assert_eq!(cache.get("k0", "a").unwrap().reason, "newer");
}

#[test]
fn failed_compaction_keeps_the_old_file() {
    let dir = temp_dir();
    let path = cache_path(&dir);
    let now = OffsetDateTime::now_utc();
    let lines: Vec<_> = (0..4)
        .map(|_| raw_line("k", now, PROMPT_VERSION, "x"))
        .collect();
    write_lines(&path, &lines);
    let before = fs::read(&path).unwrap();
    // The temporary file cannot be created: a folder has its name.
    fs::create_dir_all(tmp_path(&path)).unwrap();
    let mut cache = LlmCache::open(&path).unwrap();
    assert_eq!(cache.get("k", "a").unwrap().reason, "x");
    assert_eq!(fs::read(&path).unwrap(), before);
    // An explicit compaction reports the error.
    assert!(cache.compact().is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn failed_compaction_after_flush_is_not_an_error() {
    let dir = temp_dir();
    let path = cache_path(&dir);
    let mut cache = LlmCache::open(&path).unwrap();
    cache.insert("k".to_owned(), &info("m"), &result("a", "v1"));
    cache.flush().unwrap();
    fs::create_dir_all(tmp_path(&path)).unwrap();
    // 1 stale of 2 lines: compaction is due but fails; the append succeeded.
    cache.insert("k".to_owned(), &info("m"), &result("a", "v2"));
    cache.flush().unwrap();
    assert_eq!(file_lines(&path).len(), 2);
    assert_eq!(cache.get("k", "a").unwrap().reason, "v2");
    assert_eq!(
        LlmCache::open(&path).unwrap().get("k", "a").unwrap().reason,
        "v2"
    );
}

#[test]
fn compaction_replaces_a_leftover_temporary_file() {
    let dir = temp_dir();
    let path = cache_path(&dir);
    let now = OffsetDateTime::now_utc();
    let lines: Vec<_> = (0..4)
        .map(|_| raw_line("k", now, PROMPT_VERSION, "x"))
        .collect();
    write_lines(&path, &lines);
    fs::write(tmp_path(&path), "leftover of a crash\n").unwrap();
    let cache = LlmCache::open(&path).unwrap();
    assert_eq!(cache.len(), 1);
    assert_eq!(file_lines(&path).len(), 1);
    assert!(!tmp_path(&path).exists());
}

#[test]
fn clear_removes_the_file() {
    let dir = temp_dir();
    let path = cache_path(&dir);
    let mut cache = LlmCache::open(&path).unwrap();
    cache.insert("k".to_owned(), &info("m"), &result("a", "x"));
    cache.flush().unwrap();
    assert!(path.exists());
    cache.clear().unwrap();
    assert!(!path.exists());
    assert!(cache.get("k", "a").is_none());
    // Clearing a missing file is fine.
    cache.clear().unwrap();
}

#[test]
fn file_path_is_in_the_cache_folder() {
    let p = LlmCache::file_path(Path::new("savekeeper-data"));
    assert_eq!(p, Path::new("savekeeper-data/cache/llm-cache.jsonl"));
}
