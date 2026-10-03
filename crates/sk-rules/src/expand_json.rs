//! `from_json` targets: roots read from a JSON/JSONC config of the program
//! (SPEC-04 §4.2.1).
//!
//! The config is read with `read_small` (at most [`FROM_JSON_MAX_BYTES`]),
//! `select` picks string values in document order, and each value is
//! normalized into an absolute path and filtered: relative, network and
//! system paths, missing paths and repeats are skipped with an Info issue.
//! Every remaining path becomes a root under the template
//! `PathTemplate::from_path` gives it, so its `FindingId` is the same on any
//! machine.

use std::collections::{BTreeMap, HashSet};
use std::path::{Component, Path, PathBuf, MAIN_SEPARATOR, MAIN_SEPARATOR_STR};

use sk_core::env::{DriveKind, KnownFolder};
use sk_core::fs::{EntryMeta, FsError};
use sk_core::model::IssueSeverity;
use sk_core::path::starts_with_ci;
use sk_core::template::PathTemplate;

use super::jsonc::{select_strings, ParseError};
use super::{root_target, Expanded, Found, RuleOutput, TargetExpander};
use crate::compile::CompiledTarget;
use crate::schema::FromJson;
use crate::set::issue;

/// Largest config file a `from_json` target reads (SPEC-04 §4.2.1 step 2).
pub(crate) const FROM_JSON_MAX_BYTES: usize = 1024 * 1024;

/// Evidence message of a `from_json` finding; args `file`, `select`, `name`.
pub(crate) const FROM_JSON_MESSAGE_KEY: &str = "evidence.rule_from_json";

/// The config is larger than [`FROM_JSON_MAX_BYTES`]. Warning; args
/// `rule_id`, `file`, `limit`.
pub(crate) const ISSUE_FROM_JSON_TOO_LARGE: &str = "issue.rules.from_json_too_large";

/// The config exists but cannot be read (locked, access denied, cloud-only,
/// a folder). Warning; args `rule_id`, `file`, `error`.
pub(crate) const ISSUE_FROM_JSON_UNREADABLE: &str = "issue.rules.from_json_unreadable";

/// The config is not valid JSON/JSONC. Warning; args `rule_id`, `file`,
/// `error`, `line` when known.
pub(crate) const ISSUE_FROM_JSON_PARSE: &str = "issue.rules.from_json_parse";

/// `select` found no string values. Info; args `rule_id`, `file`, `select`.
pub(crate) const ISSUE_FROM_JSON_EMPTY: &str = "issue.rules.from_json_empty";

/// `select` found more than `max_matches` values; the first ones are kept.
/// Warning; args `rule_id`, `file`, `matches`, `limit`.
pub(crate) const ISSUE_FROM_JSON_TRUNCATED: &str = "issue.rules.from_json_truncated";

/// A value was skipped by a filter of §4.2.1 step 6. Info; `path` and arg
/// `path` are the template of the value; args `rule_id`, `reason`
/// ([`Skip::as_str`]).
pub(crate) const ISSUE_FROM_JSON_SKIPPED: &str = "issue.rules.from_json_skipped";

/// Why a selected value gives no root (SPEC-04 §4.2.1 step 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Skip {
    NotAbsolute,
    Network,
    SystemFolder,
    Missing,
    Duplicate,
}

impl Skip {
    /// Value of the `reason` argument.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Skip::NotAbsolute => "not_absolute",
            Skip::Network => "network",
            Skip::SystemFolder => "system_folder",
            Skip::Missing => "missing",
            Skip::Duplicate => "duplicate",
        }
    }
}

/// The `from_json` target being expanded.
#[derive(Clone, Copy)]
struct Source<'r> {
    rule_id: &'r str,
    target: &'r CompiledTarget,
    spec: &'r FromJson,
}

