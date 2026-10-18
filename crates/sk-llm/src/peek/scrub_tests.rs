//! Tests of the content peek scrub (SPEC-08 §4.9, §6.1).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sk_core::env::{Environment, KnownFolder};

use super::*;
use crate::privacy::final_check;

fn env() -> Environment {
    let mut env = Environment::fake(Path::new(if cfg!(windows) { r"C:\fake" } else { "/fake" }));
    env.user_name = "Max".to_owned();
    env.machine_name = "DESKTOP-01".to_owned();
    env.known_folders = BTreeMap::from([
        (KnownFolder::Home, PathBuf::from(r"C:\Users\Max")),
        (
            KnownFolder::AppData,
            PathBuf::from(r"C:\Users\Max\AppData\Roaming"),
        ),
        (
            KnownFolder::LocalAppData,
            PathBuf::from(r"C:\Users\Max\AppData\Local"),
        ),
    ]);
    env
}

fn scrub(text: &str) -> String {
    scrub_secrets(text, &env())
}

#[test]
fn secret_keys() {
    for key in [
        "password",
        "Pass",
        "db_pwd",
        "client_secret",
        "AccessToken",
        "api_key",
        "API-KEY",
        "apikey",
        "Authorization",
        "cookie",
        "SessionId",
        "private_key",
        "Bearer",
        "credentials",
    ] {
        assert!(is_secret_key(key), "{key}");
    }
    for key in ["user", "port", "volume", "theme", "language"] {
        assert!(!is_secret_key(key), "{key}");
    }
}

#[test]
fn ini_passwords() {
    let text = "[db]\nuser = bob\npassword = hunter2\nPWD=1234\napi-key: abc\n[session]\nport=5432";
    assert_eq!(
        scrub(text),
        "[db]\nuser = bob\npassword = <redacted>\nPWD=<redacted>\napi-key: <redacted>\n[session]\nport=5432"
    );
}

#[test]
fn quoted_and_empty_values() {
    assert_eq!(scrub("token = \"abc\""), "token = \"<redacted>\"");
    assert_eq!(scrub("secret: 'x y'"), "secret: '<redacted>'");
    assert_eq!(scrub("auth:"), "auth:");
    assert_eq!(scrub("  - password: x"), "  - password: <redacted>");
    assert_eq!(scrub("token = \"\""), "token = \"\"");
}

#[test]
fn json_passwords() {
    let text = "{\n  \"user\": \"bob\",\n  \"password\": \"hunter2\",\n  \"auth\": {\n    \"token\": 12345,\n    \"port\": 1\n  }\n}";
    assert_eq!(
        scrub(text),
        "{\n  \"user\": \"bob\",\n  \"password\": \"<redacted>\",\n  \"auth\": {\n    <redacted>\n    <redacted>\n  }\n}"
    );
}

#[test]
fn minified_json() {
    let text = r#"{"user":"bob","password":"hun\"ter","port":5432,"apiKey":"k","list":["a"]}"#;
    assert_eq!(
        scrub(text),
        r#"{"user":"bob","password":"<redacted>","port":5432,"apiKey":"<redacted>","list":["a"]}"#
    );
}

#[test]
fn xml_passwords() {
    let text = concat!(
        "<config>\n",
        "  <user>bob</user>\n",
        "  <Password>hunter2</Password>\n",
        "  <db host=\"x\" password=\"p1\" token='t1'/>\n",
        "  <add key=\"DbPassword\" value=\"p2\" />\n",
        "  <add key=\"Theme\" value=\"dark\" />\n",
        "  <secret>\n",
        "    multi line\n",
        "  </secret>\n",
        "</config>"
    );
    assert_eq!(
        scrub(text),
        concat!(
            "<config>\n",
            "  <user>bob</user>\n",
            "  <Password><redacted></Password>\n",
            "  <db host=\"x\" password=\"<redacted>\" token='<redacted>'/>\n",
            "  <add key=\"DbPassword\" value=\"<redacted>\" />\n",
            "  <add key=\"Theme\" value=\"dark\" />\n",
            "  <secret>\n",
            "    <redacted>\n",
            "  </secret>\n",
            "</config>"
        )
    );
}

