//! Pattern steps of the content peek scrub: paths, URLs with credentials,
//! long runs and IP addresses (SPEC-08 §4.9).

use std::net::Ipv6Addr;

use sk_core::env::Environment;
use sk_core::privacy::REDACTED;

/// Minimum length of a run of `[A-Za-z0-9+/=_-]` that is replaced.
pub const LONG_RUN_MIN_LEN: usize = 24;

fn eq_ci(a: char, b: char) -> bool {
    a == b || a.to_lowercase().eq(b.to_lowercase())
}

fn is_sep(c: char) -> bool {
    c == '\\' || c == '/'
}

/// A path ends where the next character cannot continue a file name.
fn ends_path(next: Option<&char>) -> bool {
    match next {
        None => true,
        Some(&c) => !(c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '~')),
    }
}

/// Length of the match of `path` at `text[at..]`, ignoring case; a
/// separator of `path` matches `/` or one or more `\` (JSON-escaped paths).
fn match_path(text: &[char], at: usize, path: &[char]) -> Option<usize> {
    let mut i = at;
    for &p in path {
        if is_sep(p) {
            match text.get(i) {
                Some('/') => i += 1,
                Some('\\') => {
                    while text.get(i) == Some(&'\\') {
                        i += 1;
                    }
                }
                _ => return None,
            }
        } else {
            if !text.get(i).is_some_and(|&c| eq_ci(c, p)) {
                return None;
            }
            i += 1;
        }
    }
    ends_path(text.get(i)).then_some(i - at)
}

/// Known folders of `env` → `{TOKEN}` (longest path first, ignoring case, at
/// a component boundary), then the remaining drive roots → `{DRIVE:X}`:
/// `X:\` anywhere, `X:/` only without an ASCII letter or digit before the
/// drive letter (as [`final_check`](crate::final_check) does), so
/// `https://` is kept.
pub fn replace_paths(text: &str, env: &Environment) -> String {
    let mut folders: Vec<(Vec<char>, String)> = env
        .known_folders
        .iter()
        .map(|(folder, path)| {
            let path = path.to_string_lossy();
            let path: Vec<char> = path.trim_end_matches(['\\', '/']).chars().collect();
            (path, format!("{{{}}}", folder.token()))
        })
        .filter(|(path, _)| !path.is_empty())
        .collect();
    folders.sort_by_key(|(path, _)| std::cmp::Reverse(path.len()));

    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    'outer: while i < chars.len() {
        for (path, token) in &folders {
            if let Some(len) = match_path(&chars, i, path) {
                out.push_str(token);
                i += len;
                continue 'outer;
            }
        }
        let c = chars[i];
        let drive = c.is_ascii_alphabetic()
            && chars.get(i + 1) == Some(&':')
            && match chars.get(i + 2) {
                Some('\\') => true,
                Some('/') => i == 0 || !chars[i - 1].is_ascii_alphanumeric(),
                _ => false,
            };
        if drive {
            out.push_str("{DRIVE:");
            out.push(c.to_ascii_uppercase());
            out.push('}');
            i += 2;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Characters that end a URL in free text.
fn ends_url(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '`')
}

/// `scheme://user:pass@host/…` → `<redacted>` (the whole URL).
pub fn redact_credential_urls(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut i = 0;
    while i + 3 <= chars.len() {
        if chars[i..i + 3] != [':', '/', '/'] {
            i += 1;
            continue;
        }
        let mut start = i;
        while start > copied
            && (chars[start - 1].is_ascii_alphanumeric()
                || matches!(chars[start - 1], '+' | '.' | '-'))
        {
            start -= 1;
        }
        while start < i && !chars[start].is_ascii_alphabetic() {
            start += 1;
        }
        let auth_start = i + 3;
        let mut auth_end = auth_start;
        while auth_end < chars.len()
            && !ends_url(chars[auth_end])
            && !matches!(chars[auth_end], '/' | '?' | '#')
        {
            auth_end += 1;
        }
        let authority = &chars[auth_start..auth_end];
        let has_userinfo = authority
            .iter()
            .rposition(|&c| c == '@')
            .is_some_and(|at| authority[..at].contains(&':'));
        if start == i || !has_userinfo {
            i += 3;
            continue;
        }
        let mut end = auth_end;
        while end < chars.len() && !ends_url(chars[end]) {
            end += 1;
        }
        out.extend(&chars[copied..start]);
        out.push_str(REDACTED);
        copied = end;
        i = end;
    }
    out.extend(&chars[copied..]);
    out
}

fn is_run_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '_' | '-')
}

/// A character that JSON serializes as a backslash and a letter (`\n`,
/// `\r`, `\t`). Other control characters are replaced by
/// [`normalize`] first.
fn is_letter_escape(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\t')
}

/// Runs of at least [`LONG_RUN_MIN_LEN`] `[A-Za-z0-9+/=_-]` → `<redacted>`.
/// A run right after `\n`, `\r` or `\t` counts one more character: in the
/// serialized JSON the escape letter joins the run (`\tabc…`), and the
/// raw view of [`final_check`](crate::final_check) sees it.
pub fn redact_long_runs(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if !is_run_char(chars[i]) {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && is_run_char(chars[i]) {
            i += 1;
        }
        let bonus = usize::from(start > 0 && is_letter_escape(chars[start - 1]));
        if i - start + bonus >= LONG_RUN_MIN_LEN {
            out.push_str(REDACTED);
        } else {
            out.extend(&chars[start..i]);
        }
    }
    out
}

