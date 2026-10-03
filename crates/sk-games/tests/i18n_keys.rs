//! i18n keys of the games collector (SPEC-05 T-05-09, SPEC-11 §4.7).
//!
//! Every `issue.games.*`, `evidence.games.*`, `evidence.ludusavi_match` and
//! `games.*` key the crate emits must have a text in both
//! `app/src/i18n/ru` and `app/src/i18n/en`; texts in these namespaces must
//! be used (emitted, or reached by a `$t(<prefix>.{{arg}})` reference whose
//! every value has a text); ru and en must have the same keys and `{{args}}`.
//!
//! Resource layout as in `sk-rules/tests/i18n_keys.rs` (SPEC-11 §4.7): the
//! files `app/src/i18n/<lang>/*.json` are merged under a top-level key equal
//! to the file name; the full key is `<file>.<path>`.

// Helpers outside `#[test]` functions are not covered by `allow-unwrap-in-tests`.
#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use sk_games::{
    BattleNetDetector, EaDetector, EpicDetector, GogDetector, LauncherDetector, SteamDetector,
    UbisoftDetector, XboxDetector,
};

/// UI languages (SPEC-11 FR-11-12).
const LANGUAGES: [&str; 2] = ["ru", "en"];

/// Prefixes of the keys owned by this crate.
const PREFIXES: [&str; 4] = [
    "issue.games.",
    "evidence.games.",
    "evidence.ludusavi_match",
    "games.",
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
            && name != "test_support.rs"
        {
            out.push(path);
        }
    }
}

/// String literals that are whole keys of this crate (`"issue.games.x"`;
/// format strings such as `"games.{launcher}"` are not keys).
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

/// Prefixes of `$t(<prefix>{{arg}})` references in the texts.
fn reference_prefixes(texts: &BTreeMap<String, String>) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for text in texts.values() {
        for (start, _) in text.match_indices("$t(") {
            let target = &text[start + 3..];
            let end = target.find(')').unwrap_or(target.len());
            if let Some((prefix, _)) = target[..end].split_once("{{") {
                out.insert(prefix.to_owned());
            }
        }
    }
    out
}

/// `reason` values of the launcher issues: the arms of `fs_reason`,
/// `INVALID` and `drive_missing` (SPEC-05 §5).
fn reasons() -> BTreeSet<String> {
    let src = crate_dir().join("src/launchers");
    let vdf = fs::read_to_string(src.join("vdf.rs")).unwrap();
    let mut out: BTreeSet<String> = vdf
        .lines()
        .filter(|l| l.contains("FsError::") && l.contains("=> \"") || l.contains("Reason = \""))
        .filter_map(|l| l.split('"').nth(1).map(str::to_owned))
        .collect();
    let steam = fs::read_to_string(src.join("steam.rs")).unwrap();
    assert!(steam.contains("\"drive_missing\""));
    out.insert("drive_missing".to_owned());
    assert!(
        out.contains("not_found") && out.contains("invalid") && out.len() >= 9,
        "{out:?}"
    );
    out
}

#[test]
fn every_emitted_key_has_texts() {
    let keys = emitted_keys();
    for known in [
        "issue.games.manifest_offline",
        "issue.games.manifest_invalid",
        "issue.games.manifest_cache_failed",
        "issue.games.steam_libraryfolders_unreadable",
        "issue.games.steam_library_unavailable",
        "issue.games.steam_appmanifest_unreadable",
        "issue.games.launcher_file_unreadable",
        "issue.games.registry_hklm_skipped",
        "evidence.ludusavi_match",
        "evidence.games.installed",
        "evidence.games.install_dir",
        "evidence.games.xbox_wgs",
        "evidence.games.ubisoft_savegames",
        "games.title.save",
        "games.title.config",
        "games.title.install_dir",
        "games.note.xbox_wgs",
        "games.note.reinstallable",
    ] {
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
        let prefixes = reference_prefixes(&texts);
        let stale: Vec<_> = texts
            .keys()
            .filter(|k| owned(k) && !keys.contains(*k))
            .filter(|k| !prefixes.iter().any(|p| k.starts_with(p.as_str())))
            .collect();
        assert!(stale.is_empty(), "{lang}: texts nothing uses: {stale:?}");
    }
}

#[test]
fn references_reach_a_text_for_every_value() {
    let launchers = [
        SteamDetector::new().id(),
        EpicDetector::new().id(),
        GogDetector::new().id(),
        UbisoftDetector::new().id(),
        EaDetector::new().id(),
        BattleNetDetector::new().id(),
        XboxDetector::new().id(),
    ];
    for lang in LANGUAGES {
        let texts = load_language(lang);
        let prefixes = reference_prefixes(&texts);
        assert!(prefixes.contains("games.launcher."), "{prefixes:?}");
        assert!(
            prefixes.contains("issue.games.file_reason."),
            "{prefixes:?}"
        );
        for id in launchers {
            let key = format!("games.launcher.{id}");
            assert!(texts.contains_key(&key), "{lang}: `{key}` is missing");
        }
        for reason in reasons() {
            let key = format!("issue.games.file_reason.{reason}");
            assert!(texts.contains_key(&key), "{lang}: `{key}` is missing");
        }
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
