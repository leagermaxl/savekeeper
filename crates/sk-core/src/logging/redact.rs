//! Anonymization of log lines (SPEC-01 §4.8.3).

use std::io::{self, Write};
use std::sync::Arc;

use tracing_subscriber::fmt::MakeWriter;

use crate::env::Environment;
use crate::privacy::redact_names;

/// Replaces known folder paths with tokens and the user and machine names
/// with `<redacted>`.
#[derive(Debug, Clone)]
pub(crate) struct LineRedactor {
    /// `(path, token)`, longest path first.
    paths: Vec<(Vec<char>, String)>,
    user_name: String,
    machine_name: String,
}

impl LineRedactor {
    pub(crate) fn new(env: &Environment) -> Self {
        let mut paths: Vec<(Vec<char>, String)> = env
            .known_folders
            .iter()
            .map(|(folder, path)| {
                let path = path.to_string_lossy();
                let path = path.trim_end_matches(['\\', '/']);
                (
                    path.chars().collect::<Vec<char>>(),
                    format!("{{{}}}", folder.token()),
                )
            })
            .filter(|(path, _)| !path.is_empty())
            .collect();
        paths.sort_by_key(|(path, _)| std::cmp::Reverse(path.len()));
        Self {
            paths,
            user_name: env.user_name.clone(),
            machine_name: env.machine_name.clone(),
        }
    }

    pub(crate) fn apply(&self, line: &str) -> String {
        let mut out = line.to_owned();
        for (path, token) in &self.paths {
            out = replace_path(&out, path, token);
        }
        redact_names(&out, &self.user_name, &self.machine_name)
    }
}

fn eq_ci(a: char, b: char) -> bool {
    a == b || a.to_lowercase().eq(b.to_lowercase())
}

/// A path ends where the next character cannot continue a file name.
fn ends_path(next: Option<&char>) -> bool {
    match next {
        None => true,
        Some(&c) => !(c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '~')),
    }
}

/// Replaces `path` (optionally preceded by `\\?\`) ignoring case, only where it
/// ends at a component boundary: `C:\Users\max` does not match in `C:\Users\maxim`.
fn replace_path(text: &str, path: &[char], token: &str) -> String {
    const VERBATIM: [char; 4] = ['\\', '\\', '?', '\\'];
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let start = if chars[i..].starts_with(&VERBATIM) {
            i + 4
        } else {
            i
        };
        let end = start + path.len();
        let matches = end <= chars.len()
            && chars[start..end]
                .iter()
                .zip(path)
                .all(|(a, b)| eq_ci(*a, *b))
            && ends_path(chars.get(end));
        if matches {
            out.push_str(token);
            i = end;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// A writer that anonymizes everything written through it.
pub(crate) struct RedactingWriter<W> {
    inner: W,
    redactor: Arc<LineRedactor>,
}

impl<W: Write> Write for RedactingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // The fmt layer writes a whole formatted event at once.
        let line = self.redactor.apply(&String::from_utf8_lossy(buf));
        self.inner.write_all(line.as_bytes())?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// [`MakeWriter`] for [`RedactingWriter`].
pub(crate) struct RedactingMakeWriter<M> {
    pub(crate) inner: M,
    pub(crate) redactor: Arc<LineRedactor>,
}

impl<'a, M: MakeWriter<'a>> MakeWriter<'a> for RedactingMakeWriter<M> {
    type Writer = RedactingWriter<M::Writer>;

    fn make_writer(&'a self) -> Self::Writer {
        RedactingWriter {
            inner: self.inner.make_writer(),
            redactor: Arc::clone(&self.redactor),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::env::KnownFolder;

    fn env() -> (Environment, PathBuf) {
        let root = PathBuf::from(if cfg!(windows) { r"C:\fake" } else { "/fake" });
        let mut env = Environment::fake(&root);
        env.user_name = "max".to_owned();
        env.machine_name = "DESKTOP-01".to_owned();
        let home = root.join("Users").join("max");
        env.known_folders.insert(KnownFolder::Home, home.clone());
        env.known_folders
            .insert(KnownFolder::AppData, home.join("AppData").join("Roaming"));
        env.known_folders
            .insert(KnownFolder::LocalLow, home.join("AppData").join("LocalLow"));
        (env, home)
    }

    fn s(path: &Path) -> String {
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn known_folders_become_tokens() {
        let (env, home) = env();
        let r = LineRedactor::new(&env);
        let app_data = home.join("AppData").join("Roaming").join("Code");
        assert_eq!(
            r.apply(&format!("open {}", s(&app_data))),
            format!("open {{APPDATA}}{}Code", std::path::MAIN_SEPARATOR)
        );
        let low = home.join("AppData").join("LocalLow");
        assert_eq!(r.apply(&format!("[{}]", s(&low))), "[{LOCALLOW}]");
        assert_eq!(
            r.apply(&s(&home.join(".ssh"))),
            format!("{{HOME}}{}.ssh", std::path::MAIN_SEPARATOR)
        );
    }

    #[test]
    fn matching_ignores_case_and_respects_boundaries() {
        let (env, home) = env();
        let r = LineRedactor::new(&env);
        let upper = s(&home).to_uppercase();
        assert_eq!(r.apply(&upper), "{HOME}");
        // `...\maxim` is not under `...\max`, and `max` is not a separate word in it.
        let other = format!("{}im", s(&home));
        assert_eq!(r.apply(&other), other);
    }

    #[test]
    fn names_are_redacted_outside_paths() {
        let (env, _) = env();
        let r = LineRedactor::new(&env);
        assert_eq!(
            r.apply("user max on desktop-01, scan 1a2b3c4d-5e6f-7a8b-9c0d-112233445566"),
            "user <redacted> on <redacted>, scan 1a2b3c4d-5e6f-7a8b-9c0d-112233445566"
        );
    }

    #[cfg(windows)]
    #[test]
    fn verbatim_prefix_is_replaced_too() {
        let (env, _) = env();
        let r = LineRedactor::new(&env);
        assert_eq!(
            r.apply(r"read \\?\C:\fake\Users\max\AppData\Roaming\x.json"),
            r"read {APPDATA}\x.json"
        );
        assert_eq!(r.apply(r"D:\Users\max\Games"), r"D:\Users\<redacted>\Games");
    }
}
