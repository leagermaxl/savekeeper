//! The `git-bundle` exporter (SPEC-06 §4.3): a `git bundle` of all refs of a repository
//! plus a copy of its changed and untracked files, the alternative to copying a
//! repository with unsaved work (SPEC-07 §4.4.4).
//!
//! The exporter plans no finding of its own: H-GIT creates them as children of the
//! repository findings, with `params = { "repo": <PathTemplate> }`. `git.exe` is looked
//! up only when the export runs; without it the result is
//! [`ExportError::NotAvailable`].
//!
//! The repository is only read (principle P1): git runs with `--no-optional-locks` and
//! `GIT_OPTIONAL_LOCKS=0` (no index refresh is written), `core.fsmonitor=false` (no
//! hook or daemon is started) and without clean/smudge filters (no filter program runs
//! or writes, e.g. into `.git/lfs`). Git does not run at all on a repository with
//! cloud-only files in `.git`, and `git status` not on one with cloud-only files in the
//! work tree; the changed files are copied without opening cloud-only ones (SPEC-10
//! FR-10-15).

mod cloud;
mod config;
mod gitdir;
mod repo;
mod status;

#[cfg(test)]
mod gitdir_tests;
#[cfg(test)]
mod real_git_tests;
#[cfg(test)]
mod safety_tests;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sk_core::env::Environment;
use sk_core::model::Finding;
use sk_core::CancellationToken;

use super::partial::{Partial, ISSUE_FILE_COPY_FAILED};
use super::source::{self, SourceFs};
use super::{copy_file, describe_file, failure_text, unavailable, warning, NOT_WINDOWS};
use crate::cmd::{Cmd, CmdOutput, CmdRunner, OutputEncoding, SystemCmdRunner};
use crate::error::ExportError;
use crate::exporter::{
    Availability, ExportContext, ExportResult, ExportedFile, RestoreHint, SystemExporter,
};
use config::RepoConfig;
use gitdir::Precheck;
use repo::{
    has_worktree, inherited_git_vars, remove_own_output, repo_path, restore_hint, unique_slug,
};

/// Exporter identifier.
pub(crate) const ID: &str = "git-bundle";

/// `reason_key`: `git.exe` is not in `PATH`.
const MISSING: &str = "system.git_bundle.missing";

/// Warning: `git bundle create` failed, only the changed files were saved. Args: `error`.
pub(crate) const ISSUE_BUNDLE_FAILED: &str = "issue.system.git_bundle_failed";

/// Warning: `git status` failed, the changed files were not saved. Args: `error`.
pub(crate) const ISSUE_STATUS_FAILED: &str = "issue.system.git_status_failed";

/// Warning: the work tree has a cloud-only file, `git status` was not run and the
/// uncommitted files were not saved. Args: `path`.
pub(crate) const ISSUE_WORKTREE_CLOUD_ONLY: &str = "issue.system.git_worktree_cloud_only";

/// `error` of [`ISSUE_STATUS_FAILED`] when the `git status` output was cut at
/// [`crate::cmd::OUTPUT_LIMIT`].
pub(crate) const STATUS_TRUNCATED: &str = "output truncated";

/// Git LFS switched off in every call (SPEC-06 §4.3).
const LFS_OFF: [&str; 4] = [
    "filter.lfs.process=",
    "filter.lfs.clean=",
    "filter.lfs.smudge=",
    "filter.lfs.required=false",
];

/// `error` of [`ISSUE_BUNDLE_FAILED`] for a partial clone: the bundle would fetch the
/// missing objects into the repository.
pub(crate) const PARTIAL_CLONE: &str = "partial clone";

/// `error` of [`ISSUE_BUNDLE_FAILED`]: the `gitdir:` folder is on a network share.
pub(crate) const REMOTE_GITDIR: &str = "remote gitdir";

/// `error` of [`ISSUE_STATUS_FAILED`]: split index, reading it would touch the source.
pub(crate) const SPLIT_INDEX: &str = "split index";

/// `error` of [`ISSUE_STATUS_FAILED`]: `core.worktree` is set.
pub(crate) const CORE_WORKTREE: &str = "core.worktree";

/// Executable name.
#[cfg(windows)]
pub(crate) const EXE: &str = "git.exe";
#[cfg(not(windows))]
pub(crate) const EXE: &str = "git";

/// Timeout of `git bundle create`: large repositories take minutes to pack.
const BUNDLE_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// Timeout of `git status` on a large working copy.
const STATUS_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// `git bundle create` of a repository and a copy of its uncommitted files.
pub(crate) struct GitBundleExporter {
    runner: Arc<dyn CmdRunner>,
    /// The changed files to copy; a cloud-only one is not opened (SPEC-10 FR-10-15).
    source: Arc<dyn SourceFs>,
    /// `PATH` to search for `git.exe`; `None` means the `PATH` of this process.
    path_var: Option<OsString>,
}

