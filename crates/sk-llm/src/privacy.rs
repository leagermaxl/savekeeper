//! Defense-in-depth check of every element before it is sent (SPEC-08 §4.8).

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
    /// A run of at least 24 `[A-Za-z0-9_-]` characters.
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
/// Three views of the input are checked, the first violation wins: the raw
/// text (catches duplicate keys that a JSON parser drops), the raw text with
/// JSON escapes decoded leniently (catches `Max` even in invalid JSON)
/// and, if the text is valid JSON, every decoded string, keys included.
pub fn final_check(json: &str, env: &Environment) -> Result<(), PrivacyViolation> {
    check_text(json, env)?;
    check_text(&unescape_lenient(json), env)?;
    match serde_json::from_str::<Value>(json) {
        Ok(value) => check_value(&value, env),
        Err(_) => Ok(()),
    }
}

fn check_value(value: &Value, env: &Environment) -> Result<(), PrivacyViolation> {
    match value {
        Value::String(s) => check_text(s, env),
        Value::Array(items) => items.iter().try_for_each(|v| check_value(v, env)),
        Value::Object(obj) => obj.iter().try_for_each(|(k, v)| {
            check_text(k, env)?;
            check_value(v, env)
        }),
        Value::Null | Value::Bool(_) | Value::Number(_) => Ok(()),
    }
}

fn check_text(text: &str, env: &Environment) -> Result<(), PrivacyViolation> {
    if contains_word(text, &env.user_name) {
        return Err(PrivacyViolation::UserName);
    }
    if contains_word(text, &env.machine_name) {
        return Err(PrivacyViolation::MachineName);
    }
    if has_absolute_path(text) {
        return Err(PrivacyViolation::AbsolutePath);
    }
    if has_email(text) {
        return Err(PrivacyViolation::Email);
    }
    if has_token(text) {
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
fn has_absolute_path(text: &str) -> bool {
    let b = text.as_bytes();
    (0..b.len().saturating_sub(2)).any(|i| {
        b[i].is_ascii_alphabetic()
            && b[i + 1] == b':'
            && match b[i + 2] {
                b'\\' => true,
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

fn has_token(text: &str) -> bool {
    let mut run = 0usize;
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') {
            run += 1;
            if run >= TOKEN_MIN_LEN {
                return true;
            }
        } else {
            run = 0;
        }
    }
    false
}

#[cfg(test)]
#[path = "privacy_tests.rs"]
mod tests;
