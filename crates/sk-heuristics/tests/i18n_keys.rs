//! i18n keys of the heuristics (SPEC-07 T-07-11, SPEC-11 §4.7).
//!
//! Every `heuristic.*` key the crate emits (evidence `reason_key`s, title parts,
//! `notes_key`s and issues) must have a text in both `app/src/i18n/ru` and
//! `app/src/i18n/en`; texts in this namespace must be used; ru and en must have
//! the same keys and `{{args}}`, and a text uses only the args the code passes.
//!
//! Resource layout as in `sk-system/tests/i18n_keys.rs` (SPEC-11 §4.7): the files
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

/// Prefix of the keys owned by this crate.
const PREFIX: &str = "heuristic.";

/// Keys listed in SPEC-07 T-07-11 and the reasons of §4.5 and §4.7.
const SPEC_KEYS: [&str; 42] = [
    "heuristic.access_denied",
    "heuristic.read_failed",
    "heuristic.data_invalid",
    "heuristic.dotfile",
    "heuristic.dotfolder",
    "heuristic.uwp_saves",
    "heuristic.uwp_package",
    "heuristic.saved_games",
    "heuristic.unreal_saves",
    "heuristic.unity_game",
    "heuristic.save_ext",
    "heuristic.config_like",
    "heuristic.sqlite_recent",
    "heuristic.user_content",
    "heuristic.project_like",
    "heuristic.empty",
    "heuristic.unknown",
    "heuristic.junk.cache_name",
    "heuristic.junk.cache_like",
    "heuristic.junk.restorable",
    "heuristic.junk.installed_program",
    "heuristic.junk.uniform_binaries",
    "heuristic.junk.launcher",
    "heuristic.junk.downloadable",
    "heuristic.title.unrecognized",
    "heuristic.title.app_data",
    "heuristic.web.electron",
    "heuristic.web.chromium",
    "heuristic.dpapi_bound",
    "heuristic.usr.cluster",
    "heuristic.usr.user_folder",
    "heuristic.usr.valuable_file",
    "heuristic.usr_truncated",
    "heuristic.git.no_remote",
    "heuristic.git.unsaved",
    "heuristic.git.unchecked",
    "heuristic.git.clean",
    "heuristic.git.failed",
    "heuristic.git.not_analyzed",
    "heuristic.git.bundle",
    "heuristic.git_failed",
    "heuristic.git_truncated",
];

/// Arguments of every issue: the text uses exactly these (SPEC-07 §4.3.3, §4.4.3, §5).
const ISSUE_ARGS: [(&str, &[&str]); 6] = [
    ("heuristic.access_denied", &[]),
    ("heuristic.read_failed", &["error"]),
    ("heuristic.data_invalid", &["error"]),
    ("heuristic.usr_truncated", &["count", "limit"]),
    ("heuristic.git_failed", &["error"]),
    ("heuristic.git_truncated", &["count", "limit"]),
];

/// Arguments of the git evidence (`state_args`, §4.4.3); `cloud_only_files` is
/// optional and not used by the texts.
const GIT_ARGS: &[&str] = &[
    "dirty",
    "untracked",
    "unpushed",
    "stash",
    "branches",
    "remote",
];

/// Evidence texts that may use args: a subset of the args every emitter of the key
/// passes. A reason of §4.5/§4.7 is passed `files, bytes` by H-UNK, `bytes` only for
/// a dot-file and the cluster args by H-USR, so those texts use none; titles and
/// notes get no args.
const EVIDENCE_ARGS: [(&str, &[&str]); 8] = [
    ("heuristic.usr.cluster", &["files", "bytes"]),
    ("heuristic.usr.user_folder", &["files", "bytes"]),
    ("heuristic.usr.valuable_file", &["bytes", "group"]),
    ("heuristic.git.no_remote", GIT_ARGS),
    ("heuristic.git.unsaved", GIT_ARGS),
    ("heuristic.git.unchecked", GIT_ARGS),
    ("heuristic.git.clean", GIT_ARGS),
    ("heuristic.git.bundle", GIT_ARGS),
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
    key.starts_with(PREFIX)
}

/// Library sources of the crate (tests and test helpers excluded).
fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if path.is_dir() {
            sources(&path, out);
        } else if name.ends_with(".rs")
            && !name.ends_with("_tests.rs")
            && name != "tests.rs"
            && name != "git_test_repo.rs"
        {
            out.push(path);
        }
    }
}

/// String literals that are whole keys of this crate (`"heuristic.x"`; the
/// collector id `"heuristics"` and prefixes ending in `.` are not keys).
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
    for known in SPEC_KEYS {
        assert!(
            keys.contains(known),
            "`{known}` is not found in the sources"
        );
    }
    let listed: BTreeSet<&str> = SPEC_KEYS.into_iter().collect();
    let unlisted: Vec<_> = keys
        .iter()
        .filter(|k| !listed.contains(k.as_str()))
        .collect();
    assert!(
        unlisted.is_empty(),
        "keys not listed in SPEC-07: {unlisted:?}"
    );
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

/// Issues use exactly the args the code passes, evidence texts a subset of them,
/// everything else none; no text nests another one with `$t(…)`.
#[test]
fn texts_use_only_the_given_args() {
    let to_set =
        |args: &[&str]| -> BTreeSet<String> { args.iter().map(|a| (*a).to_owned()).collect() };
    let issue_args: BTreeMap<&str, BTreeSet<String>> = ISSUE_ARGS
        .iter()
        .map(|(key, args)| (*key, to_set(args)))
        .collect();
    let evidence_args: BTreeMap<&str, BTreeSet<String>> = EVIDENCE_ARGS
        .iter()
        .map(|(key, args)| (*key, to_set(args)))
        .collect();
    for lang in LANGUAGES {
        let texts = load_language(lang);
        for (key, text) in texts.iter().filter(|(k, _)| owned(k)) {
            assert!(!text.contains("$t("), "{lang}: `{key}` nests a text");
            let used = placeholders(text);
            if let Some(expected) = issue_args.get(key.as_str()) {
                assert_eq!(&used, expected, "{lang}: `{key}`");
            } else if let Some(allowed) = evidence_args.get(key.as_str()) {
                assert!(used.is_subset(allowed), "{lang}: `{key}` uses {used:?}");
            } else {
                assert!(used.is_empty(), "{lang}: `{key}` uses {used:?}");
            }
        }
    }
}

/// The title parts are short labels: the UI joins them with ` — ` (SPEC-11 §4.7).
#[test]
fn title_parts_are_single_labels() {
    for lang in LANGUAGES {
        let texts = load_language(lang);
        for key in ["heuristic.title.app_data", "heuristic.title.unrecognized"] {
            let text = &texts[key];
            assert!(
                !text.contains(" — ") && !text.contains('\n'),
                "{lang}: `{key}` must be one title part"
            );
        }
    }
}
