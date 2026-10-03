//! Reading Valve KeyValues text (`.vdf`, `.acf`) with `keyvalues-parser`
//! (SPEC-05 §4.4): file access through [`FsScanner::read_small`] and
//! case-insensitive lookups.

use std::path::Path;

use keyvalues_parser::{Obj, Value};
use sk_core::fs::{FsError, FsScanner};

/// Deepest nesting of `{` accepted before parsing: the parser is recursive,
/// and Steam files nest only a few levels.
const MAX_DEPTH: usize = 64;

/// Why a file could not be used; the `reason` argument of issues.
pub(crate) type Reason = &'static str;

/// The file as text (lossy UTF-8, without a BOM), at most `max` bytes.
pub(crate) fn read_text(fs: &dyn FsScanner, path: &Path, max: usize) -> Result<String, Reason> {
    let bytes = fs.read_small(path, max).map_err(|e| fs_reason(&e))?;
    let text = String::from_utf8_lossy(&bytes);
    Ok(text.strip_prefix('\u{feff}').unwrap_or(&text).to_owned())
}

/// `reason` of a file system error.
pub(crate) fn fs_reason(error: &FsError) -> Reason {
    match error {
        FsError::NotFound => "not_found",
        FsError::AccessDenied => "access_denied",
        FsError::SharingViolation => "locked",
        FsError::TooLarge => "too_large",
        FsError::CloudOnly => "cloud_only",
        FsError::Cancelled => "cancelled",
        FsError::Io(_) => "io",
    }
}

/// `reason` of a file that was read but is not valid.
pub(crate) const INVALID: Reason = "invalid";

/// The top-level object of a KeyValues document (its key is ignored);
/// `None` if the text does not parse, is nested too deeply or its value is
/// a string.
pub(crate) fn parse(text: &str) -> Option<Obj<'_>> {
    if depth(text) > MAX_DEPTH {
        return None;
    }
    match keyvalues_parser::parse(text).ok()?.value {
        Value::Obj(obj) => Some(obj),
        Value::Str(_) => None,
    }
}

/// The first value of `key`, compared without ASCII case.
pub(crate) fn get<'a, 't>(obj: &'a Obj<'t>, key: &str) -> Option<&'a Value<'t>> {
    obj.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .and_then(|(_, values)| values.first())
}

/// The first value of `key` if it is a string.
pub(crate) fn get_str<'a>(obj: &'a Obj<'_>, key: &str) -> Option<&'a str> {
    get(obj, key).and_then(Value::get_str)
}

/// Maximum nesting of `{` outside quoted strings (`\"` does not end a string).
fn depth(text: &str) -> usize {
    let (mut depth, mut max) = (0usize, 0usize);
    let mut quoted = false;
    let mut escaped = false;
    for b in text.bytes() {
        if quoted {
            match b {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => quoted = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => quoted = true,
            b'{' => {
                depth += 1;
                max = max.max(depth);
            }
            b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    max
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_objects_and_ignores_key_case() {
        let text =
            "\"AppState\"\n{\n\t\"AppID\"\t\t\"10\"\n\t\"Sub\"\n\t{\n\t\t\"k\" \"v\"\n\t}\n}\n";
        let obj = parse(text).unwrap_or_else(|| panic!("no object"));
        assert_eq!(get_str(&obj, "appid"), Some("10"));
        assert_eq!(get_str(&obj, "sub"), None);
        assert!(get(&obj, "SUB").is_some_and(Value::is_obj));
        assert_eq!(get(&obj, "missing"), None);
    }

    #[test]
    fn unescapes_backslashes() {
        let obj = parse(r#""a" { "path" "D:\\SteamLibrary" }"#).unwrap_or_else(|| panic!("none"));
        assert_eq!(get_str(&obj, "path"), Some(r"D:\SteamLibrary"));
    }

    #[test]
    fn rejects_broken_and_deep_text() {
        assert!(parse("\"a\" { \"b\" ").is_none());
        assert!(parse("").is_none());
        assert!(parse("\"a\" \"b\"").is_none());
        let deep = format!("\"a\" {}{}", "{ \"b\" ".repeat(100), "}".repeat(100));
        assert!(parse(&deep).is_none());
        // Braces inside strings do not count.
        let quoted = format!("\"a\" {{ \"b\" \"{}\" }}", "{".repeat(100));
        assert!(parse(&quoted).is_some());
    }

    #[test]
    fn depth_skips_escaped_quotes() {
        assert_eq!(depth(r#""a\"{" { "b" { } }"#), 2);
    }
}
