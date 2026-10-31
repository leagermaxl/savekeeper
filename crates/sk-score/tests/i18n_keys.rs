//! i18n keys of the scoring phase (SPEC-09 T-09-10, SPEC-11 §4.7).
//!
//! Every key `sk-score` emits — the `scoring.*` issues and the
//! `evidence.scoring_fallback` evidence — and the texts the UI shows for
//! «why this score» — `scoring.component.<key>` for every `Score.components`
//! key (§4.5) and `scoring.tag.<tag>` for every tag the crate adds (§4.3,
//! §4.4, §4.7, §5) — must have a text in both `app/src/i18n/ru` and
//! `app/src/i18n/en`; texts in the `scoring.` namespace must be used; ru and
//! en must have the same keys and `{{args}}`, and an issue text uses only the
//! args the code passes. Category args are translated by a
//! `$t(category.{{arg}})` reference (SPEC-11 §4.7), which must reach a text for
//! every `Category` value.
//!
//! Resource layout as in `sk-games/tests/i18n_keys.rs` (SPEC-11 §4.7): the
//! files `app/src/i18n/<lang>/*.json` are merged under a top-level key equal
//! to the file name; the full key is `<file>.<path>`.

// Helpers outside `#[test]` functions are not covered by `allow-unwrap-in-tests`.
#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use sk_core::model::Category;
use sk_score::{
    ADJUSTMENTS, ALTERNATIVE_TAG, BAD_CONFIG_KEY, BAD_NUMBER_KEY, BASE, BELOW_THRESHOLD_TAG,
    CATEGORY_CONFLICT_KEY, CONFIDENCE, CONFLICT_TAG, IRREPLACEABILITY, NEEDS_ELEVATION_TAG,
    NESTED_TAG, NO_EVIDENCE_KEY, RECENCY, RESTORABLE_TAG, SENSITIVE_SELECTED_KEY, SIZE_PENALTY,
    TOO_LARGE_DEFAULT_TAG, UNMEASURED_TAG, USER_AUTHORED, VALUE,
};

/// UI languages (SPEC-11 FR-11-12).
const LANGUAGES: [&str; 2] = ["ru", "en"];

/// Namespace of the texts owned by this crate.
const PREFIX: &str = "scoring.";

/// Message key of the synthetic fallback evidence (SPEC-09 §5); the constant
/// is crate-private, `emitted_message_keys` checks it is in the sources.
const FALLBACK_EVIDENCE_KEY: &str = "evidence.scoring_fallback";

/// Prefix of the label of a `Score.components` key.
const COMPONENT_PREFIX: &str = "scoring.component.";

/// Prefix of the explanation of a tag.
const TAG_PREFIX: &str = "scoring.tag.";

/// `Score.components` keys (SPEC-09 §4.5).
const COMPONENTS: [&str; 8] = [
    IRREPLACEABILITY,
    USER_AUTHORED,
    RECENCY,
    CONFIDENCE,
    SIZE_PENALTY,
    ADJUSTMENTS,
    BASE,
    VALUE,
];

/// Tags `sk-score` adds to findings: the reason tags of §4.7 and the
/// `conflict` (§4.3) and `unmeasured` (§5) tags.
const TAGS: [&str; 8] = [
    NESTED_TAG,
    ALTERNATIVE_TAG,
    RESTORABLE_TAG,
    NEEDS_ELEVATION_TAG,
    TOO_LARGE_DEFAULT_TAG,
    BELOW_THRESHOLD_TAG,
    CONFLICT_TAG,
    UNMEASURED_TAG,
];

/// Arguments of every issue the crate emits (`message_args` keys).
const ISSUE_ARGS: [(&str, &[&str]); 5] = [
    (BAD_CONFIG_KEY, &["field", "value"]),
    (CATEGORY_CONFLICT_KEY, &["finding_id", "winner", "loser"]),
    (BAD_NUMBER_KEY, &["finding_id", "field", "value"]),
    (NO_EVIDENCE_KEY, &["finding_id"]),
    (SENSITIVE_SELECTED_KEY, &["count"]),
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
    key.starts_with(PREFIX) || key == FALLBACK_EVIDENCE_KEY
}

/// Every key that must have a text.
fn required_keys() -> BTreeSet<String> {
    ISSUE_ARGS
        .iter()
        .map(|(key, _)| (*key).to_owned())
        .chain([FALLBACK_EVIDENCE_KEY.to_owned()])
        .chain(COMPONENTS.iter().map(|c| format!("{COMPONENT_PREFIX}{c}")))
        .chain(TAGS.iter().map(|t| format!("{TAG_PREFIX}{t}")))
        .collect()
}

/// Library sources of the crate (tests excluded).
fn sources() -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    for entry in fs::read_dir(crate_dir().join("src")).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if name.ends_with(".rs") && !name.ends_with("_tests.rs") {
            let text = fs::read_to_string(&path).unwrap();
            out.push((path, text));
        }
    }
    out
}

