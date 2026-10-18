//! Secret scrubbing of content peek heads, key-value lines (SPEC-08 §4.9).

use sk_core::env::Environment;
use sk_core::privacy::{redact, REDACTED};

use super::{patterns, scrub_json, scrub_xml};

/// Substrings of a key that mark its value as secret, matched ignoring
/// ASCII case: `(?i)(pass(word)?|pwd|secret|token|api[_-]?key|auth|cookie|session|private|bearer|credential)`.
const SECRET_KEYWORDS: [&str; 13] = [
    "pass",
    "pwd",
    "secret",
    "token",
    "apikey",
    "api_key",
    "api-key",
    "auth",
    "cookie",
    "session",
    "private",
    "bearer",
    "credential",
];

/// Whether a key (INI/YAML key, JSON key, XML element or attribute name)
/// names a secret: it contains one of the keywords of §4.9, ignoring case.
pub fn is_secret_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    SECRET_KEYWORDS.iter().any(|w| key.contains(w))
}

/// Scrubs secrets from the head of a text file (§4.9), in this order:
///
/// 0. `\uXXXX` escapes are decoded and control characters other than
///    `\n`, `\r`, `\t` become spaces ([`patterns::normalize`]);
/// 1. values of secret keys (see [`is_secret_key`]) → `<redacted>`, line by
///    line, with state carried to the next lines: XML (secret elements,
///    attributes, `key`/`name` attributes, tags over several lines), JSON /
///    TOML quoted pairs (strings, scalars, arrays, objects, `{"key":
///    "password", "value": …}`), the first `=`/`:` of the line (INI, YAML,
///    TOML; an empty value or a block scalar takes the more indented next
///    lines; `"""`/`'''` strings up to the closing delimiter), a secret
///    first word followed by whitespace (`.cfg`); then every `secret_key =
///    value` / `secret_key: value` inside the line (connection strings, URL
///    queries, `--password=…`);
/// 2. absolute paths: known folders → their token (`{APPDATA}`), other
///    drive roots → `{DRIVE:X}`, so [`final_check`](crate::final_check) passes;
/// 3. URLs with `user:pass@` → `<redacted>` (the whole URL);
/// 4. runs of at least 24 `[A-Za-z0-9+/=_-]` → `<redacted>`;
/// 5. what a `\n`/`\r`/`\t` escape letter would complete: `@domain`, `:\`;
/// 6. IPv4 and IPv6 addresses → `<redacted>`;
/// 7. [`sk_core::privacy::redact`] (emails, tokens, user and machine names).
///
/// Line endings (`\n`, `\r\n`) are kept.
pub fn scrub_secrets(text: &str, env: &Environment) -> String {
    let text = patterns::normalize(text);
    let text = scrub_lines(&text);
    let text = patterns::replace_paths(&text, env);
    let text = patterns::redact_credential_urls(&text);
    let text = patterns::redact_long_runs(&text);
    let text = patterns::redact_after_escapes(&text);
    let text = patterns::redact_ipv4(&text);
    let text = patterns::redact_ipv6(&text);
    redact(&text, env)
}

/// What the previous lines left open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Pending {
    /// Nothing.
    None,
    /// A secret XML element whose text has not been seen yet.
    Xml,
    /// An XML open tag going on in the next line: its name or an earlier
    /// element is secret / a `key`/`name` attribute has a secret value.
    Tag { secret: bool, names_secret: bool },
    /// A secret array not closed yet (`"tokens": [`).
    Array,
    /// A secret object not closed yet, with this many open braces.
    Object(usize),
    /// A secret key at this indentation with an empty value or a YAML block
    /// scalar: more indented lines (and `- ` items at the same indentation)
    /// are its value.
    Block(usize),
    /// A TOML multi-line string of a secret key, up to this delimiter.
    MultiString(&'static str),
}

/// State carried from line to line.
#[derive(Debug, Clone, Copy)]
pub(super) struct State {
    /// What is open.
    pub(super) pending: Pending,
    /// A JSON object had `"key"`/`"name"` with a secret value and its
    /// `"value"` was not seen yet.
    pub(super) kv_secret: bool,
}

/// Step 1 of [`scrub_secrets`] for every line.
fn scrub_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut state = State {
        pending: Pending::None,
        kv_secret: false,
    };
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let (body, cr) = match line.strip_suffix('\r') {
            Some(body) => (body, "\r"),
            None => (line, ""),
        };
        let structured = scrub_line(body, &mut state);
        out.push_str(&rebuild(&structured, inline_ranges(&structured)));
        out.push_str(cr);
    }
    out
}

