//! Review probes of the content peek scrub (SPEC-08 §4.9): secrets inside
//! other values, XML forms, whitespace keys, YAML blocks, arrays, JSON
//! escapes and the `final_check` raw view.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sk_core::env::{Environment, KnownFolder};

use super::*;
use crate::privacy::final_check;

fn env() -> Environment {
    let mut env = Environment::fake(Path::new(if cfg!(windows) { r"C:\fake" } else { "/fake" }));
    env.user_name = "Max".to_owned();
    env.machine_name = "DESKTOP-01".to_owned();
    env.known_folders = BTreeMap::from([(KnownFolder::Home, PathBuf::from(r"C:\Users\Max"))]);
    env
}

fn scrub(text: &str) -> String {
    scrub_secrets(text, &env())
}

/// The head as `content_peek` would send it passes `final_check`.
fn passes(head: &str, env: &Environment) -> bool {
    let json = serde_json::json!({ "file": "a.txt", "head": head }).to_string();
    final_check(&json, env).is_ok()
}

#[test]
fn secret_pairs_inside_other_values() {
    assert_eq!(
        scrub("Server=db;User Id=sa;Password=hunter2;"),
        "Server=db;User Id=sa;Password=<redacted>;"
    );
    assert_eq!(
        scrub(r#""Default": "Server=.;Password=hunter2""#),
        r#""Default": "Server=.;Password=<redacted>""#
    );
    assert_eq!(
        scrub(r#"<add name="Default" connectionString="Server=.;Password=hunter2"/>"#),
        r#"<add name="Default" connectionString="Server=.;Password=<redacted>"/>"#
    );
    assert_eq!(
        scrub("url=https://x.org/api?access_token=hunter2&a=1"),
        "url=https://x.org/api?access_token=<redacted>&a=1"
    );
    assert_eq!(
        scrub(r#""args": "--password=hunter2""#),
        r#""args": "--password=<redacted>""#
    );
    assert_eq!(
        scrub(r#""h": "Authorization: Bearer hunter2""#),
        r#""h": "Authorization: <redacted>""#
    );
    assert_eq!(scrub("std::token::x a==b"), "std::token::x a==b");
}

#[test]
fn xml_values_by_key_name_child_and_cdata() {
    assert_eq!(
        scrub(r#"<setting name="Password" serializeAs="String"><value>hunter2</value></setting>"#),
        r#"<setting name="Password" serializeAs="String"><value><redacted></value></setting>"#
    );
    assert_eq!(
        scrub("<setting name=\"Password\" serializeAs=\"String\">\n  <value>hunter2</value>\n</setting>\n<x>keep</x>"),
        "<setting name=\"Password\" serializeAs=\"String\">\n  <value><redacted></value>\n</setting>\n<x>keep</x>"
    );
    assert_eq!(
        scrub(r#"<entry key="db.password">hunter2</entry><entry key="db.user">bob</entry>"#),
        r#"<entry key="db.password"><redacted></entry><entry key="db.user">bob</entry>"#
    );
    assert_eq!(
        scrub("<password><![CDATA[hunter2]]></password>"),
        "<password><![CDATA[<redacted>]]></password>"
    );
    assert_eq!(
        scrub("<user><![CDATA[bob]]></user> <!-- password -->"),
        "<user><![CDATA[bob]]></user> <!-- password -->"
    );
}

#[test]
fn whitespace_keys_inline_tables_and_yaml_blocks() {
    assert_eq!(
        scrub(r#"rcon_password "hunter2""#),
        r#"rcon_password "<redacted>""#
    );
    assert_eq!(scrub("requirepass hunter2"), "requirepass <redacted>");
    assert_eq!(scrub("requirepass foo=bar"), "requirepass <redacted>");
    assert_eq!(scrub("maxclients 100"), "maxclients 100");
    assert_eq!(
        scrub(r#"db = { password = "hunter2", port = 5 }"#),
        r#"db = { password = "<redacted>", port = 5 }"#
    );
    assert_eq!(
        scrub("password: |\n  hunter2\n  more\n\nuser: bob"),
        "password: |\n  <redacted>\n  <redacted>\n\nuser: bob"
    );
    assert_eq!(
        scrub("a:\n  token: >-\n    x\n  name: n"),
        "a:\n  token: >-\n    <redacted>\n  name: n"
    );
}

#[test]
fn both_key_rules_and_arrays() {
    assert_eq!(
        scrub(r#"session_data = {"id": "hunter2"}"#),
        "session_data = {<redacted>}"
    );
    assert_eq!(
        scrub(r#""tokens": ["hunter2","abc"], "n": 1"#),
        r#""tokens": [<redacted>], "n": 1"#
    );
    assert_eq!(
        scrub("{\n  \"tokens\": [\n    \"hunter2\",\n    \"abc\"\n  ],\n  \"n\": 1\n}"),
        "{\n  \"tokens\": [\n    <redacted>\n    <redacted>\n  ],\n  \"n\": 1\n}"
    );
    assert_eq!(
        scrub("tokens = [\n  \"a\",\n]\nport = 1"),
        "tokens = [\n  <redacted>\n]\nport = 1"
    );
}

#[test]
fn heads_pass_the_raw_view_of_final_check() {
    let env = env();
    for text in [
        "a=1\n\tabcdefghijklmnopqrstuvw\n",
        "x\n@example.org is ours",
        "x\n:\\foo",
        "x\r:/foo",
        "a\u{1}bcdefghijklmnopqrstuvwxyz",
    ] {
        let head = scrub_secrets(text, &env);
        assert!(passes(&head, &env), "{text:?} -> {head:?}");
    }
    assert_eq!(
        scrub("a=1\n\tabcdefghijklmnopqrstuvw\n"),
        "a=1\n\t<redacted>\n"
    );
    assert_eq!(
        scrub("\tabcdefghijklmnopqrstuv"),
        "\tabcdefghijklmnopqrstuv"
    );
    assert_eq!(scrub("x\n@example.org"), "x\n<redacted>");
    assert_eq!(scrub("x\n:\\foo"), "x\n<redacted>\\foo");
    assert_eq!(scrub("a\u{1}b"), "a b");
}

#[test]
fn json_escapes_are_decoded_before_scrubbing() {
    let mut env = env();
    env.user_name = "Макс".to_owned();
    let text = r#"{"dir": "C:\\Users\\\u041c\u0430\u043a\u0441\\AppData"}"#;
    let head = scrub_secrets(text, &env);
    assert_eq!(head, r#"{"dir": "{DRIVE:C}\\Users\\<redacted>\\AppData"}"#);
    assert!(passes(&head, &env));
    // Escaped backslash before `u`, quotes and control characters stay escaped.
    assert_eq!(
        scrub(r#"\\u0041 \u0041 \u0022 \u000a \uD83D\uDE00 \uD83D"#),
        r#"\\u0041 A \u0022 \u000a 😀 \uD83D"#
    );
    assert_eq!(scrub(r"name=\u004dax"), "name=<redacted>");
}
