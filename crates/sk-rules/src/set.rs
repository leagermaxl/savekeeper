//! The active rule set: built-in rules merged with user rules (SPEC-04 §4.4
//! step 4, §4.6, FR-04-06, FR-04-07).
//!
//! Built-in rules are the `rules/*.yaml` files embedded with `include_dir!`;
//! user rules are `*.yaml`/`*.yml` files in `savekeeper-data/rules.d/`. Both
//! are loaded in alphabetical file order. A user rule replaces the built-in
//! rule with the same id; a user rule with `disabled: true` removes it.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::{Path, PathBuf};

use include_dir::{include_dir, Dir};
use sk_core::model::{IssueSeverity, ScanIssue};

use crate::compile::{compile_yaml, CompiledRule};
use crate::diagnostic::{self, DiagnosticSeverity, RuleDiagnostic};
use crate::error::RuleError;

/// The repository's `rules/` folder, embedded at build time.
static BUILTIN: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../rules");

/// `ScanIssue::source` of the issues produced while loading rules.
pub(crate) const ISSUE_SOURCE: &str = "rules";

/// A user rule file was skipped (SPEC-04 §5); args `file`, `line`, `error`.
pub(crate) const ISSUE_INVALID_FILE: &str = "issue.rules.invalid_file";

/// Two user files define the same rule id, the later one wins (SPEC-04 §5);
/// args `rule_id`, `file` (the winner), `previous_file`.
pub(crate) const ISSUE_DUPLICATE_ID: &str = "issue.rules.duplicate_id";

/// The user rule folder exists but cannot be listed; arg `error`.
pub(crate) const ISSUE_USER_DIR_UNREADABLE: &str = "issue.rules.user_dir_unreadable";

/// The built-in rules failed to load (prevented by the `builtin_rules_valid`
/// test, FR-04-07); arg `error`.
pub(crate) const ISSUE_BUILTIN_INVALID: &str = "issue.rules.builtin_invalid";

/// Where an active rule comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleSource {
    /// A built-in file, by its name inside `rules/` (e.g. `dev.yaml`).
    Builtin {
        /// File name inside `rules/`.
        file: String,
    },
    /// A user file in `rules.d/` (shown as "your rule" in the UI).
    User {
        /// Full path of the file.
        file: PathBuf,
    },
}

impl RuleSource {
    /// Whether the rule comes from the user's `rules.d/`.
    pub fn is_user(&self) -> bool {
        matches!(self, Self::User { .. })
    }
}

/// Active rules ordered by `priority` desc, then `id` (SPEC-04 §4.4 step 4).
/// Disabled rules are not part of the set.
#[derive(Debug, Clone, Default)]
pub struct RuleSet {
    rules: Vec<CompiledRule>,
    /// Source of `rules[i]`.
    sources: Vec<RuleSource>,
    /// Rule id → index in `rules`.
    index: HashMap<String, usize>,
}

impl RuleSet {
    /// The built-in rules only. Fails on the first invalid file or on an id
    /// used twice across built-in files.
    pub fn builtin() -> Result<Self, RuleError> {
        let mut merger = Merger::default();
        merger.add_builtin(&builtin_files()?)?;
        Ok(merger.finish())
    }

    /// Built-in rules (when `builtin`) merged with the user rules in
    /// `user_dir` (when given).
    ///
    /// Never fails: an invalid user file is skipped with a `Warning` issue,
    /// a missing `user_dir` is not a problem, and if the built-in rules fail
    /// to load they are left out with an `Error` issue.
    pub fn load(builtin: bool, user_dir: Option<&Path>) -> (Self, Vec<ScanIssue>) {
        let mut merger = Merger::default();
        let mut issues = Vec::new();
        if builtin {
            let loaded = builtin_files().and_then(|files| merger.add_builtin(&files));
            if let Err(err) = loaded {
                issues.push(issue(
                    IssueSeverity::Error,
                    ISSUE_BUILTIN_INVALID,
                    [("error", err.to_string())],
                ));
            }
        }
        if let Some(dir) = user_dir {
            merger.add_user_dir(dir, &mut issues);
        }
        (merger.finish(), issues)
    }