pub(super) fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// `line[..end]` redacted, then the rest of the line scrubbed as a new line.
fn close_and_go_on(line: &str, end: usize, state: &mut State) -> String {
    state.pending = Pending::None;
    let mut out = redact_whole(&line[..end]);
    out.push_str(&scrub_line(&line[end..], state));
    out
}

fn scrub_line(line: &str, state: &mut State) -> String {
    match state.pending {
        Pending::Block(indent) => {
            let trimmed = line.trim_start();
            if trimmed.is_empty() {
                return line.to_owned();
            }
            let item = indent_of(line) == indent && trimmed.starts_with('-');
            if indent_of(line) > indent || item {
                return redact_whole(line);
            }
            state.pending = Pending::None;
        }
        Pending::Array => {
            return match line.find(']') {
                Some(close) => close_and_go_on(line, close, state),
                None => redact_whole(line),
            };
        }
        Pending::Object(depth) => {
            return match scrub_json::close_brace(line, 0, depth) {
                Ok(close) => close_and_go_on(line, close, state),
                Err(depth) => {
                    state.pending = Pending::Object(depth);
                    redact_whole(line)
                }
            };
        }
        Pending::MultiString(delim) => {
            return match line.find(delim) {
                Some(close) => close_and_go_on(line, close, state),
                None => redact_whole(line),
            };
        }
        // A line without `=` or `>` does not go on with a tag.
        Pending::Tag { .. } if !line.contains(['=', '>']) => state.pending = Pending::None,
        Pending::Xml | Pending::Tag { .. } => {
            return rebuild(line, scrub_xml::ranges(line, &mut state.pending));
        }
        Pending::None => {}
    }
    if line.trim_start().starts_with('<') {
        return rebuild(line, scrub_xml::ranges(line, &mut state.pending));
    }
    let mut ranges = scrub_json::quoted_key_ranges(line, state);
    ranges.extend(plain_key_range(line, &mut state.pending));
    ranges.extend(word_key_range(line));
    rebuild(line, ranges)
}

/// Leading whitespace kept, the rest replaced; a blank text is kept.
fn redact_whole(text: &str) -> String {
    if text.trim().is_empty() {
        return text.to_owned();
    }
    let mut out = text[..indent_of(text)].to_owned();
    out.push_str(REDACTED);
    out
}

/// Replaces the byte ranges of `line` with `<redacted>`; overlapping ranges
/// are merged.
pub(super) fn rebuild(line: &str, mut ranges: Vec<(usize, usize)>) -> String {
    ranges.retain(|(s, e)| s < e);
    ranges.sort_unstable();
    let mut out = String::with_capacity(line.len());
    let mut copied = 0;
    let mut i = 0;
    while i < ranges.len() {
        let (start, mut end) = ranges[i];
        while i + 1 < ranges.len() && ranges[i + 1].0 <= end {
            end = end.max(ranges[i + 1].1);
            i += 1;
        }
        let start = start.max(copied);
        if start < end {
            out.push_str(&line[copied..start]);
            out.push_str(REDACTED);
            copied = end;
        }
        i += 1;
    }
    out.push_str(&line[copied..]);
    out
}

/// The range of a value `core` starting at byte `start`: inside the quotes
/// if quoted; `None` for an empty value or `<redacted>`.
pub(super) fn value_range(core: &str, start: usize) -> Option<(usize, usize)> {
    let quoted = core.len() >= 2
        && ((core.starts_with('"') && core.ends_with('"'))
            || (core.starts_with('\'') && core.ends_with('\'')));
    let (s, e) = if quoted {
        (start + 1, start + core.len() - 1)
    } else {
        (start, start + core.len())
    };
    let inner = &core[s - start..e - start];
    (!inner.trim().is_empty() && inner != REDACTED).then_some((s, e))
}

/// A YAML block scalar indicator: `|`, `>`, `|-`, `>+2`.
fn is_block_indicator(core: &str) -> bool {
    core.starts_with(['|', '>'])
        && core[1..]
            .chars()
            .all(|c| matches!(c, '+' | '-') || c.is_ascii_digit())
}

