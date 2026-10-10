//! i18n keys of the system exports (SPEC-06 T-06-11, SPEC-11 §4.7).
//!
//! Every `system.*`, `issue.system.*` and `evidence.system_export` key the crate emits
//! (finding titles, `reason_key`s, restore hints, warnings, the evidence and the texts
//! of `checklist.md`) must have a text in both `app/src/i18n/ru` and `app/src/i18n/en`;
//! texts in these namespaces must be used; ru and en must have the same keys and
//! `{{args}}`, and the args of the warnings are the ones the exporters pass.
//!
//! Resource layout as in `sk-games/tests/i18n_keys.rs` (SPEC-11 §4.7): the files
//! `app/src/i18n/<lang>/*.json` are merged under a top-level key equal to the file
//! name; the full key is `<file>.<path>`.

// Helpers outside `#[test]` functions are not covered by `allow-unwrap-in-tests`.
#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// UI languages (SPEC-11 FR-11-12).
const LANGUAGES: [&str; 2] = ["ru", "en"];

/// Prefixes of the keys owned by this crate.
const PREFIXES: [&str; 3] = ["system.", "issue.system.", "evidence.system_export"];

/// Arguments of every warning (SPEC-06 §4.3).
const ISSUE_ARGS: [(&str, &[&str]); 17] = [
    ("issue.system.activation_query_failed", &["error"]),
    ("issue.system.bitlocker_needs_admin", &[]),
    ("issue.system.bitlocker_query_failed", &["error"]),
    ("issue.system.cloud_only", &["path"]),
    ("issue.system.drivers_export_exit_code", &["code"]),
    ("issue.system.file_copy_failed", &["path", "error"]),
    ("issue.system.folder_read_failed", &["path", "error"]),
    (
        "issue.system.ps_module_not_copied",
        &["module", "edition", "bytes"],
    ),
    ("issue.system.registry_export_failed", &["key", "error"]),
    ("issue.system.registry_key_missing", &["key"]),
    ("issue.system.registry_read_failed", &["key", "error"]),
    ("issue.system.task_export_failed", &["task", "error"]),
    ("issue.system.tasks_hidden_without_admin", &[]),
    ("issue.system.wifi_export_exit_code", &["code"]),
    ("issue.system.wifi_key_protected", &["profile"]),
    ("issue.system.winget_export_exit_code", &["code"]),
    ("issue.system.winget_json_unreadable", &["error"]),
];

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Texts of one language: full key → text.
fn load_language(lang: &str) -> BTreeMap<String, String> {
    let dir = crate_dir().join("../../app/src/i18n").join(lang);
    let mut texts = BTreeMap::new();
    for entry in fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let ns = path.file_stem().unwrap().to_string_lossy().into_owned();
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap())
            .unwrap_or_else(|e| panic!("{}: invalid JSON: {e}", path.display()));
        flatten(&ns, &value, &path, &mut texts);
    }
    texts
}

fn flatten(key: &str, value: &Value, file: &Path, out: &mut BTreeMap<String, String>) {
    match value {
        Value::Object(members) => {
            for (name, nested) in members {
                flatten(&format!("{key}.{name}"), nested, file, out);
            }
        }
        Value::String(text) => {
            assert!(
                !text.trim().is_empty(),
                "{}: `{key}` is empty",
                file.display()
            );
            out.insert(key.to_owned(), text.clone());
        }
        other => panic!("{}: `{key}` is not a string: {other}", file.display()),
    }
}

fn owned(key: &str) -> bool {
    PREFIXES.iter().any(|p| key.starts_with(p))
}

/// Library sources of the crate (tests excluded).
fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if path.is_dir() {
            sources(&path, out);
        } else if name.ends_with(".rs")
            && !name.ends_with("_tests.rs")
            && name != "tests.rs"
            && name != "test_util.rs"
        {
            out.push(path);
        }
    }
}