impl TargetExpander<'_> {
    /// Roots of a `from_json` target. Problems with the config or its values
    /// are reported right away (whether or not the rule fires); the config
    /// file itself is claimed when it exists.
    pub(super) fn json_targets(
        &self,
        rule_id: &str,
        target: &CompiledTarget,
        spec: &FromJson,
        out: &mut RuleOutput,
    ) -> Expanded {
        let mut expanded = Expanded::default();
        let mut seen = HashSet::new();
        let source = Source {
            rule_id,
            target,
            spec,
        };
        for file in spec.file.resolve(self.env, self.resolve) {
            self.read_config(&source, &file, &mut seen, &mut expanded, out);
        }
        expanded
    }

    /// Roots selected from one resolved config file.
    fn read_config(
        &self,
        source: &Source<'_>,
        file: &Path,
        seen: &mut HashSet<String>,
        expanded: &mut Expanded,
        out: &mut RuleOutput,
    ) {
        let Source {
            rule_id,
            target,
            spec,
        } = *source;
        let file_template = spec.file.as_str();
        let file_issue = |severity, key, mut args: Vec<(&'static str, String)>| {
            args.insert(0, ("rule_id", rule_id.to_owned()));
            args.insert(1, ("file", file_template.to_owned()));
            let mut found = issue(severity, key, args);
            found.path = Some(file_template.to_owned());
            found
        };
        let bytes = match self.fs.read_small(file, FROM_JSON_MAX_BYTES) {
            Ok(bytes) => bytes,
            // A missing config gives nothing and is not a problem (step 1).
            Err(FsError::NotFound) => return,
            Err(FsError::TooLarge) => {
                expanded.claimed.push(file.to_path_buf());
                out.issues.push(file_issue(
                    IssueSeverity::Warning,
                    ISSUE_FROM_JSON_TOO_LARGE,
                    vec![("limit", FROM_JSON_MAX_BYTES.to_string())],
                ));
                return;
            }
            Err(err) => {
                expanded.claimed.push(file.to_path_buf());
                out.issues.push(file_issue(
                    IssueSeverity::Warning,
                    ISSUE_FROM_JSON_UNREADABLE,
                    vec![("error", err.to_string())],
                ));
                return;
            }
        };
        expanded.claimed.push(file.to_path_buf());

        let mut values = match select_strings(&bytes, spec.format, &spec.select) {
            Ok(values) => values,
            Err(ParseError { message, line }) => {
                let mut args = vec![("error", message)];
                args.extend(line.map(|line| ("line", line.to_string())));
                out.issues.push(file_issue(
                    IssueSeverity::Warning,
                    ISSUE_FROM_JSON_PARSE,
                    args,
                ));
                return;
            }
        };
        if values.is_empty() {
            out.issues.push(file_issue(
                IssueSeverity::Info,
                ISSUE_FROM_JSON_EMPTY,
                vec![("select", spec.select.clone())],
            ));
            return;
        }
        if values.len() > spec.max_matches {
            out.issues.push(file_issue(
                IssueSeverity::Warning,
                ISSUE_FROM_JSON_TRUNCATED,
                vec![
                    ("matches", values.len().to_string()),
                    ("limit", spec.max_matches.to_string()),
                ],
            ));
            values.truncate(spec.max_matches);
        }

        let base = file.parent().unwrap_or(file);
        for value in values {
            match self.check_value(&value, base, seen) {
                Ok((template, resolved, meta)) => {
                    let name = resolved
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| template.as_str().to_owned());
                    let message_args = BTreeMap::from([
                        ("file".to_owned(), file_template.to_owned()),
                        ("select".to_owned(), spec.select.clone()),
                        ("name".to_owned(), name),
                    ]);
                    expanded.targets.push(Found {
                        target: root_target(target, template, resolved, &meta),
                        from_json: Some(message_args),
                    });
                }
                Err((reason, shown)) => {
                    let mut skipped = issue(
                        IssueSeverity::Info,
                        ISSUE_FROM_JSON_SKIPPED,
                        [
                            ("rule_id", rule_id.to_owned()),
                            ("path", shown.clone()),
                            ("reason", reason.as_str().to_owned()),
                        ],
                    );
                    skipped.path = Some(shown);
                    out.issues.push(skipped);
                }
            }
        }
    }

    /// The template, path and metadata of one selected value, or why it is
    /// skipped together with the path to show (a template when possible).
    fn check_value(
        &self,
        value: &str,
        base: &Path,
        seen: &mut HashSet<String>,
    ) -> Result<(PathTemplate, PathBuf, EntryMeta), (Skip, String)> {
        let normalized = normalize_value(value, &|name| std::env::var(name).ok());
        if normalized.is_empty() {
            return Err((Skip::NotAbsolute, value.to_owned()));
        }
        if is_unc(&normalized) {
            return Err((Skip::Network, normalized));
        }
        if let Some(letter) = drive_letter(&normalized) {
            let network =
                self.env.drives.iter().any(|d| {
                    d.letter.eq_ignore_ascii_case(&letter) && d.kind == DriveKind::Network
                });
            if network {
                return Err((Skip::Network, self.shown(Path::new(&normalized))));
            }
        }
        let path = lexical_clean(&base.join(&normalized));
        if !path.is_absolute() {
            return Err((Skip::NotAbsolute, normalized));
        }
        let shown = self.shown(&path);
        let system = [
            KnownFolder::WinDir,
            KnownFolder::ProgramFiles,
            KnownFolder::ProgramFilesX86,
        ]
        .into_iter()
        .filter_map(|folder| self.env.known_folder(folder))
        .any(|folder| starts_with_ci(&path, folder));
        if system {
            return Err((Skip::SystemFolder, shown));
        }
        if !seen.insert(path.to_string_lossy().to_lowercase()) {
            return Err((Skip::Duplicate, shown));
        }
        match self.fs.metadata(&path) {
            Ok(meta) => Ok((PathTemplate::from_path(&path, self.env), path, meta)),
            Err(_) => Err((Skip::Missing, shown)),
        }
    }

    /// `path` as a template, for issues (no user names in reports).
    fn shown(&self, path: &Path) -> String {
        PathTemplate::from_path(path, self.env).as_str().to_owned()
    }
}