/// The value of a secret key starting at byte `start` (`core` is the
/// trimmed value without a trailing comma): open arrays, objects, TOML
/// multi-line strings and YAML blocks set `pending` for the next lines.
pub(super) fn secret_value(
    line: &str,
    core: &str,
    start: usize,
    pending: &mut Pending,
) -> Option<(usize, usize)> {
    if core.is_empty() || is_block_indicator(core) {
        *pending = Pending::Block(indent_of(line));
        return None;
    }
    for delim in ["\"\"\"", "'''"] {
        if let Some(rest) = core.strip_prefix(delim) {
            if let Some(close) = rest.find(delim) {
                let s = start + delim.len();
                return (close > 0).then_some((s, s + close));
            }
            *pending = Pending::MultiString(delim);
            return (!rest.trim().is_empty()).then_some((start + delim.len(), line.len()));
        }
    }
    if core.starts_with('[') && !core.contains(']') {
        *pending = Pending::Array;
        return value_range(&core[1..], start + 1);
    }
    if core.starts_with('{') {
        let end = match scrub_json::close_brace(line, start + 1, 1) {
            Ok(close) => close,
            Err(depth) => {
                *pending = Pending::Object(depth);
                line.len()
            }
        };
        return (!line[start + 1..end].trim().is_empty()).then_some((start + 1, end));
    }
    value_range(core, start)
}

/// `key = value` / `key: value`: the first `=` or `:` of the line. Keys
/// starting with `"` or with `"`, `{`, `[` or `,` inside are left to the
/// JSON rule.
fn plain_key_range(line: &str, pending: &mut Pending) -> Option<(usize, usize)> {
    let sep = line.find(['=', ':'])?;
    let key = line[..sep].trim().trim_start_matches(['-', ' ', '\t']);
    if key.starts_with('"') {
        return None;
    }
    let key = key.trim_matches('\'');
    if key.contains(['"', '{', '[', ',']) || !is_secret_key(key) {
        return None;
    }
    let value = &line[sep + 1..];
    let core = value.trim().trim_end_matches(',').trim_end();
    secret_value(line, core, sep + 1 + indent_of(value), pending)
}

fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')
}

/// `.cfg`/`.conf`: a secret first word followed by whitespace and a value
/// (`rcon_password "x"`, `requirepass x`) → the rest of the line.
fn word_key_range(line: &str) -> Option<(usize, usize)> {
    let start = indent_of(line);
    let word_len = line[start..].find(|c: char| !is_ident(c))?;
    let word_end = start + word_len;
    if word_len == 0
        || !line[word_end..].starts_with(char::is_whitespace)
        || !is_secret_key(&line[start..word_end])
    {
        return None;
    }
    let rest = line[word_end..].trim();
    if rest.starts_with(['=', ':']) {
        return None;
    }
    value_range(rest, word_end + indent_of(&line[word_end..]))
}

pub(super) fn skip_ws(line: &str, mut i: usize) -> usize {
    let b = line.as_bytes();
    while i < b.len() && b[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

/// Characters that end an inline value.
fn ends_inline(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            ';' | '&' | ',' | '"' | '\'' | '<' | '>' | '(' | ')' | '[' | ']' | '{' | '}'
        )
}

/// Every `secret_key = value` / `secret_key: value` inside a line, after
/// the structured rules: the value up to `;`, `&`, `,`, whitespace, a quote
/// or a bracket (`Password=x;`, `?access_token=x&`, `--password=x`,
/// `{ password = "x" }`); a quoted value → its content. After `Bearer` or
/// `Basic` the next word is taken too. `key::x` and `key==x` are not pairs.
fn inline_ranges(line: &str) -> Vec<(usize, usize)> {
    let b = line.as_bytes();
    let mut ranges = Vec::new();
    let mut i = 0;
    while i < line.len() {
        // Only ASCII bytes start an identifier, so `i` is a char boundary here.
        if !is_ident(b[i] as char) || (i > 0 && is_ident(b[i - 1] as char)) {
            i += 1;
            continue;
        }
        let end = i + line[i..]
            .find(|c: char| !is_ident(c))
            .unwrap_or(line.len() - i);
        let key = &line[i..end];
        i = end;
        let sep = skip_ws(line, end);
        let Some(&s) = b.get(sep).filter(|s| matches!(s, b'=' | b':')) else {
            continue;
        };
        if b.get(sep + 1) == Some(&s) || !is_secret_key(key) {
            continue;
        }
        let v = skip_ws(line, sep + 1);
        if let Some(&q) = b.get(v).filter(|q| matches!(q, b'"' | b'\'')) {
            if let Some(close) = line[v + 1..].find(q as char) {
                ranges.extend(value_range(&line[v..v + close + 2], v));
                i = v + close + 2;
            }
            continue;
        }
        let mut v_end = v + line[v..].find(ends_inline).unwrap_or(line.len() - v);
        let word = &line[v..v_end];
        if word.eq_ignore_ascii_case("bearer") || word.eq_ignore_ascii_case("basic") {
            let next = skip_ws(line, v_end);
            v_end = next + line[next..].find(ends_inline).unwrap_or(line.len() - next);
        }
        if v_end > v && !is_block_indicator(&line[v..v_end]) {
            ranges.push((v, v_end));
        }
        i = v_end.max(i);
    }
    ranges
}
