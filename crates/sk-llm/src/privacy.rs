//! Defense-in-depth check of every element before it is sent (SPEC-08 §4.8).

use std::collections::HashMap;

use serde_json::Value;
use sk_core::env::Environment;

/// Minimum length of a run of `[A-Za-z0-9_-]` that looks like a token
/// (same rule as `sk_core::privacy::redact`, SPEC-02 §4.2).
const TOKEN_MIN_LEN: usize = 24;

/// Why an element was not sent. Carries no matched text, so it can go to
/// issues and logs as is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PrivacyViolation {
    /// `env.user_name` as a whole word, ignoring case.
    #[error("contains the user name")]
    UserName,
    /// `env.machine_name` as a whole word, ignoring case.
    #[error("contains the machine name")]
    MachineName,
    /// An absolute path: `X:\` or `X:/` (not `{DRIVE:X}`).
    #[error("contains an absolute path")]
    AbsolutePath,
    /// An email address.
    #[error("contains an email address")]
    Email,
    /// A run of at least 24 `[A-Za-z0-9_-]` characters, outside the
    /// exceptions of [`final_check`].
    #[error("contains a token-like string")]
    Token,
}

/// Checks the serialized JSON of one element before sending (§4.8).
///
/// Looks for, in this order: the user name and the machine name (whole
/// words, ignoring case, names shorter than two characters are ignored), an
/// absolute path (`[A-Za-z]:\` anywhere; `[A-Za-z]:/` when no letter or
/// digit is right before the drive letter, so `https://` passes;
/// `{DRIVE:X}` never matches), an email and a token-like string. The rules
/// for emails and tokens are those of `sk_core::privacy::redact`, so a
/// properly redacted summary always passes. `<redacted>` itself passes.
///
/// Token exceptions (real Windows folder names, T-08-15): in a string under
/// a `path` key, a run of the form `<name>_<13 chars [a-z0-9]>` (UWP package
/// family name) or `S-1-<n>(-<n>)+` (SID) is not a token; in the `ext` of an
/// `ext_histogram` entry the token rule does not apply. The exceptions come
/// from the parsed JSON, so they need valid JSON; in the text views a run is
/// exempt only as many times as it occurs in those fields, so the same run
/// in `sample_names` or under a duplicate key is still a token.
///
/// Three views of the input are checked, the first violation wins: the raw
/// text (catches duplicate keys that a JSON parser drops), the raw text with
/// JSON escapes decoded leniently (catches `Max` even in invalid JSON)
/// and, if the text is valid JSON, every decoded string, keys included.
/// Then the lenient decoding is repeated on its own result, up to
/// [`UNESCAPE_ROUNDS`] rounds in all or until nothing changes, so text
/// escaped twice (`\\u041c` inside a JSON string) is seen decoded too.
pub fn final_check(json: &str, env: &Environment) -> Result<(), PrivacyViolation> {
    let parsed = serde_json::from_str::<Value>(json).ok();
    let mut allowed = Allowed::new();
    if let Some(value) = &parsed {
        let _ = walk(value, Field::Other, &mut |s, field| {
            allow_exempt_runs(s, field, &mut allowed);
            Ok::<(), ()>(())
        });
    }
    check_view(json, env, true, &allowed)?;
    let mut view = unescape_lenient(json);
    check_view(&view, env, false, &allowed)?;
    if let Some(value) = &parsed {
        walk(value, Field::Other, &mut |s, field| {
            let mut own = Allowed::new();
            allow_exempt_runs(s, field, &mut own);
            check_view(s, env, false, &own)
        })?;
    }
    for _ in 1..UNESCAPE_ROUNDS {
        let next = unescape_lenient(&view);
        if next == view {
            break;
        }
        check_view(&next, env, false, &allowed)?;
        view = next;
    }
    Ok(())
}

/// Rounds of lenient JSON unescaping checked by [`final_check`].
const UNESCAPE_ROUNDS: usize = 4;

/// How many times each token-like run may occur in a view without being a
/// violation (the token exceptions of [`final_check`]).
type Allowed = HashMap<String, usize>;

/// Where a JSON value sits, for the token exceptions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Other,
    /// A value under a `path` key.
    Path,
    /// The array under an `ext_histogram` key.
    ExtHistogram,
    /// An object in that array.
    ExtEntry,
    /// The `ext` of such an object.
    Ext,
}