/// Before scrubbing: `\uXXXX` escapes written in the file (JSON) are
/// decoded (surrogate pairs too; an escape preceded by an even number of
/// backslashes is decoded, `\\u0041` is kept), unless the result is a
/// control character, `"` or `\` or a lone surrogate; then control
/// characters other than `\n`, `\r`, `\t` become a space. Names and paths
/// written as escapes are then found by the later steps.
pub fn normalize(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let hex4 = |at: usize| -> Option<u32> {
        let s: String = chars.get(at..at + 4)?.iter().collect();
        if s.chars().all(|c| c.is_ascii_hexdigit()) {
            u32::from_str_radix(&s, 16).ok()
        } else {
            None
        }
    };
    let mut out = String::with_capacity(text.len());
    let mut slashes = 0usize;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && slashes.is_multiple_of(2) && chars.get(i + 1) == Some(&'u') {
            if let Some((ch, len)) = decode_u_escape(&hex4, &chars, i) {
                out.push(ch);
                i += len;
                slashes = 0;
                continue;
            }
        }
        slashes = if c == '\\' { slashes + 1 } else { 0 };
        if c.is_control() && !is_letter_escape(c) {
            out.push(' ');
        } else {
            out.push(c);
        }
        i += 1;
    }
    out
}

/// The character of the `\uXXXX` (or surrogate pair) escape at `at` and the
/// number of characters it takes.
fn decode_u_escape(
    hex4: &impl Fn(usize) -> Option<u32>,
    chars: &[char],
    at: usize,
) -> Option<(char, usize)> {
    let hi = hex4(at + 2)?;
    let (code, len) = if (0xD800..0xDC00).contains(&hi) {
        let lo = (chars.get(at + 6) == Some(&'\\') && chars.get(at + 7) == Some(&'u'))
            .then(|| hex4(at + 8))
            .flatten()
            .filter(|lo| (0xDC00..0xE000).contains(lo))?;
        (0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00), 12)
    } else {
        (hi, 6)
    };
    let ch = char::from_u32(code)?;
    (!ch.is_control() && ch != '"' && ch != '\\').then_some((ch, len))
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

/// What the escape letter of `\n`, `\r`, `\t` would complete in the
/// serialized JSON: `\n@example.org` (an email to the raw view of
/// [`final_check`](crate::final_check)) → `\n<redacted>`, and `\n:\` /
/// `\n:/` (an absolute path `n:\`) → `\n<redacted>\`.
pub fn redact_after_escapes(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let after_escape = i > 0 && is_letter_escape(chars[i - 1]);
        if after_escape && c == ':' && chars.get(i + 1).is_some_and(|&n| is_sep(n)) {
            out.push_str(REDACTED);
            i += 1;
            continue;
        }
        if after_escape && c == '@' {
            let mut end = i + 1;
            while end < chars.len()
                && (chars[end].is_ascii_alphanumeric() || matches!(chars[end], '.' | '-'))
            {
                end += 1;
            }
            while end > i + 1 && matches!(chars[end - 1], '.' | '-') {
                end -= 1;
            }
            if is_email_domain(&chars[i + 1..end]) {
                out.push_str(REDACTED);
                i = end;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

fn is_ipv4(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 4
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.len() <= 3
                && p.bytes().all(|b| b.is_ascii_digit())
                && p.parse::<u16>().is_ok_and(|n| n <= 255)
        })
}

/// IPv4 addresses `a.b.c.d` (each part 0..=255) not glued to letters or
/// digits (`v1.2.3.4` is kept) → `<redacted>`. One trailing dot (end of a
/// sentence) stays in the text.
pub fn redact_ipv4(text: &str) -> String {
    redact_addresses(text, |c| c.is_ascii_digit() || c == '.', is_ipv4)
}

fn is_ipv6(s: &str) -> bool {
    s.matches(':').count() >= 2
        && s.chars().any(|c| c.is_ascii_hexdigit())
        && s.parse::<Ipv6Addr>().is_ok()
}

/// IPv6 addresses (at least two `:` and a hex digit, valid for
/// [`Ipv6Addr`], not glued to letters or digits) → `<redacted>`.
/// `12:30:45` and `std::cout` are kept.
pub fn redact_ipv6(text: &str) -> String {
    redact_addresses(
        text,
        |c| c.is_ascii_hexdigit() || c == ':' || c == '.',
        is_ipv6,
    )
}

/// Maximal runs of `in_run` characters whose neighbours are not letters or
/// digits; a run that is `valid` (or is valid without one trailing `.` or
/// `:`) is replaced.
fn redact_addresses(
    text: &str,
    in_run: impl Fn(char) -> bool,
    valid: impl Fn(&str) -> bool,
) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if !in_run(chars[i]) {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && in_run(chars[i]) {
            i += 1;
        }
        let run: String = chars[start..i].iter().collect();
        let glued = (start > 0 && chars[start - 1].is_alphanumeric())
            || chars.get(i).is_some_and(|c| c.is_alphanumeric());
        let trimmed = run.strip_suffix(['.', ':']).unwrap_or(&run);
        if glued {
            out.push_str(&run);
        } else if valid(&run) {
            out.push_str(REDACTED);
        } else if trimmed.len() < run.len() && valid(trimmed) {
            out.push_str(REDACTED);
            out.push_str(&run[trimmed.len()..]);
        } else {
            out.push_str(&run);
        }
    }
    out
}
