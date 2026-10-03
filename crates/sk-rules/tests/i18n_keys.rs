//! i18n keys of the rules engine (SPEC-04 T-04-11, SPEC-11 §4.7).
//!
//! The key list is generated here from the sources: every `title_key`,
//! `label_key`, `notes_key` and `message_key` of the built-in rules and every
//! `issue.*`/`evidence.*` key the crate emits must have a text in both
//! `app/src/i18n/ru` and `app/src/i18n/en`, and the two languages must have
//! the same keys.
//!
//! Resource layout (SPEC-11 §3, §4.7): the files `app/src/i18n/<lang>/*.json`
//! are merged into one i18next namespace (`translation`, `keySeparator: "."`),
//! each under a top-level key equal to its file name. A file holds nested
//! objects without that prefix, and the full key is `<file>.<path>`
//! (`rules.json` → `{"chrome": {"profiles": …}}` → `rules.chrome.profiles`).
//! Values are strings with `{{arg}}` interpolation; plural forms use the
//! i18next suffixes (`_one`, `_few`, `_many`, `_other`).

// Helpers outside `#[test]` functions are not covered by `allow-unwrap-in-tests`.
#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::Value;
use sk_rules::compile::compile_yaml;
use sk_rules::schema::Rule;
use sk_rules::RuleSet;

/// UI languages (SPEC-11 FR-11-12).
const LANGUAGES: [&str; 2] = ["ru", "en"];

/// i18next plural suffixes; a key with one of them is a form of its base key
/// when the base also has an `_other` form.
const PLURAL_SUFFIXES: [&str; 6] = ["_zero", "_one", "_two", "_few", "_many", "_other"];

/// Top-level key (file name) of the rule texts (`title_key`, `label_key`,
/// `notes_key`).
const RULES_NS: &str = "rules";

/// `reason` values of `issue.rules.from_json_skipped` (SPEC-04 §4.2.1 step 6),
/// translated by the nested `issue.rules.from_json_skip_reason.<reason>`.
///
/// `Skip` is crate-private, so the values are read from the match arms of
/// `Skip::as_str` in `src/expand_json.rs`.
fn skip_reasons() -> BTreeSet<String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/expand_json.rs");
    let text = fs::read_to_string(&path).unwrap();
    let re = Regex::new(r#"Skip::[A-Za-z]+\s*=>\s*"([a-z_]+)""#).unwrap();
    let reasons: BTreeSet<String> = re.captures_iter(&text).map(|c| c[1].to_owned()).collect();
    assert!(
        reasons.contains("not_absolute") && reasons.len() >= 5,
        "{}: `Skip::as_str` values are not found: {reasons:?}",
        path.display()
    );
    reasons
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Texts of one language: full key → text.
fn load_language(lang: &str) -> BTreeMap<String, String> {
    let dir = repo().join("app/src/i18n").join(lang);
    let mut texts = BTreeMap::new();
    for entry in fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let ns = path.file_stem().unwrap().to_string_lossy().into_owned();
        let text = fs::read_to_string(&path).unwrap();
        let value: Value = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("{}: invalid JSON: {e}", path.display()));
        assert!(value.is_object(), "{}: not an object", path.display());
        flatten(&ns, &value, &path, &mut texts);
    }
    texts
}

