//! Attribution of the Ludusavi manifest (SPEC-05 FR-05-10, FR-05-11, T-05-11).
//!
//! The "About" texts live in `app/src/i18n/<lang>/about.json` under
//! `ludusavi` (full keys `about.ludusavi.*`, SPEC-11 §4.7); the same credit
//! is in `THIRD_PARTY_NOTICES.md` at the repository root.

// Helpers outside `#[test]` functions are not covered by `allow-unwrap-in-tests`.
#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

use serde_json::Value;

/// The credit line of FR-05-10, verbatim.
const CREDIT: &str = "Game save data: Ludusavi Manifest (MIT, github.com/mtkennerly/ludusavi-manifest) / PCGamingWiki (CC BY-NC-SA 3.0)";
const MIT_URL: &str = "https://github.com/mtkennerly/ludusavi-manifest/blob/master/LICENSE";
const CC_URL: &str = "https://creativecommons.org/licenses/by-nc-sa/3.0/";
/// Keys of the "About" block, without the `about.ludusavi.` prefix.
const KEYS: [&str; 5] = ["heading", "credit", "licenses", "snapshot", "terms"];

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `about.ludusavi.*` texts of one language, keyed without the prefix.
fn about(lang: &str) -> BTreeMap<String, String> {
    let path = repo().join("app/src/i18n").join(lang).join("about.json");
    let json: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap())
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let block = json["ludusavi"].as_object().unwrap();
    block
        .iter()
        .map(|(k, v)| {
            let text = v.as_str().unwrap_or_else(|| panic!("{lang}: `{k}`"));
            assert!(!text.trim().is_empty(), "{lang}: `{k}` is empty");
            (k.clone(), text.to_owned())
        })
        .collect()
}

#[test]
fn about_texts_exist_in_both_languages() {
    let expected: BTreeSet<String> = KEYS.iter().map(|k| (*k).to_owned()).collect();
    for lang in ["ru", "en"] {
        let texts = about(lang);
        let keys: BTreeSet<String> = texts.keys().cloned().collect();
        assert_eq!(keys, expected, "{lang}");
        let credit = &texts["credit"];
        assert!(
            credit.contains("Ludusavi Manifest (MIT, github.com/mtkennerly/ludusavi-manifest)"),
            "{lang}: {credit}"
        );
        assert!(
            credit.contains("PCGamingWiki (CC BY-NC-SA 3.0)"),
            "{lang}: {credit}"
        );
        let licenses = &texts["licenses"];
        assert!(licenses.contains(MIT_URL), "{lang}: {licenses}");
        assert!(licenses.contains(CC_URL), "{lang}: {licenses}");
        // No `{{args}}`: the texts are static.
        assert!(
            texts.values().all(|t| !t.contains("{{")),
            "{lang}: {texts:?}"
        );
    }
    assert_eq!(about("en")["credit"], CREDIT);
}

#[test]
fn notices_file_has_the_credit_licenses_and_the_unmodified_note() {
    let text = fs::read_to_string(repo().join("THIRD_PARTY_NOTICES.md")).unwrap();
    for needle in [
        CREDIT,
        MIT_URL,
        CC_URL,
        "**not modified**",
        "embedded-manifest",
    ] {
        assert!(text.contains(needle), "`{needle}` is missing");
    }
}

#[test]
fn the_snapshot_feature_is_on_by_default() {
    let toml = fs::read_to_string(repo().join("crates/sk-games/Cargo.toml")).unwrap();
    assert!(
        toml.contains("default = [\"embedded-manifest\"]"),
        "sk-games must embed the snapshot by default (FR-05-11)"
    );
}