impl GitBundleExporter {
    /// The exporter with real processes and files.
    pub(crate) fn new() -> Self {
        Self {
            runner: Arc::new(SystemCmdRunner),
            source: source::real(),
            path_var: None,
        }
    }

    /// The exporter with `runner`, `source` and the given `PATH` (tests).
    #[cfg(test)]
    pub(crate) fn with_parts(
        runner: Arc<dyn CmdRunner>,
        source: Arc<dyn SourceFs>,
        path_var: Option<OsString>,
    ) -> Self {
        Self {
            runner,
            source,
            path_var,
        }
    }

    /// Full path of `git.exe`: the first one in the absolute entries of `PATH`
    /// (SPEC-06 §4.6 item 3: no lookup by the OS).
    fn locate(&self) -> Option<PathBuf> {
        let process_path;
        let path_var = match &self.path_var {
            Some(path_var) => Some(path_var.as_os_str()),
            None => {
                process_path = std::env::var_os("PATH");
                process_path.as_deref()
            }
        };
        path_var
            .into_iter()
            .flat_map(std::env::split_paths)
            .filter(|dir| dir.is_absolute())
            .map(|dir| dir.join(EXE))
            .find(|exe| exe.is_file())
    }

    /// `git bundle create <target>/<slug>.bundle --all`: the bundle file, or the failure
    /// recorded in `failures`. A failed or cut-off bundle is removed again (FR-06-02).
    async fn bundle(
        &self,
        job: &Job<'_>,
        failures: &mut Partial,
    ) -> Result<Option<ExportedFile>, ExportError> {
        let name = format!("{}.bundle", job.slug);
        let out = job.target.join(&name);
        let args: [&OsStr; 4] = [
            "bundle".as_ref(),
            "create".as_ref(),
            out.as_os_str(),
            "--all".as_ref(),
        ];
        let result = self
            .runner
            .run(job.git(&[], args, BUNDLE_TIMEOUT), job.cancel)
            .await;
        let failure = match result {
            Ok(output) if output.code == 0 => match describe_file(&job.target, &name) {
                Ok(file) if file.bytes > 0 => return Ok(Some(file)),
                Ok(_) => process_failed(&output),
                Err(err) => ExportError::Io(err),
            },
            Ok(output) => process_failed(&output),
            Err(err) => err,
        };
        remove_own_output(&out);
        remove_own_output(&job.target.join(format!("{name}.lock")));
        let args = BTreeMap::from([("error".to_owned(), failure.to_string())]);
        failures.add_issue(ISSUE_BUNDLE_FAILED, args, failure)?;
        Ok(None)
    }

    /// Copies the files `git status` lists as changed or untracked to
    /// `<target>/<slug>-worktree/<path>`. Deleted files, folders (submodules, nested
    /// repositories) and links are skipped. Every filter of `config` is switched off
    /// and `GIT_ATTR_SOURCE` is the empty tree; a filter that cannot be switched off
    /// keeps `git status` from running.
    async fn copy_worktree(
        &self,
        job: &Job<'_>,
        config: &RepoConfig,
        files: &mut Vec<ExportedFile>,
        failures: &mut Partial,
    ) -> Result<(), ExportError> {
        // Reading a split index updates `sharedindex.*`; with `core.worktree` the paths
        // would belong to another folder.
        let refusal = if let Some(name) = &config.unpassable_filter {
            Some(format!("filter {name} cannot be switched off"))
        } else if config.split_index || gitdir::has_shared_index(&job.git_dirs) {
            Some(SPLIT_INDEX.to_owned())
        } else if config.core_worktree {
            Some(CORE_WORKTREE.to_owned())
        } else {
            None
        };
        if let Some(error) = refusal {
            let args = BTreeMap::from([("error".to_owned(), error)]);
            return failures.add_issue(ISSUE_STATUS_FAILED, args, ExportError::NotAvailable);
        }
        let args: [&OsStr; 6] = [
            "status".as_ref(),
            "--porcelain=v1".as_ref(),
            "-z".as_ref(),
            "--no-renames".as_ref(),
            "--untracked-files=all".as_ref(),
            "--ignore-submodules=all".as_ref(),
        ];
        let cmd = job
            .git(&config.filters_off(), args, STATUS_TIMEOUT)
            .env("GIT_ATTR_SOURCE", config.empty_tree());
        let result = self.runner.run(cmd, job.cancel).await;
        let output = match result {
            Ok(output) if output.code == 0 => output,
            Ok(output) => {
                return status_failed(failures, process_failed(&output));
            }
            Err(err) => return status_failed(failures, err),
        };
        if output.truncated {
            tracing::warn!("git status output cut at the size limit");
            let args = BTreeMap::from([("error".to_owned(), STATUS_TRUNCATED.to_owned())]);
            failures.note(warning(ISSUE_STATUS_FAILED, args));
        }
        let worktree = PathBuf::from(format!("{}-worktree", job.slug));
        for path in status::changed_paths(&output.stdout) {
            if job.cancel.is_cancelled() {
                return Err(ExportError::Cancelled);
            }
            let Some(rel) = status::safe_relative(&path) else {
                tracing::debug!("git status path outside the repository skipped");
                continue;
            };
            let src = job.repo.join(&rel);
            match std::fs::symlink_metadata(&src) {
                Ok(meta) if meta.is_file() => {}
                Ok(_) => {
                    tracing::debug!(path = %src.display(), "not a regular file, skipped");
                    continue;
                }
                // Deleted in the working copy: nothing to copy.
                Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
                Err(err) => {
                    failures.add_path(ISSUE_FILE_COPY_FAILED, &src, err.into())?;
                    continue;
                }
            }
            match copy_file(self.source.as_ref(), &src, &job.target, worktree.join(&rel)) {
                Ok(Some(file)) => files.push(file),
                Ok(None) => failures.cloud_only(&src),
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => failures.add_path(ISSUE_FILE_COPY_FAILED, &src, err.into())?,
            }
        }
        Ok(())
    }
}

