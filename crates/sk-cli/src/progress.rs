//! Scan progress on standard error with `indicatif` (SPEC-01 §4.5).

use std::fmt::Write as _;
use std::time::Duration;

use indicatif::{ProgressBar, ProgressStyle};
use sk_core::events::{Event, LogLevel, ScanPhase};
use sk_core::model::{IssueSeverity, ScanIssue};

/// A spinner with the current phase; issues and log lines go above it.
/// Hidden when standard error is not a terminal, except the printed lines.
pub(crate) struct Progress {
    bar: ProgressBar,
}

impl Progress {
    pub(crate) fn new() -> Self {
        let bar = ProgressBar::new_spinner();
        if let Ok(style) = ProgressStyle::with_template("{spinner} {prefix:.bold} {wide_msg}") {
            bar.set_style(style);
        }
        bar.enable_steady_tick(Duration::from_millis(100));
        Self { bar }
    }

    /// Shows one pipeline event.
    pub(crate) fn handle(&self, event: Event) {
        match event {
            Event::PhaseStarted { phase } => {
                self.bar.set_prefix(phase_name(phase));
                self.bar.set_message("");
            }
            Event::Progress {
                done,
                total,
                current,
                ..
            } => self.bar.set_message(progress_message(done, total, current)),
            Event::Issue { issue } => self.print(&issue_line(&issue)),
            Event::Log { level, message } => {
                self.print(&format!("{}: {message}", level_name(level)))
            }
            Event::PhaseFinished { .. }
            | Event::FindingsAdded { .. }
            | Event::BackupProgress { .. } => {}
        }
    }

    /// Prints a line above the spinner, also when the spinner is hidden.
    pub(crate) fn print(&self, line: &str) {
        self.bar.suspend(|| eprintln!("{line}"));
    }

    pub(crate) fn finish(&self) {
        self.bar.finish_and_clear();
    }
}

fn phase_name(phase: ScanPhase) -> &'static str {
    match phase {
        ScanPhase::Environment => "environment",
        ScanPhase::Collect => "collect",
        ScanPhase::Heuristics => "heuristics",
        ScanPhase::Measure => "measure",
        ScanPhase::Classify => "classify",
        ScanPhase::Score => "score",
        ScanPhase::Done => "done",
    }
}

fn level_name(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Debug => "debug",
        LogLevel::Info => "info",
        LogLevel::Warn => "warning",
        LogLevel::Error => "error",
    }
}

fn progress_message(done: u64, total: Option<u64>, current: Option<String>) -> String {
    let mut text = match total {
        Some(total) => format!("{done}/{total}"),
        None => done.to_string(),
    };
    if let Some(current) = current {
        let _ = write!(text, " {current}");
    }
    text
}

/// `warning [games] collector.failed error=boom path={HOME}\x`.
pub(crate) fn issue_line(issue: &ScanIssue) -> String {
    let severity = match issue.severity {
        IssueSeverity::Info => "info",
        IssueSeverity::Warning => "warning",
        IssueSeverity::Error => "error",
    };
    let mut line = format!("{severity} [{}] {}", issue.source, issue.message_key);
    for (key, value) in &issue.message_args {
        let _ = write!(line, " {key}={value}");
    }
    if let Some(path) = &issue.path {
        let _ = write!(line, " path={path}");
    }
    line
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn issue_is_one_line() {
        let issue = ScanIssue {
            severity: IssueSeverity::Error,
            source: "games".to_owned(),
            path: Some(r"{HOME}\x".to_owned()),
            message_key: "collector.failed".to_owned(),
            message_args: BTreeMap::from([("error".to_owned(), "boom".to_owned())]),
        };
        assert_eq!(
            issue_line(&issue),
            r"error [games] collector.failed error=boom path={HOME}\x"
        );
    }

    #[test]
    fn progress_text() {
        assert_eq!(progress_message(3, Some(10), None), "3/10");
        assert_eq!(progress_message(3, None, Some("a".to_owned())), "3 a");
    }
}