/// Calls `f` for every string in `value`, keys included (as [`Field::Other`]).
fn walk<E>(
    value: &Value,
    field: Field,
    f: &mut impl FnMut(&str, Field) -> Result<(), E>,
) -> Result<(), E> {
    match value {
        Value::String(s) => f(s.as_str(), field),
        Value::Array(items) => {
            let child = if field == Field::ExtHistogram {
                Field::ExtEntry
            } else {
                Field::Other
            };
            items.iter().try_for_each(|v| walk(v, child, f))
        }
        Value::Object(obj) => obj.iter().try_for_each(|(k, v)| {
            f(k.as_str(), Field::Other)?;
            let child = match (field, k.as_str()) {
                (_, "path") => Field::Path,
                (_, "ext_histogram") => Field::ExtHistogram,
                (Field::ExtEntry, "ext") => Field::Ext,
                _ => Field::Other,
            };
            walk(v, child, f)
        }),
        Value::Null | Value::Bool(_) | Value::Number(_) => Ok(()),
    }
}

/// Finds the token-like runs of a string in `field` that are not tokens.
/// Adds them to `allowed`.
fn allow_exempt_runs(text: &str, field: Field, allowed: &mut Allowed) {
    for run in token_runs(text) {
        let exempt = match field {
            Field::Path => is_package_family_name(run) || is_sid(run),
            Field::Ext => true,
            Field::Other | Field::ExtHistogram | Field::ExtEntry => false,
        };
        if exempt {
            *allowed.entry(run.to_owned()).or_default() += 1;
        }
    }
}

/// `<name>_<publisher id>`: a non-empty name, `_` and 13 chars `[a-z0-9]`.
fn is_package_family_name(run: &str) -> bool {
    const PUBLISHER_ID_LEN: usize = 13;
    let b = run.as_bytes();
    if b.len() <= PUBLISHER_ID_LEN + 1 {
        return false;
    }
    let (name, id) = b.split_at(b.len() - PUBLISHER_ID_LEN);
    let id_char = |c: &u8| c.is_ascii_lowercase() || c.is_ascii_digit();
    name.ends_with(b"_") && id.iter().all(id_char)
}

/// `S-1-<n>(-<n>)+`.
fn is_sid(run: &str) -> bool {
    let Some(rest) = run.strip_prefix("S-1-") else {
        return false;
    };
    let number = |p: &&str| !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit());
    rest.contains('-') && rest.split('-').all(|p| number(&p))
}

/// `raw`: `text` is the serialized JSON itself, see [`has_absolute_path`].
fn check_view(
    text: &str,
    env: &Environment,
    raw: bool,
    allowed: &Allowed,
) -> Result<(), PrivacyViolation> {
    if contains_word(text, &env.user_name) {
        return Err(PrivacyViolation::UserName);
    }
    if contains_word(text, &env.machine_name) {
        return Err(PrivacyViolation::MachineName);
    }
    if has_absolute_path(text, raw) {
        return Err(PrivacyViolation::AbsolutePath);
    }
    if has_email(text) {
        return Err(PrivacyViolation::Email);
    }
    if has_token(text, allowed) {
        return Err(PrivacyViolation::Token);
    }
    Ok(())
}

/// `word` as a whole word (neighbours are not letters or digits), ignoring case.
fn contains_word(text: &str, word: &str) -> bool {
    let word: Vec<char> = word.chars().collect();
    if word.len() < 2 {
        return false;
    }
    let chars: Vec<char> = text.chars().collect();
    let eq = |a: char, b: char| a == b || a.to_lowercase().eq(b.to_lowercase());
    (0..chars.len()).any(|i| {
        let end = i + word.len();
        end <= chars.len()
            && (i == 0 || !chars[i - 1].is_alphanumeric())
            && (end == chars.len() || !chars[end].is_alphanumeric())
            && chars[i..end].iter().zip(&word).all(|(a, b)| eq(*a, *b))
    })
}

