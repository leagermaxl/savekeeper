//! Tests of the JSONC preprocessor and of `select` (SPEC-04 §4.2.1 steps 3–4).

use super::*;

fn strings(text: &str, format: JsonFormat, select: &str) -> Vec<String> {
    select_strings(text.as_bytes(), format, select).unwrap_or_else(|e| panic!("{e:?}"))
}

#[test]
fn jsonc_comments_and_trailing_commas_are_removed() {
    let text = r#"{
  // vaults of the user
  "vaults": {
    "a": { "path": "one", /* inline */ },
    /* several
       lines */
    "b": { "path": "two" },
  },
  "list": [1, 2, ],
}"#;
    assert_eq!(
        strings(text, JsonFormat::Jsonc, "/vaults/*/path"),
        ["one", "two"]
    );
    // Lines are kept, so errors point to the original line.
    let stripped = strip_jsonc(text.as_bytes());
    assert_eq!(stripped.len(), text.len());
    assert_eq!(
        stripped.iter().filter(|&&b| b == b'\n').count(),
        text.matches('\n').count()
    );
    // Strict JSON rejects the same text.
    let err = select_strings(text.as_bytes(), JsonFormat::Json, "/vaults")
        .err()
        .unwrap_or_else(|| panic!("comments are not JSON"));
    assert_eq!(err.line, Some(2));
}

#[test]
fn jsonc_keeps_strings_intact() {
    let text = r#"{ "a": "http://x/*y*/", "b": "q\"// not a comment", "c": "x,]" , }"#;
    assert_eq!(
        strings(text, JsonFormat::Jsonc, "/*"),
        ["http://x/*y*/", "q\"// not a comment", "x,]"]
    );
}

#[test]
fn jsonc_error_line_follows_comments() {
    let text = "{\n/* one\ntwo */\n\"a\": [1 2]\n}";
    let err = select_strings(text.as_bytes(), JsonFormat::Jsonc, "/a")
        .err()
        .unwrap_or_else(|| panic!("invalid JSON"));
    assert_eq!(err.line, Some(4));
}

#[test]
fn bom_is_skipped() {
    let mut bytes = b"\xEF\xBB\xBF".to_vec();
    bytes.extend_from_slice(br#"{"a": "x"}"#);
    let values = select_strings(&bytes, JsonFormat::Json, "/a");
    assert_eq!(values, Ok(vec!["x".to_owned()]));
}

#[test]
fn wildcard_selects_object_members_and_array_items_in_document_order() {
    let text = r#"{
  "data": {
    "zeta": { "path": "z" },
    "alpha": { "path": "a" },
    "mid": { "path": 3, "other": "x" }
  },
  "list": [ { "path": "first" }, "skip", { "path": null }, { "path": "last" } ],
  "nested": [ [ "a1", "a2" ], [ "b1" ], { "k": "c1" } ]
}"#;
    assert_eq!(strings(text, JsonFormat::Json, "/data/*/path"), ["z", "a"]);
    assert_eq!(
        strings(text, JsonFormat::Json, "/list/*/path"),
        ["first", "last"]
    );
    assert_eq!(
        strings(text, JsonFormat::Json, "/nested/*/*"),
        ["a1", "a2", "b1", "c1"]
    );
    assert_eq!(strings(text, JsonFormat::Json, "/list/1"), ["skip"]);
    assert_eq!(strings(text, JsonFormat::Json, "/list/3/path"), ["last"]);
    // Missing members and non-index segments on arrays select nothing.
    assert!(strings(text, JsonFormat::Json, "/list/01").is_empty());
    assert!(strings(text, JsonFormat::Json, "/list/x").is_empty());
    assert!(strings(text, JsonFormat::Json, "/missing/*").is_empty());
    assert!(strings(text, JsonFormat::Json, "/data/zeta/path/more").is_empty());
}

#[test]
fn pointer_escapes_and_root() {
    let text = r#"{ "a/b": { "c~d": "slash-tilde" }, "": "empty-key", "~1": "literal" }"#;
    assert_eq!(
        strings(text, JsonFormat::Json, "/a~1b/c~0d"),
        ["slash-tilde"]
    );
    assert_eq!(strings(text, JsonFormat::Json, "/"), ["empty-key"]);
    // `~01` is `~1`, not `/`.
    assert_eq!(strings(text, JsonFormat::Json, "/~01"), ["literal"]);
    assert_eq!(strings("\"root\"", JsonFormat::Json, ""), ["root"]);
}

#[test]
fn repeated_key_uses_the_last_value() {
    let text = r#"{ "a": "first", "a": "second" }"#;
    assert_eq!(strings(text, JsonFormat::Json, "/a"), ["second"]);
    // A wildcard sees both members.
    assert_eq!(strings(text, JsonFormat::Json, "/*"), ["first", "second"]);
}

#[test]
fn invalid_json_reports_line() {
    let err = select_strings(b"{\n\"a\": \n}", JsonFormat::Json, "/a")
        .err()
        .unwrap_or_else(|| panic!("invalid JSON"));
    assert_eq!(err.line, Some(3));
    assert!(!err.message.is_empty());
}
