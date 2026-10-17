//! Findings and issues of H-GIT (SPEC-07 §4.4.3, §4.4.4).

use std::collections::BTreeMap;
use std::path::Path;

use sk_core::model::{Category, Finding, IssueSeverity, ScanIssue, Target};
use sk_core::privacy::{redact, redact_path};
use sk_core::template::PathTemplate;

use crate::collector::COLLECTOR_ID;
use crate::finding::{new_finding, HUGE_BYTES};
use crate::git::HEURISTIC_ID;
use crate::git_state::{Analysis, GitState};
use crate::heuristic::HeuristicContext;
use crate::verdict::MarkerVerdict;
use crate::walk_tree::DirNode;

/// Issue: gix could not read a repository (`{error}`).
pub(crate) const ISSUE_FAILED: &str = "heuristic.git_failed";
/// Issue: more repositories than `git.max_repos` (`{count, limit}`).
pub(crate) const ISSUE_TRUNCATED: &str = "heuristic.git_truncated";
/// Reason: the repository has no remote (§4.4.3).
pub(crate) const REASON_NO_REMOTE: &str = "heuristic.git.no_remote";
/// Reason: changes, unpushed commits or stash entries (§4.4.3).
pub(crate) const REASON_UNSAVED: &str = "heuristic.git.unsaved";
/// Reason: everything is committed and pushed (§4.4.3).
pub(crate) const REASON_CLEAN: &str = "heuristic.git.clean";
/// Reason: gix could not read the repository (§4.4.2).
pub(crate) const REASON_FAILED: &str = "heuristic.git.failed";
/// Reason: not read, the limit of repositories was reached (NFR-07-03).
pub(crate) const REASON_NOT_ANALYZED: &str = "heuristic.git.not_analyzed";
/// Tags of §4.4.3 and §4.8.
pub(crate) const TAG_GIT: &str = "git";
pub(crate) const TAG_NO_REMOTE: &str = "git-no-remote";
pub(crate) const TAG_UNSAVED: &str = "git-unsaved";
pub(crate) const TAG_CLEAN: &str = "git-clean";
/// Tag of a repository that was not read because of the limit (NFR-07-03).
pub(crate) const TAG_TRUNCATED: &str = "truncated";
const TAG_CLOUD_SYNCED: &str = "cloud-synced";
const TAG_HUGE: &str = "huge";
/// Confidence of the rows of §4.4.3.
const UNSAVED_CONFIDENCE: f32 = 0.95;
const CLEAN_CONFIDENCE: f32 = 0.85;
/// Confidence of a repository without a state (§4.4.2).
const NO_STATE_CONFIDENCE: f32 = 0.5;

/// What reading a repository gave.
pub(crate) enum Outcome {
    /// The state was read.
    Analyzed(Analysis),
    /// gix failed with this error.
    Failed(String),
    /// Not read: the limit of repositories was reached.
    NotAnalyzed,
}

/// The finding of a repository (§4.4.3, §4.4.4).
pub(crate) fn repo_finding(
    hctx: &HeuristicContext<'_>,
    node: &DirNode,
    bare: bool,
    outcome: &Outcome,
    exclude: Vec<String>,
) -> Finding {
    let path = node.path.as_path();
    let mut tags = vec![TAG_GIT.to_owned()];
    let mut args = BTreeMap::new();
    let verdict = match outcome {
        Outcome::Analyzed(a) => {
            let cloud_only = node.subtree.cloud_only_files;
            let (verdict, row_tags) = classify(&a.state, cloud_only > 0 && !bare);
            tags.extend(row_tags.iter().map(|t| (*t).to_owned()));
            args = state_args(&a.state, &a.remote_host);
            if cloud_only > 0 {
                tags.push(TAG_CLOUD_SYNCED.to_owned());
                args.insert("cloud_only_files".to_owned(), cloud_only.to_string());
            }
            verdict
        }
        Outcome::Failed(_) => {
            MarkerVerdict::whole(Category::DevEnvironment, NO_STATE_CONFIDENCE, REASON_FAILED)
        }
        Outcome::NotAnalyzed => {
            tags.push(TAG_TRUNCATED.to_owned());
            MarkerVerdict::whole(
                Category::DevEnvironment,
                NO_STATE_CONFIDENCE,
                REASON_NOT_ANALYZED,
            )
        }
    };
    if node.subtree.bytes > HUGE_BYTES {
        tags.push(TAG_HUGE.to_owned());
    }
    let target = Target::FileSet {
        root: PathTemplate::from_path(path, hctx.env),
        resolved: path.to_path_buf(),
        include: vec!["**".to_owned()],
        exclude,
    };
    let mut finding = new_finding(target, path, &verdict, args, HEURISTIC_ID, hctx.env, tags);
    // A repository is the user's project, not an application.
    finding.app = None;
    finding
}