#[async_trait]
impl SystemExporter for GitBundleExporter {
    fn id(&self) -> &'static str {
        ID
    }

    /// `git.exe` in `PATH`. Reasons: `system.git_bundle.missing`, `system.not_windows`.
    /// The scan does not depend on it: the findings come from H-GIT (SPEC-07 §4.4.4).
    fn detect(&self, _env: &Environment) -> Availability {
        if !cfg!(windows) {
            return unavailable(NOT_WINDOWS);
        }
        match self.locate() {
            Some(_) => Availability::Available {
                estimated_bytes: None,
                details: BTreeMap::new(),
            },
            None => unavailable(MISSING),
        }
    }

    /// Always `None`: the findings are the children of the repository findings of H-GIT
    /// (SPEC-07 §4.4.4), one per repository.
    fn plan(&self, _env: &Environment, _avail: &Availability) -> Option<Finding> {
        None
    }

    /// Writes `<slug>.bundle` (`git bundle create --all`) and copies the changed and
    /// untracked files (`git status --porcelain=v1 -z`) to `<slug>-worktree/`; a bare
    /// repository has only the bundle. `<slug>` is the slug of the repository folder
    /// name, made unique in the target folder (`-2`, `-3`, …), since all repositories
    /// export into the same `system/git-bundle/`.
    ///
    /// Without `git.exe`, without the repository or with invalid `params`:
    /// [`ExportError::NotAvailable`]. A failed part is a warning
    /// (`issue.system.git_bundle_failed`, `issue.system.git_status_failed`,
    /// `issue.system.file_copy_failed`, `issue.system.cloud_only`); when nothing was
    /// saved, the first error.
    ///
    /// Before git runs, the repository is checked for cloud-only entries (attributes
    /// only, no links followed): one in `.git` or in a bare repository → git does not
    /// run, a result without files and `issue.system.cloud_only` {path}; one only in the
    /// work tree → the bundle is created, `git status` does not run,
    /// `issue.system.git_worktree_cloud_only` {path}. A partial clone gets no bundle
    /// (`issue.system.git_bundle_failed` {error: "partial clone"}): it would fetch the
    /// missing objects into the repository; git never fetches lazily
    /// (`GIT_NO_LAZY_FETCH=1`).
    async fn run(
        &self,
        ctx: &ExportContext<'_>,
        params: &serde_json::Value,
    ) -> Result<ExportResult, ExportError> {
        if ctx.cancel.is_cancelled() {
            return Err(ExportError::Cancelled);
        }
        let repo = repo_path(ctx.env, params).ok_or(ExportError::NotAvailable)?;
        let git = self.locate().ok_or(ExportError::NotAvailable)?;
        let git = git.to_str().ok_or(ExportError::NotAvailable)?;
        // `git -C <repo>` resolves relative paths against the repository: the output
        // path must be absolute, or the bundle would be written into the source (P1).
        let target = std::path::absolute(ctx.target_dir)?;

        // Git reads all its folders and `git status` every file of the work tree:
        // cloud-only files would be downloaded (SPEC-10 FR-10-15).
        let worktree = has_worktree(&repo);
        let mut failures = Partial::default();
        let (git_dirs, worktree_cloud_only) =
            match gitdir::precheck(self.source.as_ref(), &repo, worktree, ctx.cancel)? {
                Precheck::Run {
                    git_dirs,
                    worktree_cloud_only,
                } => (git_dirs, worktree_cloud_only),
                Precheck::CloudOnly(path) => {
                    failures.cloud_only(&path);
                    return failures.finish(Vec::new(), RestoreHint::None);
                }
                Precheck::RemoteGitDir => {
                    let args = BTreeMap::from([("error".to_owned(), REMOTE_GITDIR.to_owned())]);
                    return Ok(ExportResult {
                        files: Vec::new(),
                        warnings: vec![warning(ISSUE_BUNDLE_FAILED, args)],
                        restore_hint: RestoreHint::None,
                    });
                }
            };

        std::fs::create_dir_all(&target)?;
        let slug = unique_slug(&repo, &target);
        let job = Job {
            git,
            repo,
            git_dirs,
            target,
            slug,
            cancel: ctx.cancel,
        };

        // Filters, partial clone and object format, read once before any other git call.
        let config = match config::read(self.runner.as_ref(), &job).await {
            Ok(config) => Ok(config),
            Err(ExportError::Cancelled) => return Err(ExportError::Cancelled),
            Err(err) => Err(err),
        };

        let mut files = Vec::new();
        if config.as_ref().is_ok_and(|c| c.partial_clone) {
            // `bundle --all` would fetch the missing objects into the repository.
            let args = BTreeMap::from([("error".to_owned(), PARTIAL_CLONE.to_owned())]);
            failures.add_issue(ISSUE_BUNDLE_FAILED, args, ExportError::NotAvailable)?;
        } else if let Some(file) = self.bundle(&job, &mut failures).await? {
            files.push(file);
        }
        if let Some(path) = worktree_cloud_only {
            tracing::debug!(path = %path.display(), "cloud-only work tree, git status not run");
            let args = BTreeMap::from([("path".to_owned(), path.to_string_lossy().into_owned())]);
            failures.note(warning(ISSUE_WORKTREE_CLOUD_ONLY, args));
        } else if worktree {
            match config {
                Ok(config) => {
                    self.copy_worktree(&job, &config, &mut files, &mut failures)
                        .await?
                }
                Err(err) => status_failed(&mut failures, err)?,
            }
        }
        failures.finish(files, restore_hint(&job.slug))
    }
}

