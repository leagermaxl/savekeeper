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
    // An escaped backslash before a literal `u`: the string holds the text
    // `Max`, which the model can decode, so the second round catches it.
    assert_eq!(
        check(r#"{"a":"\\u004dax"}"#),
        Err(PrivacyViolation::UserName)
    );
    assert_eq!(check(r#"{"a":"\\u004d"}"#), Ok(()));
}

#[test]
fn lenient_unescape() {
    assert_eq!(unescape_lenient(r#"a\nb\t\"\\\/"#), "a\nb\t\"\\/");
    assert_eq!(unescape_lenient(r"\u0041\uD83D\uDE00"), "A\u{1F600}");
    assert_eq!(unescape_lenient(r"\uD83D x"), "\u{FFFD} x");
    assert_eq!(unescape_lenient(r"\q \u12 end\"), r"\q \u12 end\");
}

#[test]
fn catches_names_escaped_twice() {
    let mut env = env();
    env.user_name = "Макс".to_owned();
    // The text of a file holding `C:\Users\Макс` as JSON escapes, sent
    // inside a JSON string.
    let head = r"{DRIVE:C}\\Users\\\u041c\u0430\u043a\u0441\\AppData";
    let json = serde_json::json!({ "peek": [{ "file": "a.json", "head": head }] }).to_string();
    assert_eq!(final_check(&json, &env), Err(PrivacyViolation::UserName));
    let quad = serde_json::json!({ "x": json }).to_string();
    assert_eq!(final_check(&quad, &env), Err(PrivacyViolation::UserName));
}

#[test]
fn json_escapes_after_a_colon_are_not_paths_in_the_raw_view() {
    for head in [
        "services:\n  web:",
        "a:\tb",
        "a:\rb",
        "x:\"",
        "a:\u{8}b",
        "a:\u{1}",
    ] {
        let json = serde_json::json!({ "head": head }).to_string();
        assert_eq!(check(&json), Ok(()), "{json}");
    }
    // `\/` decodes to `/`: `ab:/` is not a drive (letter before the drive letter).
    assert_eq!(check(r#"{"h":"ab:\/b"}"#), Ok(()));
    assert_eq!(check(r#"{"h":"a:\u0041"}"#), Ok(()));
    // Real paths are still caught: escaped backslash, `\u005C`, decoded views.
    assert_eq!(
        check(r#"{"h":"C:\\Users"}"#),
        Err(PrivacyViolation::AbsolutePath)
    );
    assert_eq!(
        check(r#"{"h":"C:\u005cUsers"}"#),
        Err(PrivacyViolation::AbsolutePath)
    );
    assert_eq!(
        check(r#"{"h":"C:\u005CUsers"}"#),
        Err(PrivacyViolation::AbsolutePath)
    );
    assert_eq!(check(r"C:\q"), Err(PrivacyViolation::AbsolutePath));
    assert_eq!(check(r"C:\"), Err(PrivacyViolation::AbsolutePath));
}

// Token exceptions for real Windows folder names (T-08-15).

const TERMINAL: &str = "Microsoft.WindowsTerminal_8wekyb3d8bbwe";
const SID: &str = "S-1-5-21-1234567890-2345678901-3456789012-1001";
const JUMP_LIST_EXT: &str = "automaticdestinations-ms";
const TOKEN: &str = "ghp_a1B2c3D4e5F6g7H8i9J0kLmNoPq";

fn summary(path: &str, ext: &str, name: &str) -> serde_json::Value {
    serde_json::json!({
        "path": path,
        "ext_histogram": [{ "ext": ext, "count": 1, "bytes": 10 }],
        "sample_names": [name]
    })
}

/// Serializes one element with this path, extension and sample name.
fn item(path: &str, ext: &str, name: &str) -> String {
    let summary = summary(path, ext, name);
    serde_json::json!({ "id": "0123456789abcdef", "summary": summary }).to_string()
}

fn token_in(path: &str, ext: &str, name: &str) -> bool {
    check(&item(path, ext, name)) == Err(PrivacyViolation::Token)
}

#[test]
fn uwp_package_family_name_in_path_passes() {
    let path = format!(r"{{LOCALAPPDATA}}\Packages\{TERMINAL}\LocalState");
    assert_eq!(check(&item(&path, "json", "settings.json")), Ok(()));
    let start = r"{LOCALAPPDATA}\Packages\Microsoft.Windows.StartMenuExperienceHost_cw5n1h2txyewy";
    assert_eq!(check(&item(start, "bin", "start2.bin")), Ok(()));
    let edge = r"{LOCALAPPDATA}\Packages\Microsoft.MicrosoftEdgeDevToolsClient_8wekyb3d8bbwe";
    assert_eq!(check(&item(edge, "json", "a.json")), Ok(()));
}

#[test]
fn sid_in_path_passes() {
    let path = format!(r"{{APPDATA}}\Microsoft\Protect\{SID}");
    assert_eq!(check(&item(&path, "", "Preferred")), Ok(()));
}

#[test]
fn long_extension_in_ext_histogram_passes() {
    let path = r"{APPDATA}\Microsoft\Windows\Recent\AutomaticDestinations";
    assert_eq!(check(&item(path, JUMP_LIST_EXT, "x.lnk")), Ok(()));
}

#[test]
fn batch_with_repeated_exempt_values_passes() {
    let path = format!(r"{{LOCALAPPDATA}}\Packages\{TERMINAL}\LocalState");
    let one = summary(&path, JUMP_LIST_EXT, "a.json");
    let json = serde_json::json!({ "items": [{ "summary": one }, { "summary": one }] });
    assert_eq!(check(&json.to_string()), Ok(()));
}

#[test]
fn real_tokens_are_still_caught() {
    // In `path`, as a whole folder name and next to an exempt name.
    assert!(token_in(&format!(r"{{APPDATA}}\App\{TOKEN}"), "json", "a"));
    assert!(token_in(
        &format!(r"{{APPDATA}}\{TERMINAL}\{TOKEN}"),
        "json",
        "a"
    ));
    // In `sample_names`.
    assert!(token_in(r"{APPDATA}\App", "json", TOKEN));
    // Exempt shapes are exempt only in their own fields.
    for name in [TERMINAL, SID, JUMP_LIST_EXT] {
        assert!(token_in(r"{APPDATA}\App", "json", name), "{name}");
    }
    assert!(token_in(
        &format!(r"{{APPDATA}}\{JUMP_LIST_EXT}"),
        "json",
        "a"
    ));
    let ext_elsewhere = serde_json::json!({ "ext": JUMP_LIST_EXT }).to_string();
    assert_eq!(check(&ext_elsewhere), Err(PrivacyViolation::Token));
    // A value in an exempt field does not cover the same run elsewhere.
    assert!(token_in(&format!(r"{{APPDATA}}\Protect\{SID}"), "", SID));
    assert!(token_in(r"{APPDATA}\App", JUMP_LIST_EXT, JUMP_LIST_EXT));
}

#[test]
fn near_misses_of_exempt_shapes_are_tokens() {
    for name in [
        // Upper case or wrong length of the publisher id, no `_`.
        "Microsoft.WindowsTerminal_8WEKYB3D8BBWE",
        "Microsoft.WindowsTerminal_8wekyb3d8bbwe1",
        "Microsoft.WindowsTerminal-8wekyb3d8bbwe",
        // Not a SID.
        "S-1-5212345678902345678901234",
        "s-1-5-21-1234567890-2345678901-3456789012",
        "S-1-5-21-1234567890-2345678901-x456789012",
        "S-1-5-21-1234567890--2345678901-3456789012",
    ] {
        assert!(
            token_in(&format!(r"{{APPDATA}}\{name}"), "json", "a"),
            "{name}"
        );
    }
}

#[test]
fn exceptions_do_not_survive_duplicate_keys_or_invalid_json() {
    let path = format!(r"{{APPDATA}}\Protect\{SID}");
    let path = serde_json::Value::String(path).to_string();
    let dup = format!(r#"{{"path":{path},"path":{path}}}"#);
    assert_eq!(check(&dup), Err(PrivacyViolation::Token));
    let broken = format!(r#"{{"path":{path} broken"#);
    assert_eq!(check(&broken), Err(PrivacyViolation::Token));
}

#[test]
fn other_rules_still_apply_in_exempt_fields() {
    let path = format!(r"{{LOCALAPPDATA}}\Packages\{TERMINAL}\Max");
    let json = item(&path, "json", "a");
    assert_eq!(check(&json), Err(PrivacyViolation::UserName));
    let json = item(r"{APPDATA}\App", "a@example.org", "a");
    assert_eq!(check(&json), Err(PrivacyViolation::Email));
}