/// The row of §4.4.3 and its tags. A repository without remotes is unsaved
/// as a whole (FR-07-05); a work tree that could not be looked at
/// (`worktree_unknown`) is not taken for clean.
pub(crate) fn classify(
    state: &GitState,
    worktree_unknown: bool,
) -> (MarkerVerdict, &'static [&'static str]) {
    if state.remotes.is_empty() {
        let v = MarkerVerdict::whole(
            Category::DevEnvironment,
            UNSAVED_CONFIDENCE,
            REASON_NO_REMOTE,
        );
        (v, &[TAG_NO_REMOTE, TAG_UNSAVED])
    } else if state.unsaved() > 0 || worktree_unknown {
        let v = MarkerVerdict::whole(Category::DevEnvironment, UNSAVED_CONFIDENCE, REASON_UNSAVED);
        (v, &[TAG_UNSAVED])
    } else {
        let v = MarkerVerdict::whole(Category::Reinstallable, CLEAN_CONFIDENCE, REASON_CLEAN);
        (v, &[TAG_CLEAN])
    }
}

/// `dirty`, `untracked`, `unpushed`, `stash`, `branches` (local branches
/// without an upstream) and `remote` (host, without credentials).
pub(crate) fn state_args(state: &GitState, remote_host: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("dirty".to_owned(), state.dirty_files.to_string()),
        ("untracked".to_owned(), state.untracked_files.to_string()),
        ("unpushed".to_owned(), state.unpushed_commits.to_string()),
        ("stash".to_owned(), state.stash_entries.to_string()),
        ("branches".to_owned(), state.local_only_branches.to_string()),
        ("remote".to_owned(), remote_host.to_owned()),
    ])
}

/// The issue `heuristic.git_failed {error}`; paths and names are redacted.
pub(crate) fn failed_issue(hctx: &HeuristicContext<'_>, path: &Path, err: &str) -> ScanIssue {
    ScanIssue {
        severity: IssueSeverity::Warning,
        source: COLLECTOR_ID.to_owned(),
        path: Some(redact_path(path, hctx.env)),
        message_key: ISSUE_FAILED.to_owned(),
        message_args: BTreeMap::from([("error".to_owned(), redact_error(hctx, path, err))]),
    }
}

/// The error text with the repository path replaced by its template.
fn redact_error(hctx: &HeuristicContext<'_>, path: &Path, err: &str) -> String {
    let template = PathTemplate::from_path(path, hctx.env);
    let text = err.replace(&*path.to_string_lossy(), template.as_str());
    redact(&text, hctx.env)
}

/// The issue `heuristic.git_truncated {count, limit}`.
pub(crate) fn truncated_issue(count: usize, limit: usize) -> ScanIssue {
    ScanIssue {
        severity: IssueSeverity::Warning,
        source: COLLECTOR_ID.to_owned(),
        path: None,
        message_key: ISSUE_TRUNCATED.to_owned(),
        message_args: BTreeMap::from([
            ("count".to_owned(), count.to_string()),
            ("limit".to_owned(), limit.to_string()),
        ]),
    }
}