/// One export run: git, the repository and where its files go.
struct Job<'a> {
    /// Full path of `git.exe`.
    git: &'a str,
    /// The repository folder (working copy root or bare repository).
    repo: PathBuf,
    /// Its local git folders ([`gitdir::precheck`]).
    git_dirs: Vec<PathBuf>,
    /// Absolute target folder.
    target: PathBuf,
    /// Unique slug of the repository in `target`.
    slug: String,
    cancel: &'a CancellationToken,
}

impl Job<'_> {
    /// `git <read-only options> [-c <config>]… -C <repo> <args>`: no optional locks (no
    /// index refresh is written to the repository), no file system monitor, Git LFS
    /// off, no prompts, UTF-8 output.
    fn git<'s>(
        &self,
        config: &[String],
        args: impl IntoIterator<Item = &'s OsStr>,
        timeout: Duration,
    ) -> Cmd {
        // The repository of an old Windows installation belongs to another SID.
        let repo = std::path::absolute(&self.repo).unwrap_or_else(|_| self.repo.clone());
        let safe = format!(
            "safe.directory={}",
            repo.to_string_lossy().replace('\\', "/")
        );
        let options = ["core.fsmonitor=false"]
            .into_iter()
            .chain(LFS_OFF)
            .chain([safe.as_str()])
            .chain(config.iter().map(String::as_str))
            .flat_map(|option| ["-c", option]);
        let mut cmd = Cmd::new(self.git);
        // `GIT_DIR`, `GIT_INDEX_FILE`, `GIT_CONFIG_*` of this process would redirect git.
        for key in inherited_git_vars(std::env::vars_os().map(|(key, _)| key)) {
            cmd = cmd.env_remove(key);
        }
        cmd.args(["--no-optional-locks"])
            .args(options)
            .args(["-C"])
            .args([self.repo.as_os_str()])
            .args(args)
            .timeout(timeout)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_NO_LAZY_FETCH", "1")
            .encoding(OutputEncoding::Utf8)
    }
}

/// [`ExportError::ProcessFailed`] of a finished git command.
fn process_failed(output: &CmdOutput) -> ExportError {
    ExportError::ProcessFailed {
        code: output.code,
        stderr: failure_text(output),
    }
}

/// Records the failed `git status` as [`ISSUE_STATUS_FAILED`].
fn status_failed(failures: &mut Partial, err: ExportError) -> Result<(), ExportError> {
    let args = BTreeMap::from([("error".to_owned(), err.to_string())]);
    failures.add_issue(ISSUE_STATUS_FAILED, args, err)
}
