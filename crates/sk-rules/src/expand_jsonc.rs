//! Reading the values of a `from_json` target (SPEC-04 §4.2.1 steps 3–5):
//! a minimal JSONC preprocessor, a JSON tree that keeps document order, and
//! the `select` pointer with `*` segments.

use std::fmt;

use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};

use crate::schema::JsonFormat;

/// A JSON document with object members in document order.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Json {
    String(String),
    Array(Vec<Json>),
    /// Members in document order, repeated keys included.
    Object(Vec<(String, Json)>),
    /// `null`, booleans and numbers: never selected as paths.
    Other,
}

/// The file could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ParseError {
    /// Human-readable description.
    pub(super) message: String,
    /// 1-based line, when known.
    pub(super) line: Option<usize>,
}

/// String values selected by `select` from `bytes`, in document order.
/// Values of other types are left out.
pub(super) fn select_strings(
    bytes: &[u8],
    format: JsonFormat,
    select: &str,
) -> Result<Vec<String>, ParseError> {
    let root = parse(bytes, format)?;
    Ok(select_values(&root, select)
        .into_iter()
        .filter_map(|value| match value {
            Json::String(s) => Some(s.clone()),
            _ => None,
        })
        .collect())
}

/// Parses `bytes` (UTF-8, an optional BOM is skipped).
pub(super) fn parse(bytes: &[u8], format: JsonFormat) -> Result<Json, ParseError> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let result = match format {
        JsonFormat::Json => serde_json::from_slice::<Json>(bytes),
        JsonFormat::Jsonc => serde_json::from_slice::<Json>(&strip_jsonc(bytes)),
    };
    result.map_err(|err| ParseError {
        message: err.to_string(),
        line: (err.line() > 0).then_some(err.line()),
    })
}

/// JSONC → JSON: `//` and `/* */` comments and trailing commas (before `}`
/// or `]`) become spaces. Line breaks are kept, so parse errors point to the
/// original lines; text inside strings is never changed.
pub(super) fn strip_jsonc(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    let mut in_string = false;
    while i < input.len() {
        let b = input[i];
        if in_string {
            out.push(b);
            i += 1;
            if b == b'\\' {
                if let Some(&escaped) = input.get(i) {
                    out.push(escaped);
                    i += 1;
                }
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match (b, input.get(i + 1)) {
            (b'"', _) => {
                in_string = true;
                out.push(b);
                i += 1;
            }
            (b'/', Some(b'/')) => {
                while i < input.len() && input[i] != b'\n' {
                    out.push(b' ');
                    i += 1;
                }
            }
            (b'/', Some(b'*')) => {
                out.extend_from_slice(b"  ");
                i += 2;
                while i < input.len() {
                    if input[i] == b'*' && input.get(i + 1) == Some(&b'/') {
                        out.extend_from_slice(b"  ");
                        i += 2;
                        break;
                    }
                    out.push(if input[i] == b'\n' { b'\n' } else { b' ' });
                    i += 1;
                }
            }
            _ => {
                out.push(b);
                i += 1;
            }
        }
    }
    drop_trailing_commas(&mut out);
    out
}

/// Replaces a comma followed (after whitespace) by `}` or `]` with a space.
/// Runs after comments are blanked out.
fn drop_trailing_commas(text: &mut [u8]) {
    let mut in_string = false;
    let mut escaped = false;
    for i in 0..text.len() {
        let b = text[i];
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b',' => {
                let next = text[i + 1..].iter().find(|c| !c.is_ascii_whitespace());
                if matches!(next, Some(b'}' | b']')) {
                    text[i] = b' ';
                }
            }
            _ => {}
        }
    }
}

/// Values at `pointer`: a JSON Pointer (RFC 6901, `~0`/`~1` escapes) where a
/// `*` segment stands for every member of an object or element of an array.
/// The empty pointer selects the root. Missing members give nothing.
pub(super) fn select_values<'a>(root: &'a Json, pointer: &str) -> Vec<&'a Json> {
    let mut current = vec![root];
    if pointer.is_empty() {
        return current;
    }
    for segment in pointer.split('/').skip(1) {
        let wildcard = segment == "*";
        let key = unescape(segment);
        let mut next = Vec::new();
        for value in current {
            match value {
                Json::Object(members) if wildcard => next.extend(members.iter().map(|(_, v)| v)),
                Json::Array(items) if wildcard => next.extend(items),
                // A repeated key: the last one wins, as in `serde_json`.
                Json::Object(members) => {
                    next.extend(
                        members
                            .iter()
                            .rev()
                            .find(|(k, _)| *k == key)
                            .map(|(_, v)| v),
                    );
                }
                Json::Array(items) => {
                    next.extend(array_index(segment).and_then(|index| items.get(index)));
                }
                Json::String(_) | Json::Other => {}
            }
        }
        current = next;
    }
    current
}

/// `~1` → `/`, then `~0` → `~` (RFC 6901 §4).
fn unescape(segment: &str) -> String {
    segment.replace("~1", "/").replace("~0", "~")
}

/// An array index: decimal digits without leading zeros (RFC 6901 §4).
fn array_index(segment: &str) -> Option<usize> {
    let digits = !segment.is_empty() && segment.bytes().all(|b| b.is_ascii_digit());
    if !digits || (segment.len() > 1 && segment.starts_with('0')) {
        return None;
    }
    segment.parse().ok()
}

impl<'de> Deserialize<'de> for Json {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(JsonVisitor)
    }
}

struct JsonVisitor;

impl<'de> Visitor<'de> for JsonVisitor {
    type Value = Json;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_bool<E>(self, _: bool) -> Result<Json, E> {
        Ok(Json::Other)
    }

    fn visit_i64<E>(self, _: i64) -> Result<Json, E> {
        Ok(Json::Other)
    }

    fn visit_u64<E>(self, _: u64) -> Result<Json, E> {
        Ok(Json::Other)
    }

    fn visit_f64<E>(self, _: f64) -> Result<Json, E> {
        Ok(Json::Other)
    }

    fn visit_unit<E>(self) -> Result<Json, E> {
        Ok(Json::Other)
    }

    fn visit_str<E>(self, value: &str) -> Result<Json, E> {
        Ok(Json::String(value.to_owned()))
    }

    fn visit_string<E>(self, value: String) -> Result<Json, E> {
        Ok(Json::String(value))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Json, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(Json::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Json, A::Error> {
        let mut members = Vec::new();
        while let Some((key, value)) = map.next_entry::<String, Json>()? {
            members.push((key, value));
        }
        Ok(Json::Object(members))
    }
}

#[cfg(test)]
#[path = "expand_jsonc_tests.rs"]
mod tests;