fn flatten(key: &str, value: &Value, file: &Path, out: &mut BTreeMap<String, String>) {
    match value {
        Value::Object(members) => {
            for (name, nested) in members {
                assert!(
                    !name.is_empty() && !name.contains(['.', ':']),
                    "{}: key segment `{name}` under `{key}` is empty or has `.`/`:`",
                    file.display()
                );
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

/// Keys with the plural forms folded into their base key.
fn base_keys(texts: &BTreeMap<String, String>) -> BTreeSet<String> {
    texts
        .keys()
        .map(|key| {
            PLURAL_SUFFIXES
                .iter()
                .filter_map(|suffix| key.strip_suffix(suffix))
                .find(|base| texts.contains_key(&format!("{base}_other")))
                .unwrap_or(key)
                .to_owned()
        })
        .collect()
}

/// `{{arg}}` names used by a text.
fn placeholders(text: &str) -> BTreeSet<String> {
    let re = Regex::new(r"\{\{\s*([^}\s,]+)[^}]*\}\}").unwrap();
    re.captures_iter(text).map(|c| c[1].to_owned()).collect()
}

/// The built-in rules, read from `rules/*.yaml` (the same files as
/// `RuleSet::builtin`).
fn builtin_rules() -> Vec<Rule> {
    let builtin = RuleSet::builtin().unwrap();
    let mut rules = Vec::new();
    for entry in fs::read_dir(repo().join("rules")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
            continue;
        }
        let text = fs::read_to_string(&path).unwrap();
        let file =
            compile_yaml(&text).unwrap_or_else(|errors| panic!("{}: {errors:?}", path.display()));
        rules.extend(file.rules.into_iter().map(|compiled| compiled.rule));
    }
    for rule in &rules {
        assert!(
            builtin.get(&rule.id).is_some(),
            "`{}` is not built in",
            rule.id
        );
    }
    assert_eq!(rules.len(), builtin.len(), "every built-in rule is read");
    rules
}

/// `title_key`, `notes_key` and `label_key` of the built-in rules.
fn rule_keys() -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for rule in builtin_rules() {
        keys.extend(rule.title_key);
        keys.extend(rule.notes_key);
        keys.extend(rule.targets.into_iter().filter_map(|t| t.label_key));
    }
    keys
}

/// `issue.*` and `evidence.*` string literals in the crate's library code
/// (SPEC-11 §4.7: «grep по `"evidence.`, `"issue.`»).
fn emitted_keys() -> BTreeSet<String> {
    let re = Regex::new(r#""((?:issue|evidence)\.[A-Za-z0-9_.]+)""#).unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut keys = BTreeSet::new();
    for entry in fs::read_dir(src).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if !name.ends_with(".rs") || name.ends_with("_tests.rs") {
            continue;
        }
        let text = fs::read_to_string(&path).unwrap();
        keys.extend(re.captures_iter(&text).map(|c| c[1].to_owned()));
    }
    keys
}

fn missing(keys: &BTreeSet<String>, lang: &str) -> Vec<String> {
    let present = base_keys(&load_language(lang));
    keys.iter()
        .filter(|k| !present.contains(*k))
        .cloned()
        .collect()
}

#[test]
fn ru_and_en_have_the_same_keys_and_args() {
    let ru = load_language("ru");
    let en = load_language("en");
    let (ru_keys, en_keys) = (base_keys(&ru), base_keys(&en));
    let only_ru: Vec<_> = ru_keys.difference(&en_keys).collect();
    let only_en: Vec<_> = en_keys.difference(&ru_keys).collect();
    assert!(only_ru.is_empty(), "keys only in ru: {only_ru:?}");
    assert!(only_en.is_empty(), "keys only in en: {only_en:?}");

    let args = |texts: &BTreeMap<String, String>, key: &str| -> BTreeSet<String> {
        texts
            .iter()
            .filter(|(k, _)| *k == key || base_keys_of(k) == key)
            .flat_map(|(_, text)| placeholders(text))
            .collect()
    };
    for key in &ru_keys {
        assert_eq!(
            args(&ru, key),
            args(&en, key),
            "`{key}`: ru and en use different args"
        );
    }
}

/// Base key of a plural form, or the key itself.
fn base_keys_of(key: &str) -> &str {
    PLURAL_SUFFIXES
        .iter()
        .find_map(|suffix| key.strip_suffix(suffix))
        .unwrap_or(key)
}

#[test]
fn every_rule_key_has_texts_and_no_text_is_stale() {
    let keys = rule_keys();
    assert!(keys.len() > 100, "rule keys are collected: {}", keys.len());
    for key in &keys {
        assert!(
            key.starts_with("rules."),
            "`{key}` is outside the `rules` namespace"
        );
    }
    for lang in LANGUAGES {
        assert_eq!(
            missing(&keys, lang),
            Vec::<String>::new(),
            "rule keys without {lang} text"
        );
        let stale: Vec<_> = base_keys(&load_language(lang))
            .into_iter()
            .filter(|k| k.starts_with(&format!("{RULES_NS}.")) && !keys.contains(k))
            .collect();
        assert!(stale.is_empty(), "{lang}: texts of no rule: {stale:?}");
    }
}

#[test]
fn every_issue_and_evidence_key_has_texts() {
    let mut keys = emitted_keys();
    for known in [
        "evidence.rule_match",
        "evidence.rule_from_json",
        "issue.rules.invalid_file",
        "issue.rules.glob_root_truncated",
        "issue.rules.from_json_skipped",
    ] {
        assert!(keys.contains(known), "`{known}` is found in the sources");
    }
    // `message_key` of the built-in rules is the evidence of their findings.
    keys.extend(builtin_rules().iter().map(|r| r.message_key().to_owned()));
    for lang in LANGUAGES {
        assert_eq!(
            missing(&keys, lang),
            Vec::<String>::new(),
            "keys without {lang} text"
        );
    }
}

#[test]
fn nested_references_resolve() {
    let reference = Regex::new(r"\$t\(([^,)]+)").unwrap();
    for lang in LANGUAGES {
        let texts = load_language(lang);
        let keys = base_keys(&texts);
        for (key, text) in &texts {
            for found in reference.captures_iter(text) {
                let target = found[1].trim();
                match target.split_once("{{") {
                    // A key chosen by an arg: every value of the arg needs a text.
                    Some((prefix, _)) => assert!(
                        keys.iter().any(|k| k.starts_with(prefix)),
                        "{lang} `{key}`: no keys under `{prefix}`"
                    ),
                    None => assert!(
                        keys.contains(target),
                        "{lang} `{key}`: `{target}` is missing"
                    ),
                }
            }
        }
        for reason in skip_reasons() {
            let key = format!("issue.rules.from_json_skip_reason.{reason}");
            assert!(keys.contains(&key), "{lang}: `{key}` is missing");
        }
    }
}