/// `X:\` anywhere; `X:/` only when no letter or digit is right before the
/// drive letter, so URL schemes (`https://`) pass.
///
/// In the `raw` view (the serialized JSON itself) `X:\` does not count when
/// the `\` starts a JSON escape other than `\\`: `\n \r \t \b \f \" \/`, or
/// a valid `\uXXXX` that is not `\u005C`. `services:` before a line break is
/// `s:\n` there; the decoded views still catch real paths.
fn has_absolute_path(text: &str, raw: bool) -> bool {
    let b = text.as_bytes();
    let escape_not_backslash = |at: usize| match b.get(at) {
        Some(b'n' | b'r' | b't' | b'b' | b'f' | b'"' | b'/') => true,
        Some(b'u') => b
            .get(at + 1..at + 5)
            .and_then(|h| std::str::from_utf8(h).ok())
            .filter(|h| h.bytes().all(|c| c.is_ascii_hexdigit()))
            .and_then(|h| u32::from_str_radix(h, 16).ok())
            .is_some_and(|code| code != 0x5C),
        _ => false,
    };
    (0..b.len().saturating_sub(2)).any(|i| {
        b[i].is_ascii_alphabetic()
            && b[i + 1] == b':'
            && match b[i + 2] {
                b'\\' => !(raw && escape_not_backslash(i + 3)),
                b'/' => i == 0 || !b[i - 1].is_ascii_alphanumeric(),
                _ => false,
            }
    })
}

/// Decodes JSON string escapes (`\n`, `\\`, `\uXXXX` with surrogate pairs)
/// anywhere in `text`, without requiring valid JSON. Unknown escapes are kept
/// as is; a lone surrogate becomes U+FFFD.
fn unescape_lenient(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let hex4 = |at: usize| -> Option<u32> {
        let s: String = chars.get(at..at + 4)?.iter().collect();
        u32::from_str_radix(&s, 16).ok()
    };
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '\\' || i + 1 >= chars.len() {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let simple = match chars[i + 1] {
            'n' => Some('\n'),
            't' => Some('\t'),
            'r' => Some('\r'),
            'b' => Some('\u{8}'),
            'f' => Some('\u{c}'),
            '"' => Some('"'),
            '\\' => Some('\\'),
            '/' => Some('/'),
            _ => None,
        };
        if let Some(c) = simple {
            out.push(c);
            i += 2;
            continue;
        }
        let Some(hi) = (chars[i + 1] == 'u').then(|| hex4(i + 2)).flatten() else {
            out.push(chars[i]);
            i += 1;
            continue;
        };
        i += 6;
        let code = if (0xD800..0xDC00).contains(&hi) {
            let lo = (chars.get(i) == Some(&'\\') && chars.get(i + 1) == Some(&'u'))
                .then(|| hex4(i + 2))
                .flatten()
                .filter(|lo| (0xDC00..0xE000).contains(lo));
            match lo {
                Some(lo) => {
                    i += 6;
                    0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                }
                None => 0xFFFD,
            }
        } else {
            hi
        };
        out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
    }
    out
}

fn is_local(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-')
}

fn is_domain(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '-')
}

/// `example.org`: labels are not empty, the last one has at least two letters.
fn is_email_domain(domain: &[char]) -> bool {
    let domain: String = domain.iter().collect();
    let labels: Vec<&str> = domain.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|l| !l.is_empty())
        && labels
            .last()
            .is_some_and(|tld| tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic()))
}

fn has_email(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    (0..chars.len()).filter(|&i| chars[i] == '@').any(|at| {
        let has_local = at > 0 && is_local(chars[at - 1]);
        let mut end = at + 1;
        while end < chars.len() && is_domain(chars[end]) {
            end += 1;
        }
        // A sentence may end right after the address.
        while end > at + 1 && matches!(chars[end - 1], '.' | '-') {
            end -= 1;
        }
        has_local && is_email_domain(&chars[at + 1..end])
    })
}

/// Maximal runs of `[A-Za-z0-9_-]` at least [`TOKEN_MIN_LEN`] long.
fn token_runs(text: &str) -> Vec<&str> {
    let not_token_char = |c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-'));
    let runs = text.split(not_token_char);
    runs.filter(|run| run.len() >= TOKEN_MIN_LEN).collect()
}

/// A token-like run that occurs more often than `allowed` permits.
fn has_token(text: &str, allowed: &Allowed) -> bool {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for run in token_runs(text) {
        *counts.entry(run).or_default() += 1;
    }
    let over = |(run, n): (&&str, &usize)| *n > allowed.get(*run).copied().unwrap_or(0);
    counts.iter().any(over)
}

#[cfg(test)]
#[path = "privacy_tests.rs"]
mod tests;
