//! The configuration of the repository that decides how git may run on it (SPEC-06
//! §4.3, `git-bundle`): its filters (switched off for `git status`), whether it is a
//! partial clone (no bundle) and its object format (the empty tree of
//! `GIT_ATTR_SOURCE`).

use std::ffi::OsStr;

use super::{process_failed, Job, STATUS_TIMEOUT};
use crate::cmd::CmdRunner;
use crate::error::ExportError;

/// The SHA-1 empty tree.
pub(crate) const EMPTY_TREE_SHA1: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// The SHA-256 empty tree (`extensions.objectformat=sha256`).
pub(crate) const EMPTY_TREE_SHA256: &str =
    "6ef19b41225c5369f1c104d45d8d85efa9b057b53b14b4b9b939dd74decc5321";

/// The one configuration query (SPEC-06 §4.3).
pub(crate) const QUERY: &str = concat!(
    r"^(filter\..*|extensions\.(partialclone|objectformat)|remote\..*\.promisor",
    r"|core\.(splitindex|worktree))$"
);

/// What the configuration query found.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct RepoConfig {
    /// Names of the defined filters, once each, in order.
    pub(crate) filters: Vec<String>,
    /// A filter name that cannot be passed with `-c` (it contains `=`).
    pub(crate) unpassable_filter: Option<String>,
    /// `extensions.partialclone` is set or a remote is a promisor: objects may be
    /// missing and would be fetched.
    pub(crate) partial_clone: bool,
    /// `extensions.objectformat=sha256`.
    pub(crate) sha256: bool,
    /// `core.splitIndex` is true: reading the index touches `sharedindex.*`.
    pub(crate) split_index: bool,
    /// `core.worktree` is set: `git status` paths would belong to another folder.
    pub(crate) core_worktree: bool,
}

impl RepoConfig {
    /// The empty tree of the repository's object format, for `GIT_ATTR_SOURCE`.
    pub(crate) fn empty_tree(&self) -> &'static str {
        if self.sha256 {
            EMPTY_TREE_SHA256
        } else {
            EMPTY_TREE_SHA1
        }
    }

    /// `-c` options that switch every filter off.
    pub(crate) fn filters_off(&self) -> Vec<String> {
        self.filters
            .iter()
            .flat_map(|name| {
                ["clean=", "smudge=", "process=", "required=false"]
                    .map(|key| format!("filter.{name}.{key}"))
            })
            .collect()
    }
}

/// Runs `git config -z --get-regexp <QUERY>`; reading the configuration runs no filter
/// and writes nothing. Exit code 1 means nothing matched.
pub(crate) async fn read(runner: &dyn CmdRunner, job: &Job<'_>) -> Result<RepoConfig, ExportError> {
    let args: [&OsStr; 3] = ["config".as_ref(), "-z".as_ref(), "--get-regexp".as_ref()];
    let cmd = job.git(
        &[],
        args.into_iter().chain([OsStr::new(QUERY)]),
        STATUS_TIMEOUT,
    );
    let output = runner.run(cmd, job.cancel).await?;
    match output.code {
        0 => Ok(parse(&output.stdout)),
        1 => Ok(RepoConfig::default()),
        _ => Err(process_failed(&output)),
    }
}

/// Parses `git config -z --get-regexp` output: NUL-terminated entries `key\nvalue` (no
/// `\nvalue` for a key without value). Section and variable names come lowercased,
/// the filter name (subsection) as written.
pub(crate) fn parse(stdout: &str) -> RepoConfig {
    let mut config = RepoConfig::default();
    for entry in stdout.split('\0').filter(|e| !e.is_empty()) {
        let (key, value) = entry.split_once('\n').unwrap_or((entry, ""));
        if let Some(rest) = key.strip_prefix("filter.") {
            let Some((name, _variable)) = rest.rsplit_once('.') else {
                continue;
            };
            if name.contains('=') {
                config
                    .unpassable_filter
                    .get_or_insert_with(|| name.to_owned());
            } else if !config.filters.iter().any(|n| n == name) {
                config.filters.push(name.to_owned());
            }
        } else if key.eq_ignore_ascii_case("extensions.partialclone") {
            config.partial_clone = true;
        } else if key.starts_with("remote.") && key.ends_with(".promisor") {
            config.partial_clone |= is_true(entry, value);
        } else if key.eq_ignore_ascii_case("extensions.objectformat") {
            config.sha256 = value.trim().eq_ignore_ascii_case("sha256");
        } else if key.eq_ignore_ascii_case("core.splitindex") {
            // The last value wins, as in git.
            config.split_index = is_true(entry, value);
        } else if key.eq_ignore_ascii_case("core.worktree") {
            config.core_worktree = true;
        }
    }
    config
}

/// A boolean of `git config`: a key without value is true; an empty value, `false`,
/// `no`, `off` and `0` are false.
fn is_true(entry: &str, value: &str) -> bool {
    if !entry.contains('\n') {
        return true;
    }
    let value = value.trim();
    !["", "false", "no", "off", "0"]
        .iter()
        .any(|v| value.eq_ignore_ascii_case(v))
}