    /// Checks one rule file for the CLI; see [`diagnostic::validate_file`].
    pub fn validate_file(path: &Path) -> Vec<RuleDiagnostic> {
        diagnostic::validate_file(path)
    }

    /// Number of active rules.
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// Whether there are no active rules.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// The active rule with this id.
    pub fn get(&self, id: &str) -> Option<&CompiledRule> {
        self.index.get(id).and_then(|&i| self.rules.get(i))
    }

    /// Where the active rule with this id comes from.
    pub fn source(&self, id: &str) -> Option<&RuleSource> {
        self.index.get(id).and_then(|&i| self.sources.get(i))
    }

    /// Active rules in order: `priority` desc, then `id`.
    pub(crate) fn rules(&self) -> &[CompiledRule] {
        &self.rules
    }

    /// A set of compiled rules merged as one built-in source, for tests.
    #[cfg(test)]
    pub(crate) fn from_rules(rules: Vec<CompiledRule>) -> Self {
        let mut merger = Merger::default();
        for rule in rules {
            let id = rule.id().to_owned();
            let entry = Entry {
                source: RuleSource::Builtin {
                    file: "test.yaml".to_owned(),
                },
                rule: (!rule.rule.disabled).then_some(rule),
            };
            merger.entries.insert(id, entry);
        }
        merger.finish()
    }
}

/// `(name, text)` of each built-in rule file, in alphabetical order.
pub(crate) fn builtin_files() -> Result<Vec<(&'static str, &'static str)>, RuleError> {
    let mut files = Vec::new();
    for file in BUILTIN.files() {
        let Some(name) = file.path().to_str().filter(|name| is_rule_file(name)) else {
            continue;
        };
        let text = file.contents_utf8().ok_or_else(|| RuleError::Invalid {
            rule_id: String::new(),
            reason: format!("built-in rule file {name} is not UTF-8"),
        })?;
        files.push((name, text));
    }
    files.sort_by(|a, b| file_order(a.0, b.0));
    Ok(files)
}

/// `*.yaml` or `*.yml`, any letter case.
fn is_rule_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".yaml") || lower.ends_with(".yml")
}