#[test]
fn crlf_is_kept() {
    assert_eq!(
        scrub("user=bob\r\npassword=x\r\n"),
        "user=bob\r\npassword=<redacted>\r\n"
    );
}

#[test]
fn long_runs() {
    let b64 = "QUJDREVGR0hJSktMTU5PUFFSU1RVVldY+/==";
    assert_eq!(scrub(&format!("data {b64} end")), "data <redacted> end");
    let short = "a".repeat(LONG_RUN_MIN_LEN - 1);
    assert_eq!(scrub(&format!("x {short} y")), format!("x {short} y"));
    let long = "a".repeat(LONG_RUN_MIN_LEN);
    assert_eq!(scrub(&format!("x {long} y")), "x <redacted> y");
}

#[test]
fn emails_ips_and_credential_urls() {
    assert_eq!(scrub("mail me: bob@example.org."), "mail me: <redacted>.");
    assert_eq!(scrub("host 192.168.1.10."), "host <redacted>.");
    assert_eq!(scrub("server=10.0.0.1:8080"), "server=<redacted>:8080");
    assert_eq!(
        scrub("v1.2.3.4 and 1.2.3 and 999.1.1.1"),
        "v1.2.3.4 and 1.2.3 and 999.1.1.1"
    );
    assert_eq!(scrub("ip fe80::1ff:fe23:4567:890a"), "ip <redacted>");
    assert_eq!(scrub("[::1]:80"), "[<redacted>]:80");
    assert_eq!(scrub("at 12:30:45 std::cout"), "at 12:30:45 std::cout");
    assert_eq!(
        scrub("url = https://bob:pw@db.example.org/x y"),
        "url = <redacted> y"
    );
    assert_eq!(
        scrub("see https://example.org/a"),
        "see https://example.org/a"
    );
    assert_eq!(scrub("ftp://user@host/x"), "ftp://user@host/x");
}

#[test]
fn absolute_paths_become_tokens() {
    assert_eq!(
        scrub(r"dir=C:\Users\Max\AppData\Roaming\App"),
        r"dir={APPDATA}\App"
    );
    assert_eq!(
        scrub(r#""dir": "c:\\users\\max\\AppData\\Local\\X""#),
        r#""dir": "{LOCALAPPDATA}\\X""#
    );
    assert_eq!(scrub("home C:/Users/Max/x"), "home {HOME}/x");
    assert_eq!(scrub(r"C:\Users\Maxim\x"), r"{DRIVE:C}\Users\Maxim\x");
    assert_eq!(scrub(r"lib=d:\Games\x"), r"lib={DRIVE:D}\Games\x");
    assert_eq!(scrub("https://example.org"), "https://example.org");
}

#[test]
fn names_are_redacted() {
    assert_eq!(
        scrub("owner = Max on DESKTOP-01"),
        "owner = <redacted> on <redacted>"
    );
}

#[test]
fn scrubbed_heads_pass_the_final_check() {
    let text = concat!(
        "[account]\nname=Max\nemail = max@example.org\npassword=hunter2\n",
        "path=C:\\Users\\Max\\Documents\\notes\n",
        "other=E:\\Data\\x\nkey2=",
        "ZXhhbXBsZS10b2tlbi12YWx1ZS0xMjM0NTY3OA==",
        "\n",
        "proxy=http://u:p@10.1.2.3:3128\nhost=DESKTOP-01\n",
    );
    let head = scrub(text);
    let json = serde_json::json!({ "peek": [{ "file": "a.ini", "head": head }] }).to_string();
    assert_eq!(final_check(&json, &env()), Ok(()), "{head}");
}