/// String literals that are whole keys of this crate (`"issue.system.x"`; the bare
/// issue source `"system"` and format strings are not keys).
fn emitted_keys() -> BTreeSet<String> {
    let mut files = Vec::new();
    sources(&crate_dir().join("src"), &mut files);
    let mut keys = BTreeSet::new();
    for file in files {
        let text = fs::read_to_string(&file).unwrap();
        for (start, _) in text.match_indices('"') {
            let rest = &text[start + 1..];
            let len = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
                .unwrap_or(rest.len());
            let (literal, after) = rest.split_at(len);
            if after.starts_with('"') && owned(literal) && !literal.ends_with('.') {
                keys.insert(literal.to_owned());
            }
        }
    }
    keys
}

/// `{{arg}}` names of a text.
fn placeholders(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = text;
    while let Some(open) = rest.find("{{") {
        let after = &rest[open + 2..];
        let Some(close) = after.find("}}") else { break };
        out.insert(after[..close].trim().to_owned());
        rest = &after[close + 2..];
    }
    out
}

#[test]
fn every_emitted_key_has_texts() {
    let keys = emitted_keys();
    for known in [
        "evidence.system_export",
        "system.not_windows",
        "system.winget.missing",
        "system.title.winget",
        "system.title.checklist",
        "system.restore.programs",
        "system.checklist.title",
        "system.checklist.license.license_file",
        "system.checklist.windows_oem_key",
    ]
    .into_iter()
    .chain(ISSUE_ARGS.iter().map(|(key, _)| *key))
    {
        assert!(
            keys.contains(known),
            "`{known}` is not found in the sources"
        );
    }
    for lang in LANGUAGES {
        let texts = load_language(lang);
        let missing: Vec<_> = keys.iter().filter(|k| !texts.contains_key(*k)).collect();
        assert!(missing.is_empty(), "keys without {lang} text: {missing:?}");
    }
}

#[test]
fn every_text_is_used() {
    let keys = emitted_keys();
    for lang in LANGUAGES {
        let texts = load_language(lang);
        let stale: Vec<_> = texts
            .keys()
            .filter(|k| owned(k) && !keys.contains(*k))
            .collect();
        assert!(stale.is_empty(), "{lang}: texts nothing uses: {stale:?}");
    }
}

#[test]
fn ru_and_en_have_the_same_keys_and_args() {
    let ru = load_language("ru");
    let en = load_language("en");
    let owned_keys = |texts: &BTreeMap<String, String>| -> BTreeSet<String> {
        texts.keys().filter(|k| owned(k)).cloned().collect()
    };
    assert_eq!(owned_keys(&ru), owned_keys(&en));
    for key in owned_keys(&ru) {
        assert_eq!(
            placeholders(&ru[&key]),
            placeholders(&en[&key]),
            "`{key}`: ru and en use different args"
        );
    }
}

/// Warnings use exactly the args the exporters pass; titles, reasons, restore hints and
/// the evidence get no args (the evidence args differ per exporter, SPEC-11 §4.7), and
/// no text nests another one with `$t(…)`.
#[test]
fn texts_use_only_the_given_args() {
    let issue_args: BTreeMap<&str, BTreeSet<String>> = ISSUE_ARGS
        .iter()
        .map(|(key, args)| (*key, args.iter().map(|a| (*a).to_owned()).collect()))
        .collect();
    for lang in LANGUAGES {
        let texts = load_language(lang);
        for (key, text) in texts.iter().filter(|(k, _)| owned(k)) {
            assert!(!text.contains("$t("), "{lang}: `{key}` nests a text");
            let used = placeholders(text);
            if let Some(expected) = issue_args.get(key.as_str()) {
                assert_eq!(&used, expected, "{lang}: `{key}`");
            } else if !key.starts_with("system.checklist.") {
                assert!(used.is_empty(), "{lang}: `{key}` uses {used:?}");
            }
        }
        let issues: BTreeSet<&str> = texts
            .keys()
            .filter(|k| k.starts_with("issue.system."))
            .map(String::as_str)
            .collect();
        let listed: BTreeSet<&str> = issue_args.keys().copied().collect();
        assert_eq!(issues, listed, "{lang}: `ISSUE_ARGS` is out of date");
    }
}
