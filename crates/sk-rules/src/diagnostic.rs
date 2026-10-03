//! Rule file diagnostics with line numbers (SPEC-04 §4.1, FR-04-08).
//!
//! YAML, type and template errors carry their position from `serde-saphyr`.
//! Validation errors (SPEC-04 §4.4) point at the rule they belong to: its
//! line comes from a second, lenient parse that only records where each rule
//! starts.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_saphyr::Spanned;

use crate::compile::{compile_into, Compilation};
use crate::error::RuleError;
use crate::schema::RuleFile;

/// One problem in a rule file, for `savekeeper-cli rules validate`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleDiagnostic {
    /// The checked file.
    pub file: PathBuf,
    /// 1-based line, when known.
    pub line: Option<usize>,
    /// Id of the rule the problem belongs to, when known.
    pub rule_id: Option<String>,
    /// Whether the problem rejects the file.
    pub severity: DiagnosticSeverity,
    /// Human-readable description.
    pub message: String,
}

/// How serious a [`RuleDiagnostic`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiagnosticSeverity {
    /// The file is invalid and is not loaded.
    Error,
    /// An automatic fix (SPEC-04 §4.4, e.g. `credentials` raised to
    /// sensitivity `high`); the file stays valid.
    Warning,
}

/// Reads and checks a rule file; an empty result means the file is valid.
pub fn validate_file(path: &Path) -> Vec<RuleDiagnostic> {
    match std::fs::read_to_string(path) {
        Ok(text) => validate_str(path, &text),
        Err(err) => vec![RuleDiagnostic {
            file: path.to_path_buf(),
            line: None,
            rule_id: None,
            severity: DiagnosticSeverity::Error,
            message: format!("cannot read file: {err}"),
        }],
    }
}

/// Checks rule file `text`; `file` is only copied into the diagnostics.
///
/// Errors come first, then warnings about automatic fixes (for example
/// `credentials` raised to sensitivity `high`).
pub fn validate_str(file: &Path, text: &str) -> Vec<RuleDiagnostic> {
    let starts = RuleStarts::parse(text);
    let error = |line: Option<usize>, rule_id: Option<String>, message: String| RuleDiagnostic {
        file: file.to_path_buf(),
        line,
        rule_id,
        severity: DiagnosticSeverity::Error,
        message,
    };

    let parsed = match RuleFile::from_yaml(text) {
        Ok(parsed) => parsed,
        Err(RuleError::Yaml(err)) => {
            let line = err.location().and_then(|loc| to_line(loc.line()));
            let rule_id = line.and_then(|line| starts.rule_at(line));
            return vec![error(line, rule_id, err.without_snippet().to_string())];
        }
        Err(err) => return vec![error(None, None, err.to_string())],
    };

    let mut out = Compilation::default();
    compile_into(parsed, &mut out);
    let mut diagnostics = Vec::with_capacity(out.errors.len() + out.warnings.len());
    for (index, err) in out.errors {
        // File-level problems (no rule index) point at `schema_version`.
        let line = match index {
            Some(_) => starts.line_of(index),
            None => starts.schema_version,
        };
        let rule_id = match (&err, index) {
            (RuleError::Invalid { rule_id, .. } | RuleError::DuplicateId(rule_id), Some(_)) => {
                non_empty(rule_id)
            }
            _ => None,
        };
        let message = match err {
            RuleError::Invalid { reason, .. } => reason,
            other => other.to_string(),
        };
        diagnostics.push(error(line, rule_id, message));
    }
    for (index, warning) in out.warnings {
        diagnostics.push(RuleDiagnostic {
            file: file.to_path_buf(),
            line: starts.line_of(index),
            rule_id: non_empty(&warning.rule_id),
            severity: DiagnosticSeverity::Warning,
            message: warning.message,
        });
    }
    diagnostics
}

/// A rule written as `id: ""` has no id to report.
fn non_empty(rule_id: &str) -> Option<String> {
    (!rule_id.is_empty()).then(|| rule_id.to_owned())
}

/// `serde-saphyr` lines are 1-based; 0 means unknown.
fn to_line(line: u64) -> Option<usize> {
    usize::try_from(line).ok().filter(|line| *line > 0)
}

/// Where the rules of a file start, from a lenient parse that ignores every
/// field except `id`. Empty when the YAML itself is broken.
#[derive(Debug, Default)]
struct RuleStarts {
    schema_version: Option<usize>,
    /// `(line, id)` of each rule, in file order.
    rules: Vec<(Option<usize>, Option<String>)>,
}

#[derive(Deserialize)]
struct LenientFile {
    #[serde(default)]
    schema_version: Option<Spanned<serde::de::IgnoredAny>>,
    #[serde(default)]
    rules: Vec<Spanned<LenientRule>>,
}

#[derive(Deserialize)]
struct LenientRule {
    #[serde(default)]
    id: Option<String>,
}

impl RuleStarts {
    fn parse(text: &str) -> Self {
        let Ok(file) = serde_saphyr::from_str::<LenientFile>(text) else {
            return Self::default();
        };
        Self {
            schema_version: file
                .schema_version
                .and_then(|v| to_line(v.referenced.line())),
            rules: file
                .rules
                .into_iter()
                .map(|rule| (to_line(rule.referenced.line()), rule.value.id))
                .collect(),
        }
    }

    fn line_of(&self, index: Option<usize>) -> Option<usize> {
        index
            .and_then(|i| self.rules.get(i))
            .and_then(|(line, _)| *line)
    }

    /// Id of the last rule starting at or before `line`.
    fn rule_at(&self, line: usize) -> Option<String> {
        self.rules
            .iter()
            .rev()
            .find(|(start, _)| start.is_some_and(|start| start <= line))
            .and_then(|(_, id)| id.as_deref().and_then(non_empty))
    }
}

#[cfg(test)]
#[path = "diagnostic_tests.rs"]
mod tests;
