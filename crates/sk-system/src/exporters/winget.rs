//! The `winget` exporter (SPEC-06 §4.3): `winget export` of the installed packages into
//! `winget.json`, restored with `winget import` (SPEC-13).

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sk_core::env::{Environment, KnownFolder};
use sk_core::model::{Category, Finding, Sensitivity};

use super::{describe_file, plan_finding, unavailable, warning, Plan, NOT_WINDOWS};
use crate::cmd::{run_blocking, Cmd, CmdOutput, CmdRunner, OutputEncoding, SystemCmdRunner};
use crate::error::ExportError;
use crate::exporter::{Availability, ExportContext, ExportResult, RestoreHint, SystemExporter};

/// Exporter identifier.
pub(crate) const ID: &str = "winget";

/// The export file inside the target folder (SPEC-06 §4.3).
pub(crate) const EXPORT_FILE: &str = "winget.json";

/// Finding title (i18n key).
const TITLE: &str = "system.title.winget";

/// `reason_key`: winget is not installed or does not run (SPEC-06 §5).
const MISSING: &str = "system.winget.missing";

/// `reason_key`: winget is older than [`MIN_VERSION`].
const OUTDATED: &str = "system.winget.outdated";

/// Warning: `winget export` wrote the file but exited with a non-zero code.
const ISSUE_EXIT_CODE: &str = "issue.system.winget_export_exit_code";

/// Executable name.
const EXE: &str = "winget.exe";

/// Oldest supported winget, `major.minor` (SPEC-06 §4.3).
const MIN_VERSION: (u32, u32) = (1, 4);

/// Timeout of `winget --version` in `detect` (FR-06-01 asks for ≤ 500 ms, but the first
/// start of the App Installer alias can take longer).
const VERSION_TIMEOUT: Duration = Duration::from_secs(2);

/// Default timeout of `winget export` (SPEC-06 §4.3, `system.winget_timeout_s`).
const EXPORT_TIMEOUT: Duration = Duration::from_secs(180);

/// Size estimate of `winget.json`: fixed part plus a part per installed program.
const ESTIMATE_BASE_BYTES: u64 = 512;
const ESTIMATE_BYTES_PER_PROGRAM: u64 = 120;

/// Longest process output kept in [`ExportError::ProcessFailed`].
const FAILURE_TEXT_CHARS: usize = 2000;

/// `winget export` into `winget.json`.
pub(crate) struct WingetExporter {
    runner: Arc<dyn CmdRunner>,
    timeout: Duration,
    /// `PATH` to search for `winget.exe`; `None` means the `PATH` of this process.
    path_var: Option<OsString>,
}

impl WingetExporter {
    /// The exporter with real processes and the default timeout.
    pub(crate) fn new() -> Self {
        Self {
            runner: Arc::new(SystemCmdRunner),
            timeout: EXPORT_TIMEOUT,
            path_var: None,
        }
    }

    /// The exporter with `runner` and the given `PATH` (tests).
    #[cfg(test)]
    pub(crate) fn with_runner(runner: Arc<dyn CmdRunner>, path_var: &str) -> Self {
        Self {
            runner,
            timeout: EXPORT_TIMEOUT,
            path_var: Some(path_var.into()),
        }
    }

    /// Full path of `winget.exe` (SPEC-06 §4.6 item 3: no lookup by the OS).
    fn locate(&self, env: &Environment) -> Option<PathBuf> {
        let process_path;
        let path_var = match &self.path_var {
            Some(path_var) => Some(path_var.as_os_str()),
            None => {
                process_path = std::env::var_os("PATH");
                process_path.as_deref()
            }
        };
        locate(env, path_var)
    }

    /// `detect` without the platform check: locate winget and check its version.
    pub(crate) fn probe(&self, env: &Environment) -> Availability {
        let Some(exe) = self.locate(env) else {
            return unavailable(MISSING);
        };
        let Some(program) = exe.to_str() else {
            tracing::debug!("winget path is not valid Unicode");
            return unavailable(MISSING);
        };
        let cmd = Cmd::new(program)
            .args(["--version"])
            .timeout(VERSION_TIMEOUT)
            .encoding(OutputEncoding::Utf8);
        let output = match run_blocking(self.runner.as_ref(), cmd) {
            Ok(output) if output.code == 0 => output,
            Ok(output) => {
                tracing::debug!(code = output.code, "winget --version failed");
                return unavailable(MISSING);
            }
            Err(err) => {
                tracing::debug!(error = %err, "winget --version did not run");
                return unavailable(MISSING);
            }
        };
        let Some((text, version)) = parse_version(&output.stdout) else {
            tracing::debug!("winget --version output is not a version");
            return unavailable(MISSING);
        };
        if version < MIN_VERSION {
            return unavailable(OUTDATED);
        }
        let programs = env.installed_programs.len() as u64;
        Availability::Available {
            estimated_bytes: Some(ESTIMATE_BASE_BYTES + programs * ESTIMATE_BYTES_PER_PROGRAM),
            details: BTreeMap::from([("version".to_owned(), text)]),
        }
    }
}

