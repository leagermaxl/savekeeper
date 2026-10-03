//! `rules validate`: check rule files and print their diagnostics
//! (SPEC-04 FR-04-08, SPEC-01 §4.9).
//!
//! Each diagnostic is one line on standard output:
//! `<file>[:<line>]: <error|warning>[: rule `<id>`]: <message>`. The summary
//! goes to standard error. Exit codes: `1` if any file has an error, `3` if
//! there are only warnings, `0` otherwise.

use std::fmt::Write as _;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use sk_rules::{DiagnosticSeverity, RuleDiagnostic, RuleSet};

use crate::cli::ValidateArgs;
use crate::commands;
use crate::Status;

/// Runs `rules validate`.
pub(crate) fn validate(args: ValidateArgs) -> anyhow::Result<Status> {
    let mut found = if args.paths.is_empty() {
        let dir = commands::data_dir()?.rules();
        match rule_files(&dir) {
            Ok(files) => Found {
                files,
                diagnostics: Vec::new(),
            },
            // No user rule folder is not an error (SPEC-04 §4.6).
            Err(error) if error.kind() == io::ErrorKind::NotFound => Found::default(),
            Err(error) => {
                return Err(error).with_context(|| format!("cannot read {}", dir.display()))
            }
        }
    } else {
        collect(&args.paths)
    };

    for file in &found.files {
        found.diagnostics.extend(RuleSet::validate_file(file));
    }
    let summary = Summary::new(&found);
    let mut out = io::stdout().lock();
    for diagnostic in &found.diagnostics {
        writeln!(out, "{}", format_diagnostic(diagnostic))?;
    }
    out.flush()?;
    eprintln!("{}", summary.describe());
    summary.status()
}

/// The files to check, and the problems met while looking for them.
#[derive(Debug, Default)]
struct Found {
    files: Vec<PathBuf>,
    diagnostics: Vec<RuleDiagnostic>,
}

/// The files named on the command line: a folder gives its rule files, any
/// other path is checked as a file (a missing one is reported by
/// `validate_file`).
fn collect(paths: &[PathBuf]) -> Found {
    let mut found = Found::default();
    for path in paths {
        if !path.is_dir() {
            found.files.push(path.clone());
            continue;
        }
        match rule_files(path) {
            Ok(files) => found.files.extend(files),
            Err(error) => found.diagnostics.push(RuleDiagnostic {
                file: path.clone(),
                line: None,
                rule_id: None,
                severity: DiagnosticSeverity::Error,
                message: format!("cannot read folder: {error}"),
            }),
        }
    }
    found
}

/// `*.yaml`/`*.yml` files directly in `dir` (any letter case, no subfolders),
/// in the order `rules.d` is loaded: alphabetical without case, then exact
/// (SPEC-04 §4.6).
fn rule_files(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_file() && is_rule_file(&path) {
            files.push(path);
        }
    }
    files.sort_by(|a, b| {
        let (a, b) = (file_name(a), file_name(b));
        a.to_lowercase()
            .cmp(&b.to_lowercase())
            .then_with(|| a.cmp(&b))
    });
    Ok(files)
}

fn is_rule_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("yaml") || ext.eq_ignore_ascii_case("yml"))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `<file>[:<line>]: <severity>[: rule `<id>`]: <message>`; the next lines of
/// a multi-line message are indented.
fn format_diagnostic(diagnostic: &RuleDiagnostic) -> String {
    let mut text = diagnostic.file.display().to_string();
    if let Some(line) = diagnostic.line {
        let _ = write!(text, ":{line}");
    }
    let severity = match diagnostic.severity {
        DiagnosticSeverity::Error => "error",
        DiagnosticSeverity::Warning => "warning",
    };
    let _ = write!(text, ": {severity}");
    if let Some(rule_id) = &diagnostic.rule_id {
        let _ = write!(text, ": rule `{rule_id}`");
    }
    let message = diagnostic.message.trim_end().replace('\n', "\n    ");
    let _ = write!(text, ": {message}");
    text
}

/// Counts for the summary line and the exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Summary {
    files: usize,
    /// Files (or folders) with at least one error.
    invalid: usize,
    errors: usize,
    warnings: usize,
}