/// Alphabetical, case-insensitive first so the order is the same as in
/// Explorer, then exact for a stable tie-break.
fn file_order(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

/// A rule id in the merge: the rule, or `None` when it was disabled.
struct Entry {
    rule: Option<CompiledRule>,
    source: RuleSource,
}

/// Merges sources in order: built-in first, then user (SPEC-04 §4.4 step 4).
#[derive(Default)]
struct Merger {
    entries: BTreeMap<String, Entry>,
}

impl Merger {
    /// Adds built-in files all-or-nothing: on error nothing is added.
    fn add_builtin(&mut self, files: &[(&str, &str)]) -> Result<(), RuleError> {
        let mut added: BTreeMap<String, Entry> = BTreeMap::new();
        for (name, text) in files {
            let compiled = compile_yaml(text).map_err(first_error)?;
            for rule in compiled.rules {
                let id = rule.id().to_owned();
                if added.contains_key(&id) || self.entries.contains_key(&id) {
                    return Err(RuleError::DuplicateId(id));
                }
                let entry = Entry {
                    rule: (!rule.rule.disabled).then_some(rule),
                    source: RuleSource::Builtin {
                        file: (*name).to_owned(),
                    },
                };
                added.insert(id, entry);
            }
        }
        self.entries.extend(added);
        Ok(())
    }

    /// Adds every rule file of `dir`; a missing `dir` adds nothing.
    fn add_user_dir(&mut self, dir: &Path, issues: &mut Vec<ScanIssue>) {
        let files = match user_files(dir) {
            Ok(files) => files,
            Err(err) => {
                issues.push(issue(
                    IssueSeverity::Warning,
                    ISSUE_USER_DIR_UNREADABLE,
                    [("error", err.to_string())],
                ));
                return;
            }
        };
        for path in files {
            self.add_user_file(&path, issues);
        }
    }

    /// Adds one user file, or skips it whole with a `Warning` issue.
    fn add_user_file(&mut self, path: &Path, issues: &mut Vec<ScanIssue>) {
        let name = file_name(path);
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) => {
                let args = [
                    ("file", name),
                    ("error", format!("cannot read file: {err}")),
                ];
                issues.push(issue(IssueSeverity::Warning, ISSUE_INVALID_FILE, args));
                return;
            }
        };
        let compiled = match compile_yaml(&text) {
            Ok(compiled) => compiled,
            Err(errors) => {
                issues.push(invalid_file_issue(path, &text, name, &errors));
                return;
            }
        };
        for rule in compiled.rules {
            let id = rule.id().to_owned();
            if let Some(previous) = self.entries.get(&id) {
                if let RuleSource::User { file } = &previous.source {
                    let args = [
                        ("rule_id", id.clone()),
                        ("file", name.clone()),
                        ("previous_file", file_name(file)),
                    ];
                    issues.push(issue(IssueSeverity::Warning, ISSUE_DUPLICATE_ID, args));
                }
            }
            let entry = Entry {
                rule: (!rule.rule.disabled).then_some(rule),
                source: RuleSource::User {
                    file: path.to_path_buf(),
                },
            };
            self.entries.insert(id, entry);
        }
    }

    /// Active rules ordered by `priority` desc, then `id`.
    fn finish(self) -> RuleSet {
        let mut active: Vec<(CompiledRule, RuleSource)> = self
            .entries
            .into_values()
            .filter_map(|entry| entry.rule.map(|rule| (rule, entry.source)))
            .collect();
        active.sort_by(|(a, _), (b, _)| {
            b.rule
                .priority
                .cmp(&a.rule.priority)
                .then_with(|| a.id().cmp(b.id()))
        });
        let index = active
            .iter()
            .enumerate()
            .map(|(i, (rule, _))| (rule.id().to_owned(), i))
            .collect();
        let (rules, sources) = active.into_iter().unzip();
        RuleSet {
            rules,
            sources,
            index,
        }
    }
}

/// Rule files directly in `dir`, in alphabetical order; empty when `dir`
/// does not exist.
fn user_files(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let mut files = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.is_file() && is_rule_file(&file_name(&path)) {
            files.push(path);
        }
    }
    files.sort_by(|a, b| file_order(&file_name(a), &file_name(b)));
    Ok(files)
}

/// Only the file name goes into issues: the full path of `rules.d` may
/// contain the user name.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `invalid_file` issue with the line and rule of the first error.
fn invalid_file_issue(path: &Path, text: &str, name: String, errors: &[RuleError]) -> ScanIssue {
    let first: Option<RuleDiagnostic> = diagnostic::validate_str(path, text)
        .into_iter()
        .find(|d| d.severity == DiagnosticSeverity::Error);
    let mut args = vec![("file", name)];
    match first {
        Some(d) => {
            if let Some(line) = d.line {
                args.push(("line", line.to_string()));
            }
            if let Some(rule_id) = d.rule_id {
                args.push(("rule_id", rule_id));
            }
            args.push(("error", d.message));
        }
        None => {
            if let Some(err) = errors.first() {
                args.push(("error", err.to_string()));
            }
        }
    }
    issue(IssueSeverity::Warning, ISSUE_INVALID_FILE, args)
}

/// The first of the errors that rejected a built-in file.
fn first_error(errors: Vec<RuleError>) -> RuleError {
    errors
        .into_iter()
        .next()
        .unwrap_or_else(|| RuleError::Invalid {
            rule_id: String::new(),
            reason: "rule file rejected without a reason".to_owned(),
        })
}

pub(crate) fn issue(
    severity: IssueSeverity,
    message_key: &str,
    args: impl IntoIterator<Item = (&'static str, String)>,
) -> ScanIssue {
    ScanIssue {
        severity,
        source: ISSUE_SOURCE.to_owned(),
        path: None,
        message_key: message_key.to_owned(),
        message_args: args
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    }
}

#[cfg(test)]
#[path = "set_tests.rs"]
mod tests;
