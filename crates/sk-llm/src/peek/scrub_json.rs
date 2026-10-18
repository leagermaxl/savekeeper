//! Secret scrubbing of quoted-key lines (JSON, TOML quoted keys) in content
//! peek heads (SPEC-08 §4.9).

use super::scrub::{indent_of, is_secret_key, secret_value, skip_ws, Pending, State};

/// End (exclusive, after the closing quote) of the JSON string starting at
/// the quote `start`; `None` if it is not closed on this line.
fn string_end(line: &str, start: usize) -> Option<usize> {
    let b = line.as_bytes();
    let mut i = start + 1;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'"' => return Some(i + 1),
            _ => i += 1,
        }
    }
    None
}

/// Index of the `}` that closes `depth` open braces, scanning from `from`
/// and skipping JSON strings; `Err(depth)` with the braces still open at
/// the end of the line.
pub(super) fn close_brace(line: &str, from: usize, mut depth: usize) -> Result<usize, usize> {
    let b = line.as_bytes();
    let mut i = from;
    while i < b.len() {
        match b[i] {
            b'"' => match string_end(line, i) {
                Some(end) => {
                    i = end;
                    continue;
                }
                None => return Err(depth),
            },
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    Err(depth)
}

/// A quoted key: its content and the start of its value (after `:` or `=`).
struct Pair<'a> {
    key: &'a str,
    value: usize,
}

/// The next `"key": ` / `"key" = ` at or after `from`.
fn next_pair(line: &str, mut from: usize) -> Option<Pair<'_>> {
    let b = line.as_bytes();
    while let Some(off) = line[from..].find('"') {
        let start = from + off;
        let end = string_end(line, start)?;
        let sep = skip_ws(line, end);
        if matches!(b.get(sep), Some(b':' | b'=')) && b.get(sep + 1) != Some(&b'=') {
            return Some(Pair {
                key: &line[start + 1..end - 1],
                value: skip_ws(line, sep + 1),
            });
        }
        from = end;
    }
    None
}

/// `"key"`/`"name"` with a secret string value on this line.
fn names_secret(line: &str) -> bool {
    let mut from = 0;
    while let Some(pair) = next_pair(line, from) {
        from = pair.value;
        let is_name = pair.key.eq_ignore_ascii_case("key") || pair.key.eq_ignore_ascii_case("name");
        if is_name && line.as_bytes().get(pair.value) == Some(&b'"') {
            if let Some(end) = string_end(line, pair.value) {
                if is_secret_key(&line[pair.value + 1..end - 1]) {
                    return true;
                }
                from = end;
            }
        }
    }
    false
}

/// Every `"key": value` (or TOML `"key" = value`) pair of the line, also
/// minified JSON. For a secret key: a string → its content, a scalar → up
/// to `,}]`, an array or object closed on the line → its content, an open
/// array or object → the rest of the line and the next lines up to the
/// closing bracket, `"""`/`'''` → up to the closing delimiter, nothing →
/// the more indented next lines. `"value"` is secret too next to a
/// `"key"`/`"name"` with a secret value, also on the following lines of the
/// same object.
pub(super) fn quoted_key_ranges(line: &str, state: &mut State) -> Vec<(usize, usize)> {
    let b = line.as_bytes();
    let kv = names_secret(line);
    let value_secret = kv || state.kv_secret;
    let mut saw_value = false;
    let mut ranges = Vec::new();
    let mut from = 0;
    while let Some(Pair { key, value: v }) = next_pair(line, from) {
        from = v;
        let is_value = key.eq_ignore_ascii_case("value");
        saw_value |= is_value;
        if !(is_secret_key(key) || (is_value && value_secret)) {
            continue;
        }
        match b.get(v) {
            Some(b'"') if !line[v..].starts_with("\"\"\"") => {
                let Some(end) = string_end(line, v) else {
                    ranges.push((v + 1, line.len()));
                    break;
                };
                if end - 1 > v + 1 {
                    ranges.push((v + 1, end - 1));
                }
                from = end;
            }
            Some(b'[') => match line[v..].find(']') {
                Some(close) => {
                    ranges.push((v + 1, v + close));
                    from = v + close + 1;
                }
                None => {
                    ranges.push((v + 1, line.len()));
                    state.pending = Pending::Array;
                    break;
                }
            },
            Some(b'{') => match close_brace(line, v + 1, 1) {
                Ok(close) => {
                    ranges.push((v + 1, close));
                    from = close + 1;
                }
                Err(depth) => {
                    ranges.push((v + 1, line.len()));
                    state.pending = Pending::Object(depth);
                    break;
                }
            },
            Some(b'"' | b'\'')
                if line[v..].starts_with(r#"""""#) || line[v..].starts_with("'''") =>
            {
                let core = line[v..].trim_end();
                ranges.extend(secret_value(line, core, v, &mut state.pending));
                break;
            }
            Some(b'\'') => {
                let Some(close) = line[v + 1..].find('\'').map(|c| v + 1 + c) else {
                    ranges.push((v + 1, line.len()));
                    break;
                };
                ranges.push((v + 1, close));
                from = close + 1;
            }
            None => {
                state.pending = Pending::Block(indent_of(line));
                break;
            }
            Some(_) => {
                let len = line[v..].find([',', '}', ']']).unwrap_or(line.len() - v);
                ranges.push((v, v + line[v..v + len].trim_end().len()));
                from = v + len;
            }
        }
    }
    state.kv_secret = if kv {
        !saw_value
    } else {
        state.kv_secret && !saw_value && !line.contains('}')
    };
    ranges
}
