//! Removing personal data from strings that leave the scan: LLM input, logs,
//! issues, the backup manifest (SPEC-02 §4.2).

use std::path::Path;

use crate::env::Environment;
use crate::template::PathTemplate;

/// Replacement for removed data.
pub const REDACTED: &str = "<redacted>";

/// Minimum length of a run of `[A-Za-z0-9_-]` that looks like a token.
const TOKEN_MIN_LEN: usize = 24;

/// Replaces emails, token-like strings and the user and machine names from
/// `env` with [`REDACTED`], in that order.
pub fn redact(text: &str, env: &Environment) -> String {
    let mut out = redact_tokens(&redact_emails(text));
    for name in [&env.user_name, &env.machine_name] {
        out = redact_word(&out, name);
    }
    out
}

/// A path for logs and issues: the template from [`PathTemplate::from_path`],
/// then [`redact`].
pub fn redact_path(path: &Path, env: &Environment) -> String {
    redact(PathTemplate::from_path(path, env).as_str(), env)
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

fn redact_emails(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut pos = 0;
    while pos < chars.len() {
        if chars[pos] == '@' {
            let mut start = pos;
            while start > copied && is_local(chars[start - 1]) {
                start -= 1;
            }
            let mut end = pos + 1;
            while end < chars.len() && is_domain(chars[end]) {
                end += 1;
            }
            // A sentence may end right after the address.
            while end > pos + 1 && matches!(chars[end - 1], '.' | '-') {
                end -= 1;
            }
            if start < pos && is_email_domain(&chars[pos + 1..end]) {
                out.extend(&chars[copied..start]);
                out.push_str(REDACTED);
                copied = end;
                pos = end;
                continue;
            }
        }
        pos += 1;
    }
    out.extend(&chars[copied..]);
    out
}

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-')
}

fn redact_tokens(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if run.chars().count() >= TOKEN_MIN_LEN {
            out.push_str(REDACTED);
        } else {
            out.push_str(run);
        }
        run.clear();
    };
    for c in text.chars() {
        if is_token_char(c) {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

/// Replaces `word` as a whole word, ignoring case; words shorter than two
/// characters are left alone.
fn redact_word(text: &str, word: &str) -> String {
    let word: Vec<char> = word.chars().collect();
    if word.len() < 2 {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let eq = |a: char, b: char| a == b || a.to_lowercase().eq(b.to_lowercase());
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let end = i + word.len();
        let matches = end <= chars.len()
            && (i == 0 || !chars[i - 1].is_alphanumeric())
            && (end == chars.len() || !chars[end].is_alphanumeric())
            && chars[i..end].iter().zip(&word).all(|(a, b)| eq(*a, *b));
        if matches {
            out.push_str(REDACTED);
            i = end;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::env::KnownFolder;

    fn env() -> Environment {
        let mut env = Environment::fake(&PathBuf::from(if cfg!(windows) {
            r"C:\fake"
        } else {
            "/fake"
        }));
        env.user_name = "Max".to_owned();
        env.machine_name = "DESKTOP-01".to_owned();
        env
    }

    fn r(text: &str) -> String {
        redact(text, &env())
    }

    #[test]
    fn emails() {
        assert_eq!(r("mail max.l+x@lea-soft.org now"), "mail <redacted> now");
        assert_eq!(r("write to a_b@example.com."), "write to <redacted>.");
        assert_eq!(r("one@a.io, two@b.io"), "<redacted>, <redacted>");
        for kept in [
            "a@b",
            "user@localhost",
            "x@y.c",
            "@example.com",
            "v1@2.0",
            "x@y.123",
        ] {
            assert_eq!(r(kept), kept);
        }
    }

    #[test]
    fn tokens() {
        let t24 = "abcdefghijklmnopqrstuvwx";
        assert_eq!(r(t24), REDACTED);
        assert_eq!(r(&t24[1..]), &t24[1..]);
        assert_eq!(r("ghp_1234567890abcdefghijABCDEFGHIJ123456"), REDACTED);
        assert_eq!(
            r("key=sk-ant-api03-AbCdEfGhIjKlMnOpQrStUv"),
            "key=<redacted>"
        );
        assert_eq!(r("{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}"), "{<redacted>}");
        assert_eq!(r("save_2024_final.sav"), "save_2024_final.sav");
    }

    #[test]
    fn user_and_machine_names_as_words() {
        assert_eq!(r(r"C:\Users\max\x"), r"C:\Users\<redacted>\x");
        assert_eq!(r("MAX-notes.txt"), "<redacted>-notes.txt");
        assert_eq!(r("max_save.dat"), "<redacted>_save.dat");
        assert_eq!(r("maximum and Maxwell"), "maximum and Maxwell");
        assert_eq!(r("backup of desktop-01"), "backup of <redacted>");
        assert_eq!(r("DESKTOP-012"), "DESKTOP-012");
    }

    #[test]
    fn unicode_names_ignore_case() {
        let mut env = env();
        env.user_name = "Макс".to_owned();
        assert_eq!(
            redact("МАКС и макс_old, Максим", &env),
            "<redacted> и <redacted>_old, Максим"
        );
    }

    #[test]
    fn short_names_are_not_replaced() {
        let mut env = env();
        env.user_name = "a".to_owned();
        assert_eq!(redact("a b a", &env), "a b a");
    }

    #[test]
    fn email_with_user_name_is_one_replacement() {
        assert_eq!(r("max@example.com"), REDACTED);
    }

    #[test]
    fn redact_path_uses_template() {
        let env = env();
        let app_data = env
            .known_folder(KnownFolder::AppData)
            .unwrap()
            .to_path_buf();
        assert_eq!(redact_path(&app_data.join("Code"), &env), r"{APPDATA}\Code");
        assert_eq!(
            redact_path(&app_data.join("Max-profile"), &env),
            r"{APPDATA}\<redacted>-profile"
        );
    }

    #[cfg(windows)]
    #[test]
    fn redact_path_outside_known_folders() {
        assert_eq!(
            redact_path(Path::new(r"D:\Users\max\Games"), &env()),
            r"{DRIVE:D}\Users\<redacted>\Games"
        );
    }
}