/// A selected value as a path string (SPEC-04 §4.2.1 step 5): a `file://`
/// URI is percent-decoded, `%VAR%` are replaced by `var(VAR)` (unknown ones
/// are kept), separators become the platform separator (`\` on Windows) and
/// a `\\?\X:` prefix is dropped. Relative paths are resolved by the caller.
pub(crate) fn normalize_value(value: &str, var: &dyn Fn(&str) -> Option<String>) -> String {
    let decoded = decode_file_uri(value).unwrap_or_else(|| value.to_owned());
    let expanded = expand_vars(&decoded, var);
    let other = if MAIN_SEPARATOR == '\\' { '/' } else { '\\' };
    let mut path = expanded.replace(other, MAIN_SEPARATOR_STR);
    let verbatim = format!("{MAIN_SEPARATOR}{MAIN_SEPARATOR}?{MAIN_SEPARATOR}");
    if let Some(rest) = path.strip_prefix(&verbatim) {
        if drive_letter(rest).is_some() {
            path = rest.to_owned();
        }
    }
    path
}

/// `file:///C:/x%20y` → `C:/x y`; `file://host/share` → `//host/share`.
/// `None` when `value` is not a `file://` URI.
fn decode_file_uri(value: &str) -> Option<String> {
    let scheme = value.get(..7)?;
    if !scheme.eq_ignore_ascii_case("file://") {
        return None;
    }
    let rest = &value[7..];
    let (host, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    let path = percent_decode(path);
    if host.is_empty() || host.eq_ignore_ascii_case("localhost") {
        // `/C:/x` is a local Windows path; `/home/x` stays as is.
        let local = path.strip_prefix('/').filter(|p| drive_letter(p).is_some());
        Some(local.map_or_else(|| path.clone(), str::to_owned))
    } else {
        Some(format!("//{}{path}", percent_decode(host)))
    }
}

/// `%XX` escapes decoded as UTF-8 bytes (invalid sequences replaced).
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `%NAME%` replaced by `var(NAME)`; unknown names and a lone `%` are kept.
fn expand_vars(text: &str, var: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) if end > 0 => {
                let name = &after[..end];
                match var(name) {
                    Some(value) => out.push_str(&value),
                    None => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[end + 1..];
            }
            _ => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Two leading separators: a UNC path (`\\server\share`, `\\?\UNC\…`).
fn is_unc(path: &str) -> bool {
    let mut chars = path.chars();
    matches!(
        (chars.next(), chars.next()),
        (Some('\\' | '/'), Some('\\' | '/'))
    )
}

/// The drive letter of `X:…`, uppercase.
fn drive_letter(path: &str) -> Option<char> {
    let mut chars = path.chars();
    match (chars.next(), chars.next()) {
        (Some(letter), Some(':')) if letter.is_ascii_alphabetic() => {
            Some(letter.to_ascii_uppercase())
        }
        _ => None,
    }
}

/// `path` with `.` dropped and `..` applied, without touching the disk;
/// `..` above the root is ignored.
fn lexical_clean(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
#[path = "expand_json_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "expand_json_value_tests.rs"]
mod value_tests;
