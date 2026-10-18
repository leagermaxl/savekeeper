//! Second review round of the content peek scrub (SPEC-08 §4.9):
//! non-ASCII attribute names, the raw view of `final_check`, empty secret
//! values, tags and strings over several lines, objects, key/value pairs,
//! and a deterministic fuzz run for panics.

use std::path::Path;

use sk_core::env::Environment;

use super::*;
use crate::privacy::final_check;

fn env() -> Environment {
    let mut env = Environment::fake(Path::new(if cfg!(windows) { r"C:\fake" } else { "/fake" }));
    env.user_name = "Max".to_owned();
    env.machine_name = "DESKTOP-01".to_owned();
    env
}

fn scrub(text: &str) -> String {
    scrub_secrets(text, &env())
}

fn passes(head: &str) -> bool {
    let json = serde_json::json!({ "file": "a.yml", "head": head }).to_string();
    final_check(&json, &env()).is_ok()
}

#[test]
fn non_ascii_attribute_names() {
    assert_eq!(scrub(r#"<item название="x"/>"#), r#"<item название="x"/>"#);
    assert_eq!(
        scrub(r#"<item ä="x" password="p" größe='1'/>"#),
        r#"<item ä="x" password="<redacted>" größe='1'/>"#
    );
    assert_eq!(
        scrub(r#"<x пароль_token="p">t</x>"#),
        r#"<x пароль_token="<redacted>">t</x>"#
    );
    assert_eq!(
        scrub("<äpassword=\"p\"\n  ü=\"x\" token=\"t\">"),
        "<äpassword=\"<redacted>\"\n  ü=\"x\" token=\"<redacted>\">"
    );
}

#[test]
fn yaml_keys_before_line_breaks_pass_the_final_check() {
    for text in [
        "version: 2\nservices:\n  web:\n    image: nginx\n",
        "server:\r\n  port: 80",
        "Notes:\nbuy milk",
        "a:\tb",
        "a:\u{8}b",
        "quote: \"x:\"",
    ] {
        let head = scrub(text);
        assert_eq!(head, text.replace('\u{8}', " "));
        assert!(passes(&head), "{text:?}");
    }
    assert!(!passes(r"C:\Users\x"));
}

#[test]
fn empty_secret_values_take_the_indented_lines() {
    assert_eq!(
        scrub("password:\n  hunter2\nuser: bob"),
        "password:\n  <redacted>\nuser: bob"
    );
    assert_eq!(
        scrub("passwords:\n  - hunter2\n  - abc\nport: 1"),
        "passwords:\n  <redacted>\n  <redacted>\nport: 1"
    );
    assert_eq!(
        scrub("passwords:\n- hunter2\nport: 1"),
        "passwords:\n<redacted>\nport: 1"
    );
    assert!(passes(&scrub("password:\n  hunter2\n")));
}

#[test]
fn tags_over_several_lines() {
    assert_eq!(
        scrub("<add key=\"DbPassword\"\n     value=\"hunter2\" />\n<add key=\"x\" value=\"y\"/>"),
        "<add key=\"DbPassword\"\n     value=\"<redacted>\" />\n<add key=\"x\" value=\"y\"/>"
    );
    assert_eq!(
        scrub("<add\n  key=\"Password\"\n  value=\"hunter2\"/>"),
        "<add\n  key=\"Password\"\n  value=\"<redacted>\"/>"
    );
    assert_eq!(
        scrub("<setting\n  name=\"Password\">\n  <value>hunter2</value>\n</setting>"),
        "<setting\n  name=\"Password\">\n  <value><redacted></value>\n</setting>"
    );
    assert_eq!(
        scrub("<db host=\"x\"\n  password=\"p\" />"),
        "<db host=\"x\"\n  password=\"<redacted>\" />"
    );
    // Not a tag: the next lines are scrubbed as usual.
    assert_eq!(
        scrub("<- note\n\"token\": \"x\""),
        "<- note\n\"token\": \"<redacted>\""
    );
    assert_eq!(
        scrub("<note\nsecret: |\n  x"),
        "<note\nsecret: |\n  <redacted>"
    );
}

#[test]
fn toml_multi_line_strings() {
    assert_eq!(
        scrub("password = \"\"\"\nhunter2\n\"\"\"\nuser = \"bob\""),
        "password = \"\"\"\n<redacted>\n\"\"\"\nuser = \"bob\""
    );
    assert_eq!(
        scrub("password = '''\nhunter2\nmore'''\nuser = 'bob'"),
        "password = '''\n<redacted>\n<redacted>'''\nuser = 'bob'"
    );
    assert_eq!(
        scrub("password = \"\"\"abc\"\"\""),
        "password = \"\"\"<redacted>\"\"\""
    );
    assert_eq!(scrub("token = '''x"), "token = '''<redacted>");
}

#[test]
fn quoted_toml_keys_objects_and_key_value_pairs() {
    assert_eq!(
        scrub(r#""password" = "hunter2""#),
        r#""password" = "<redacted>""#
    );
    assert_eq!(scrub(r#""token" = 'x'"#), r#""token" = '<redacted>'"#);
    assert_eq!(
        scrub(r#""credentials": {"u": "bob", "p": "hunter2"}, "n": 1"#),
        r#""credentials": {<redacted>}, "n": 1"#
    );
    assert_eq!(
        scrub("\"credentials\": {\n  \"u\": \"bob\",\n  \"x\": {\"p\": 1}\n},\n\"n\": 1"),
        "\"credentials\": {\n  <redacted>\n  <redacted>\n},\n\"n\": 1"
    );
    assert_eq!(
        scrub(r#"{"key": "password", "value": "hunter2"}, {"key": "a", "value": "b"}"#),
        r#"{"key": "password", "value": "<redacted>"}, {"key": "a", "value": "<redacted>"}"#
    );
    assert_eq!(
        scrub("{\n \"name\": \"ApiToken\",\n \"value\": \"x\"\n},\n{\n \"name\": \"Theme\",\n \"value\": \"dark\"\n}"),
        "{\n \"name\": \"ApiToken\",\n \"value\": \"<redacted>\"\n},\n{\n \"name\": \"Theme\",\n \"value\": \"dark\"\n}"
    );
}

/// Deterministic pseudo-random generator (no `rand` dependency).
pub(super) struct Lcg(pub(super) u64);

impl Lcg {
    pub(super) fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    pub(super) fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const PIECES: &[&str] = &[
    "<",
    ">",
    "/",
    "=",
    ":",
    "\"",
    "'",
    "\"\"\"",
    "'''",
    "{",
    "}",
    "[",
    "]",
    ",",
    ";",
    "&",
    " ",
    "\n",
    "\r\n",
    "\t",
    "\\",
    "\\\\",
    "\\u041c",
    "\\uD83D",
    "\\uDE00",
    "\\u0022",
    "\\u005c",
    "password",
    "Token",
    "key",
    "name",
    "value",
    "<![CDATA[",
    "]]>",
    "<!--",
    "-->",
    "://",
    "@",
    "C:\\",
    "c:/",
    "|",
    ">-",
    "- ",
    "  ",
    "ä",
    "и",
    "название",
    "😀",
    "\u{0}",
    "\u{1}",
    "\u{2028}",
    "\u{FEFF}",
    "\u{301}",
    "Max",
    "DESKTOP-01",
    "10.0.0.1",
    "::1",
    "fe80::",
    "example.org",
    "abcdefghijklmnopqrstuvwxyz0123",
    "+/=",
    "Bearer",
    "x",
    "1",
];

/// A random text of pieces that exercise every rule.
pub(super) fn random_text(rng: &mut Lcg) -> String {
    let n = rng.below(40);
    let mut out = String::new();
    for _ in 0..n {
        if rng.below(4) == 0 {
            // Any scalar value, mostly from the BMP.
            let max = if rng.below(8) == 0 {
                0x11_0000
            } else {
                0x1_0000
            };
            out.extend(char::from_u32(rng.below(max) as u32));
        } else {
            out.push_str(PIECES[rng.below(PIECES.len())]);
        }
    }
    out
}

#[test]
fn fuzz_scrub_never_panics() {
    let env = env();
    let mut rng = Lcg(0x5eed);
    for _ in 0..5_000 {
        let text = random_text(&mut rng);
        let head = scrub_secrets(&text, &env);
        let json = serde_json::json!({ "head": head }).to_string();
        let _ = final_check(&json, &env);
        let _ = final_check(&text, &env);
    }
}
