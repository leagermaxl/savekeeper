//! Issues of the Classify phase (SPEC-08 §5). Messages never contain request
//! data, keys or URLs; paths are anonymized templates.

use std::collections::BTreeMap;

use sk_core::model::{IssueSeverity, ScanIssue};

use super::exec::Failure;
use crate::{ClassifierInfo, OLLAMA_PROVIDER};

/// `ScanIssue::source` of all issues of the runner.
pub const ISSUE_SOURCE: &str = "llm";

/// One issue with `source = "llm"`.
pub(crate) fn issue(
    severity: IssueSeverity,
    key: &str,
    path: Option<String>,
    args: Vec<(&str, String)>,
) -> ScanIssue {
    ScanIssue {
        severity,
        source: ISSUE_SOURCE.to_owned(),
        path,
        message_key: key.to_owned(),
        message_args: args.into_iter().map(|(k, v)| (k.to_owned(), v)).collect(),
    }
}

/// `provider` and `model` arguments of every provider issue.
fn provider_args(info: &ClassifierInfo) -> Vec<(&'static str, String)> {
    vec![
        ("provider", info.provider.to_owned()),
        ("model", info.model.clone()),
    ]
}

/// `llm.unreachable` (the hint «start Ollama» comes from the UI by `provider`).
pub(crate) fn unreachable(info: &ClassifierInfo, error: &str) -> ScanIssue {
    let mut args = provider_args(info);
    args.push(("error", error.to_owned()));
    issue(IssueSeverity::Error, "llm.unreachable", None, args)
}

/// `llm.model_missing`, with `command = "ollama pull <model>"` for Ollama.
pub(crate) fn model_missing(info: &ClassifierInfo) -> ScanIssue {
    let mut args = provider_args(info);
    if info.provider == OLLAMA_PROVIDER {
        args.push(("command", format!("ollama pull {}", info.model)));
    }
    issue(IssueSeverity::Error, "llm.model_missing", None, args)
}

/// `llm.auth_failed`.
pub(crate) fn auth_failed(info: &ClassifierInfo) -> ScanIssue {
    issue(
        IssueSeverity::Error,
        "llm.auth_failed",
        None,
        provider_args(info),
    )
}

/// Issue `llm.no_api_key` (§5): `mode = cloud` without a stored key, i.e.
/// [`ClassifyRunner::from_config`](crate::ClassifyRunner::from_config)
/// returned [`LlmError::Auth`](crate::LlmError::Auth).
pub fn no_api_key_issue() -> ScanIssue {
    issue(IssueSeverity::Warning, "llm.no_api_key", None, Vec::new())
}

/// `llm.cache_error`: the cache cannot be read or written.
pub(crate) fn cache_error(error: &str) -> ScanIssue {
    issue(
        IssueSeverity::Warning,
        "llm.cache_error",
        None,
        vec![("error", error.to_owned())],
    )
}

/// Batch failures merged into one issue per message key, with `count` = the
/// number of items and `error` = the first error message, if any.
#[derive(Debug, Default)]
pub(crate) struct FailureIssues {
    by_key: BTreeMap<&'static str, (usize, Option<String>, Failure)>,
}

impl FailureIssues {
    /// Records `count` items that failed with `failure`.
    pub fn add(&mut self, failure: &Failure, count: usize) {
        let error = match failure {
            Failure::Unreachable(e) | Failure::BadJson(e) | Failure::Request(e) => Some(e.clone()),
            _ => None,
        };
        let entry = self
            .by_key
            .entry(key_of(failure))
            .or_insert((0, error, failure.clone()));
        entry.0 += count;
    }

    /// The merged issues, sorted by message key.
    pub fn into_issues(self, info: &ClassifierInfo) -> Vec<ScanIssue> {
        self.by_key
            .into_iter()
            .map(|(key, (count, error, failure))| {
                let mut issue = match failure {
                    Failure::Auth => auth_failed(info),
                    Failure::ModelMissing => model_missing(info),
                    Failure::Unreachable(e) => unreachable(info, &e),
                    _ => issue(IssueSeverity::Warning, key, None, provider_args(info)),
                };
                issue
                    .message_args
                    .insert("count".to_owned(), count.to_string());
                if let Some(error) = error {
                    issue.message_args.insert("error".to_owned(), error);
                }
                issue
            })
            .collect()
    }
}

fn key_of(failure: &Failure) -> &'static str {
    match failure {
        Failure::Auth => "llm.auth_failed",
        Failure::Unreachable(_) => "llm.unreachable",
        Failure::ModelMissing => "llm.model_missing",
        Failure::Refusal => "llm.refusal",
        Failure::Truncated => "llm.truncated",
        Failure::BadJson(_) => "llm.bad_json",
        Failure::Request(_) => "llm.request_failed",
        Failure::Missing => "llm.missing_result",
    }
}
