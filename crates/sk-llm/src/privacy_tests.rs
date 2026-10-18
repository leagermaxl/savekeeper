use std::path::Path;

use sk_core::env::Environment;

use super::*;

fn env() -> Environment {
    let mut env = Environment::fake(Path::new(if cfg!(windows) { r"C:\fake" } else { "/fake" }));
    env.user_name = "Max".to_owned();
    env.machine_name = "DESKTOP-01".to_owned();
    env
}

fn check(json: &str) -> Result<(), PrivacyViolation> {
    final_check(json, &env())
}

/// Serializes `name` as one sample name of an element.
fn element(name: &str) -> String {
    serde_json::json!({
        "id": "0123456789abcdef",
        "summary": { "path": r"{APPDATA}\App", "sample_names": [name] }
    })
    .to_string()
}

#[test]
fn clean_element_passes() {
    assert_eq!(check(&element("settings.json")), Ok(()));
    assert_eq!(check(&element(r"{DRIVE:D}\Games\save.dat")), Ok(()));
    assert_eq!(check(&element("<redacted>-notes.txt")), Ok(()));
    assert_eq!(check(&element("maximum and Maxwell")), Ok(()));
    assert_eq!(check(&element("save_2024_final.sav")), Ok(()));
    assert_eq!(check(&element("see https://example")), Ok(()));
}

#[test]
fn catches_user_name() {
    assert_eq!(
        check(&element("MAX-notes.txt")),
        Err(PrivacyViolation::UserName)
    );
    assert_eq!(
        check(&element(r"{HOME}\max\x")),
        Err(PrivacyViolation::UserName)
    );
    // An escape right before the name does not hide it.
    let peek = serde_json::json!({ "peek": [{ "file": "a.ini", "head": "a=1\nmax=2" }] });
    assert_eq!(check(&peek.to_string()), Err(PrivacyViolation::UserName));
}

#[test]
fn unicode_user_name_ignores_case() {
    let mut env = env();
    env.user_name = "Макс".to_owned();
    assert_eq!(
        final_check(&element("МАКС_old"), &env),
        Err(PrivacyViolation::UserName)
    );
    assert_eq!(final_check(&element("Максим"), &env), Ok(()));
}

#[test]
fn catches_machine_name() {
    assert_eq!(
        check(&element("backup of desktop-01")),
        Err(PrivacyViolation::MachineName)
    );
    assert_eq!(check(&element("DESKTOP-012")), Ok(()));
}

#[test]
fn short_names_are_ignored() {
    let mut env = env();
    env.user_name = "a".to_owned();
    assert_eq!(final_check(&element("a b a"), &env), Ok(()));
}

#[test]
fn catches_absolute_paths() {
    for path in [
        r"C:\Users\someone\x",
        r"d:\games",
        "E:/data/file.txt",
        r"at C:\Windows",
    ] {
        assert_eq!(
            check(&element(path)),
            Err(PrivacyViolation::AbsolutePath),
            "{path}"
        );
    }
    // The key of the JSON and its escaping do not matter.
    assert_eq!(
        check(r#"{"C:\\Users\\x": 1}"#),
        Err(PrivacyViolation::AbsolutePath)
    );
}

#[test]
fn catches_emails() {
    assert_eq!(
        check(&element("mail john.doe+x@lea-soft.org now")),
        Err(PrivacyViolation::Email)
    );
    assert_eq!(
        check(&element("write to a_b@example.com.")),
        Err(PrivacyViolation::Email)
    );
    for kept in ["a@b", "user@localhost", "x@y.c", "@example.com", "v1@2.0"] {
        assert_eq!(check(&element(kept)), Ok(()), "{kept}");
    }
}

#[test]
fn catches_tokens() {
    assert_eq!(
        check(&element("ghp_1234567890abcdefghijABCDEFGHIJ123456")),
        Err(PrivacyViolation::Token)
    );
    assert_eq!(
        check(&element("abcdefghijklmnopqrstuvwx")),
        Err(PrivacyViolation::Token)
    );
    assert_eq!(check(&element("bcdefghijklmnopqrstuvwx")), Ok(()));
}

#[test]
fn checks_text_that_is_not_json() {
    assert_eq!(
        check(r"not json C:\Users\x"),
        Err(PrivacyViolation::AbsolutePath)
    );
    assert_eq!(check("not json { at all"), Ok(()));
}

#[test]
fn redacted_summary_always_passes() {
    let env = env();
    let names = [
        r"C:\Users\Max\file.txt",
        "max@example.com",
        "token sk-ant-api03-AbCdEfGhIjKlMnOpQrStUv",
        "DESKTOP-01 backup",
    ];
    for name in names {
        let redacted = sk_core::privacy::redact(name, &env);
        // `redact` does not remove drive letters: the template step does.
        let redacted = redacted.replace(r"C:\", r"{DRIVE:C}\");
        assert_eq!(final_check(&element(&redacted), &env), Ok(()), "{name}");
    }
}

#[test]
fn violation_message_has_no_matched_text() {
    let msg = PrivacyViolation::UserName.to_string();
    assert!(!msg.to_lowercase().contains("max"));
}

#[test]
fn drive_with_backslash_is_caught_after_any_character() {
    assert_eq!(
        check(&element(r"dataC:\Users\x")),
        Err(PrivacyViolation::AbsolutePath)
    );
    assert_eq!(
        check(r#"{"a":"dataC:\\Users\\x"}"#),
        Err(PrivacyViolation::AbsolutePath)
    );
    assert_eq!(check(&element("see https://x")), Ok(()));
    assert_eq!(check(&element("file:/x and ftp:/y")), Ok(()));
}

#[test]
fn duplicate_keys_do_not_hide_data() {
    // A JSON parser keeps only the last value of a repeated key.
    assert_eq!(
        check(r#"{"a":"Max","a":"x"}"#),
        Err(PrivacyViolation::UserName)
    );
    assert_eq!(
        check(r#"{"a":"C:\\Users\\x","a":"x"}"#),
        Err(PrivacyViolation::AbsolutePath)
    );
}

#[test]
fn unicode_escapes_do_not_hide_data() {
    // Valid JSON.
    assert_eq!(
        check(r#"{"a":"\u004dax"}"#),
        Err(PrivacyViolation::UserName)
    );
    // Invalid JSON: decoded leniently.
    assert_eq!(
        check(r#"{"a":"\u004dax" broken"#),
        Err(PrivacyViolation::UserName)
    );
    assert_eq!(
        check(r#"{"a":"C\u003a\u005cUsers" broken"#),
        Err(PrivacyViolation::AbsolutePath)
    );
    // An escaped backslash before a literal `u` is not an escape.
    assert_eq!(check(r#"{"a":"\\u004dax"}"#), Ok(()));
}

#[test]
fn lenient_unescape() {
    assert_eq!(unescape_lenient(r#"a\nb\t\"\\\/"#), "a\nb\t\"\\/");
    assert_eq!(unescape_lenient(r"\u0041\uD83D\uDE00"), "A\u{1F600}");
    assert_eq!(unescape_lenient(r"\uD83D x"), "\u{FFFD} x");
    assert_eq!(unescape_lenient(r"\q \u12 end\"), r"\q \u12 end\");
}