#[async_trait]
impl SystemExporter for WingetExporter {
    fn id(&self) -> &'static str {
        ID
    }

    /// `winget.exe` in `{LOCALAPPDATA}\Microsoft\WindowsApps` or `PATH`, and
    /// `winget --version` ≥ 1.4. Reasons: `system.winget.missing`,
    /// `system.winget.outdated`, `system.not_windows`.
    fn detect(&self, env: &Environment) -> Availability {
        if cfg!(windows) {
            self.probe(env)
        } else {
            unavailable(NOT_WINDOWS)
        }
    }

    fn plan(&self, _env: &Environment, avail: &Availability) -> Option<Finding> {
        let plan = Plan {
            exporter_id: ID,
            title: TITLE,
            category: Category::SystemSettings,
            sensitivity: Sensitivity::None,
            params: serde_json::json!({}),
        };
        plan_finding(plan, avail)
    }

    /// Runs `winget export -o <target_dir>\winget.json --include-versions
    /// --accept-source-agreements --disable-interactivity`.
    ///
    /// A written, non-empty `winget.json` is a success, even with a non-zero exit code
    /// (then with a warning): winget skips packages that are not available from any
    /// source (SPEC-06 §5). Without the file the result is
    /// [`ExportError::ProcessFailed`]; winget not found is [`ExportError::NotAvailable`].
    async fn run(
        &self,
        ctx: &ExportContext<'_>,
        _params: &serde_json::Value,
    ) -> Result<ExportResult, ExportError> {
        let exe = self.locate(ctx.env).ok_or(ExportError::NotAvailable)?;
        let program = exe.to_str().ok_or(ExportError::NotAvailable)?;
        std::fs::create_dir_all(ctx.target_dir)?;
        let out_file = ctx.target_dir.join(EXPORT_FILE);
        let args: [&OsStr; 6] = [
            "export".as_ref(),
            "-o".as_ref(),
            out_file.as_os_str(),
            "--include-versions".as_ref(),
            "--accept-source-agreements".as_ref(),
            "--disable-interactivity".as_ref(),
        ];
        let cmd = Cmd::new(program)
            .args(args)
            .timeout(self.timeout)
            .encoding(OutputEncoding::Utf8);
        let output = self.runner.run(cmd, ctx.cancel).await?;

        let file = describe_file(ctx.target_dir, EXPORT_FILE)
            .ok()
            .filter(|file| file.bytes > 0);
        let Some(file) = file else {
            return Err(ExportError::ProcessFailed {
                code: output.code,
                stderr: failure_text(&output),
            });
        };
        let mut warnings = Vec::new();
        if output.code != 0 {
            let args = BTreeMap::from([("code".to_owned(), output.code.to_string())]);
            warnings.push(warning(ISSUE_EXIT_CODE, args));
        }
        Ok(ExportResult {
            files: vec![file],
            warnings,
            restore_hint: restore_hint(),
        })
    }
}

/// `winget import` of the export (SPEC-13 FR-13-10); the file path is relative to the
/// export folder.
fn restore_hint() -> RestoreHint {
    RestoreHint::Command {
        program: "winget".to_owned(),
        args: [
            "import",
            "--import-file",
            EXPORT_FILE,
            "--accept-package-agreements",
            "--accept-source-agreements",
            "--disable-interactivity",
        ]
        .map(str::to_owned)
        .to_vec(),
    }
}

/// The first `winget.exe` in `{LOCALAPPDATA}\Microsoft\WindowsApps` and then in the
/// absolute entries of `path_var`.
fn locate(env: &Environment, path_var: Option<&OsStr>) -> Option<PathBuf> {
    let windows_apps = env
        .known_folder(KnownFolder::LocalAppData)
        .map(|dir| dir.join("Microsoft").join("WindowsApps"));
    let path_dirs = path_var
        .into_iter()
        .flat_map(std::env::split_paths)
        .filter(|dir| dir.is_absolute());
    windows_apps
        .into_iter()
        .chain(path_dirs)
        .map(|dir| dir.join(EXE))
        .find(|exe| is_file(exe))
}

/// Whether `path` is a file. The App Installer alias is a reparse point that cannot be
/// followed, so its own metadata is checked.
fn is_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| !meta.is_dir())
}

/// Version from `winget --version` output (`v1.9.25200`): the text without the leading
/// `v` and its `major.minor`.
fn parse_version(stdout: &str) -> Option<(String, (u32, u32))> {
    let line = stdout
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let text = line.strip_prefix(['v', 'V']).unwrap_or(line);
    let mut parts = text.split('.').map(leading_number);
    let major = parts.next()??;
    let minor = parts.next()??;
    Some((text.to_owned(), (major, minor)))
}

/// The number at the start of `part` (`"10-preview"` → 10).
fn leading_number(part: &str) -> Option<u32> {
    let end = part
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(part.len());
    part[..end].parse().ok()
}

/// What winget said about a failure: stderr, else stdout (winget reports errors there),
/// cut to [`FAILURE_TEXT_CHARS`].
fn failure_text(output: &CmdOutput) -> String {
    let text = match output.stderr.trim() {
        "" => output.stdout.trim(),
        stderr => stderr,
    };
    text.chars().take(FAILURE_TEXT_CHARS).collect()
}

#[cfg(test)]
mod tests;