impl Summary {
    fn new(found: &Found) -> Self {
        let errors: Vec<&Path> = found
            .diagnostics
            .iter()
            .filter(|d| d.severity == DiagnosticSeverity::Error)
            .map(|d| d.file.as_path())
            .collect();
        let mut invalid = errors.clone();
        invalid.sort();
        invalid.dedup();
        Self {
            files: found.files.len(),
            invalid: invalid.len(),
            errors: errors.len(),
            warnings: found.diagnostics.len() - errors.len(),
        }
    }

    fn describe(&self) -> String {
        format!(
            "{}, {}, {}",
            count(self.files, "rule file checked", "rule files checked"),
            count(self.errors, "error", "errors"),
            count(self.warnings, "warning", "warnings"),
        )
    }

    /// Errors fail the command (`1`), warnings only give `3` (SPEC-01 §4.9).
    fn status(&self) -> anyhow::Result<Status> {
        if self.invalid > 0 {
            anyhow::bail!(
                "{} invalid",
                count(self.invalid, "rule file is", "rule files are")
            );
        }
        Ok(if self.warnings > 0 {
            Status::Warnings
        } else {
            Status::Success
        })
    }
}

fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diagnostic(
        file: &str,
        line: Option<usize>,
        rule_id: Option<&str>,
        severity: DiagnosticSeverity,
        message: &str,
    ) -> RuleDiagnostic {
        RuleDiagnostic {
            file: PathBuf::from(file),
            line,
            rule_id: rule_id.map(str::to_owned),
            severity,
            message: message.to_owned(),
        }
    }

    #[test]
    fn diagnostics_are_one_line_with_position_severity_and_rule() {
        assert_eq!(
            format_diagnostic(&diagnostic(
                "my.yaml",
                Some(12),
                Some("app.one"),
                DiagnosticSeverity::Error,
                "`app` is required",
            )),
            "my.yaml:12: error: rule `app.one`: `app` is required"
        );
        assert_eq!(
            format_diagnostic(&diagnostic(
                "my.yaml",
                None,
                None,
                DiagnosticSeverity::Warning,
                "raised\nto high\n",
            )),
            "my.yaml: warning: raised\n    to high"
        );
    }

    #[test]
    fn errors_fail_and_warnings_give_3() {
        let found = |diagnostics| Found {
            files: vec![PathBuf::from("a.yaml"), PathBuf::from("b.yaml")],
            diagnostics,
        };
        let clean = Summary::new(&found(vec![]));
        assert_eq!(
            clean.describe(),
            "2 rule files checked, 0 errors, 0 warnings"
        );
        assert_eq!(clean.status().unwrap(), Status::Success);

        let warned = Summary::new(&found(vec![diagnostic(
            "a.yaml",
            Some(3),
            None,
            DiagnosticSeverity::Warning,
            "w",
        )]));
        assert_eq!(
            warned.describe(),
            "2 rule files checked, 0 errors, 1 warning"
        );
        assert_eq!(warned.status().unwrap(), Status::Warnings);

        let failed = Summary::new(&found(vec![
            diagnostic("a.yaml", Some(3), None, DiagnosticSeverity::Error, "e1"),
            diagnostic("a.yaml", Some(4), None, DiagnosticSeverity::Error, "e2"),
            diagnostic("b.yaml", None, None, DiagnosticSeverity::Warning, "w"),
        ]));
        assert_eq!((failed.invalid, failed.errors, failed.warnings), (1, 2, 1));
        assert_eq!(
            failed.status().unwrap_err().to_string(),
            "1 rule file is invalid"
        );
    }

    #[test]
    fn folders_give_their_rule_files_in_load_order() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["c.YML", "B.yaml", "d.txt", "a.yaml"] {
            std::fs::write(dir.path().join(name), "").unwrap();
        }
        std::fs::create_dir(dir.path().join("sub.yaml")).unwrap();
        let names: Vec<String> = rule_files(dir.path())
            .unwrap()
            .iter()
            .map(|p| file_name(p))
            .collect();
        assert_eq!(names, ["a.yaml", "B.yaml", "c.YML"]);

        let file = dir.path().join("d.txt");
        let found = collect(&[dir.path().to_path_buf(), file.clone()]);
        assert_eq!(found.files.len(), 4);
        assert_eq!(found.files[3], file);
        assert!(found.diagnostics.is_empty());
    }
}