/// Values of the string constants whose name ends with `suffix`
/// (`pub const NAME_KEY: &str = "value";`); `public` keeps only `pub` ones.
fn const_values(suffix: &str, public: bool) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for (_, text) in sources() {
        for line in text.lines() {
            let line = line.trim();
            let decl = if public {
                line.strip_prefix("pub const ")
            } else {
                line.split_once("const ").map(|(_, rest)| rest)
            };
            let Some(decl) = decl else { continue };
            let Some((name, value)) = decl.split_once(": &str = \"") else {
                continue;
            };
            if name.ends_with(suffix) {
                out.insert(value.trim_end_matches("\";").to_owned());
            }
        }
    }
    out
}

/// Prefix of the category texts, `category.<snake_case name>` (SPEC-11 §4.7).
const CATEGORY_PREFIX: &str = "category.";

/// Every `Category` value.
fn all_categories() -> Vec<Category> {
    let all = vec![
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
    // Exhaustive: a new variant fails to compile here until it is added above.
    for category in &all {
        match category {
            Category::GameSave
            | Category::GameConfig
            | Category::AppConfig
            | Category::AppData
            | Category::UserFiles
            | Category::DevEnvironment
            | Category::Credentials
            | Category::SystemSettings
            | Category::Reinstallable
            | Category::Cache
            | Category::Unknown => {}
        }
    }
    all
}

/// The serde (snake_case) name of a category, as the issues carry it.
fn category_name(category: Category) -> String {
    match serde_json::to_value(category).unwrap() {
        Value::String(name) => name,
        other => panic!("{category:?} serializes as {other}"),
    }
}

/// Prefixes of the `$t(<prefix>{{arg}})` references in a text.
fn reference_prefixes(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for (start, _) in text.match_indices("$t(") {
        let target = &text[start + 3..];
        let end = target.find(')').unwrap_or(target.len());
        let (prefix, _) = target[..end]
            .split_once("{{")
            .unwrap_or_else(|| panic!("`{text}`: reference without an arg"));
        out.insert(prefix.to_owned());
    }
    out
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
fn every_key_has_texts() {
    for lang in LANGUAGES {
        let texts = load_language(lang);
        let missing: Vec<_> = required_keys()
            .into_iter()
            .filter(|k| !texts.contains_key(k))
            .collect();
        assert!(missing.is_empty(), "keys without {lang} text: {missing:?}");
    }
}

#[test]
fn every_message_key_in_the_sources_is_listed() {
    let required = required_keys();
    let keys = const_values("_KEY", false);
    assert!(keys.contains(FALLBACK_EVIDENCE_KEY), "{keys:?}");
    assert_eq!(keys.len(), ISSUE_ARGS.len() + 1, "{keys:?}");
    for key in keys {
        assert!(required.contains(&key), "`{key}` has no listed text");
    }
}

#[test]
fn every_tag_in_the_sources_is_listed() {
    let tags = const_values("_TAG", true);
    let listed: BTreeSet<String> = TAGS.iter().map(|t| (*t).to_owned()).collect();
    assert_eq!(tags, listed, "`TAGS` is out of date");
}

#[test]
fn every_text_is_used() {
    let required = required_keys();
    for lang in LANGUAGES {
        let stale: Vec<_> = load_language(lang)
            .into_keys()
            .filter(|k| owned(k) && !required.contains(k))
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

#[test]
fn texts_use_only_the_given_args() {
    let issue_args: BTreeMap<&str, BTreeSet<String>> = ISSUE_ARGS
        .iter()
        .map(|(key, args)| (*key, args.iter().map(|a| (*a).to_owned()).collect()))
        .collect();
    for lang in LANGUAGES {
        let texts = load_language(lang);
        for (key, text) in texts.iter().filter(|(k, _)| owned(k)) {
            let given = issue_args.get(key.as_str()).cloned().unwrap_or_default();
            let used = placeholders(text);
            assert!(
                used.is_subset(&given),
                "{lang}: `{key}` uses {used:?}, the code passes {given:?}"
            );
        }
    }
}

#[test]
fn references_reach_a_text_for_every_value() {
    let categories = all_categories();
    assert_eq!(
        categories.len(),
        sk_score::default_weights().len(),
        "every category has default weights"
    );
    for lang in LANGUAGES {
        let texts = load_language(lang);
        let conflict = &texts[CATEGORY_CONFLICT_KEY];
        assert!(
            conflict.contains("$t(category.{{winner}})")
                && conflict.contains("$t(category.{{loser}})"),
            "{lang}: `{CATEGORY_CONFLICT_KEY}` does not translate the categories"
        );
        for (key, text) in texts.iter().filter(|(k, _)| owned(k)) {
            for prefix in reference_prefixes(text) {
                assert_eq!(
                    prefix, CATEGORY_PREFIX,
                    "{lang}: `{key}` refers to `{prefix}`"
                );
            }
        }
        for category in &categories {
            let key = format!("{CATEGORY_PREFIX}{}", category_name(*category));
            assert!(texts.contains_key(&key), "{lang}: `{key}` is missing");
        }
    }
}
