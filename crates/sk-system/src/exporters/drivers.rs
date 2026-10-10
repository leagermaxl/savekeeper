//! The `drivers` exporter (SPEC-06 §4.3): `pnputil /export-driver *` of the third-party
//! driver packages of the driver store, restored with `pnputil /add-driver` (SPEC-13
//! FR-13-12).
//!
//! The export needs administrator rights (FR-06-03): without them `detect` returns
//! [`Availability::NeedsElevation`] and `run` returns [`ExportError::NeedsElevation`]
//! without starting `pnputil`; the elevated run goes through the helper of SPEC-14
//! (T-06-12).

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sk_core::env::Environment;
use sk_core::model::{Category, Finding, Sensitivity};

use super::{describe_file, failure_text, plan_finding, unavailable, warning, Plan, NOT_WINDOWS};
use crate::cmd::{run_blocking, Cmd, CmdRunner, SystemCmdRunner};
use crate::error::ExportError;
use crate::exporter::{
    Availability, ExportContext, ExportResult, ExportedFile, RestoreHint, SystemExporter,
};
use crate::reg_export::windir;

/// Exporter identifier.
pub(crate) const ID: &str = "drivers";

/// Parameter: export only third-party drivers (SPEC-06 §4.3). `pnputil /export-driver *`
/// exports only those (`oem*.inf`), inbox drivers come with Windows.
pub(crate) const ONLY_THIRD_PARTY: &str = "only_third_party";

/// Executable name of the driver store console tool.
pub(crate) const PNPUTIL_EXE: &str = "pnputil.exe";

/// Finding title (i18n key).
const TITLE: &str = "system.title.drivers";

/// Warning: `pnputil` exported drivers but exited with a non-zero code. Args: `code`.
pub(crate) const ISSUE_EXIT_CODE: &str = "issue.system.drivers_export_exit_code";

/// Timeout of `pnputil /enum-drivers` in `detect` (FR-06-01: ≤ 500 ms; it takes about
/// 250 ms with 125 packages). A timeout leaves the size unknown.
const DETECT_TIMEOUT: Duration = Duration::from_millis(500);

/// Timeout of the export (FR-06-04).
const EXPORT_TIMEOUT: Duration = Duration::from_secs(600);

/// Size estimate of one exported driver package (SPEC-06 §4.3: ~5 MB).
const ESTIMATE_BYTES_PER_DRIVER: u64 = 5 * 1024 * 1024;

/// `pnputil /export-driver *` into the target folder.
pub(crate) struct DriversExporter {
    runner: Arc<dyn CmdRunner>,
}

impl DriversExporter {
    /// The exporter with real processes.
    pub(crate) fn new() -> Self {
        Self {
            runner: Arc::new(SystemCmdRunner),
        }
    }

    /// The exporter with the given process runner (tests).
    #[cfg(test)]
    pub(crate) fn with_runner(runner: Arc<dyn CmdRunner>) -> Self {
        Self { runner }
    }

    /// `detect` without the platform check: [`Availability::NeedsElevation`], or
    /// Available when the process already has administrator rights. The size is the
    /// number of third-party packages that `pnputil /enum-drivers` lists (500 ms
    /// timeout) × 5 MB; when the listing fails it is unknown.
    pub(crate) fn probe(&self, env: &Environment) -> Availability {
        let packages = self.count_packages(env);
        let estimated_bytes = packages.map(|n| n as u64 * ESTIMATE_BYTES_PER_DRIVER);
        if env.is_elevated {
            let details = packages
                .map(|n| BTreeMap::from([("drivers".to_owned(), n.to_string())]))
                .unwrap_or_default();
            Availability::Available {
                estimated_bytes,
                details,
            }
        } else {
            Availability::NeedsElevation { estimated_bytes }
        }
    }

    /// The number of third-party driver packages, `None` when `pnputil` fails.
    fn count_packages(&self, env: &Environment) -> Option<usize> {
        let program = pnputil_exe(env)?;
        let cmd = Cmd::new(program.to_str()?)
            .args(["/enum-drivers"])
            .timeout(DETECT_TIMEOUT);
        match run_blocking(self.runner.as_ref(), cmd) {
            Ok(output) if output.code == 0 => Some(published_names(&output.stdout).len()),
            Ok(output) => {
                tracing::debug!(code = output.code, "pnputil /enum-drivers failed");
                None
            }
            Err(err) => {
                tracing::debug!(error = %err, "pnputil /enum-drivers did not run");
                None
            }
        }
    }
}

#[async_trait]
impl SystemExporter for DriversExporter {
    fn id(&self) -> &'static str {
        ID
    }

    /// See [`DriversExporter::probe`]; `system.not_windows` outside Windows.
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
            params: default_params(),
        };
        plan_finding(plan, avail)
    }

    /// Runs `{WINDIR}\System32\pnputil.exe /export-driver * <target_dir>` (timeout 600 s)
    /// and returns every file it wrote (one subfolder per package).
    ///
    /// Without administrator rights (`ctx.elevated` or `env.is_elevated`) it is
    /// [`ExportError::NeedsElevation`] and nothing is started. Exported files with a
    /// non-zero exit code are a success with the warning
    /// `issue.system.drivers_export_exit_code`; no file at all is
    /// [`ExportError::ProcessFailed`] for a non-zero exit code and
    /// [`ExportError::NotAvailable`] otherwise. On cancellation or timeout the files
    /// written so far stay; the backup marks them incomplete (SPEC-06 §5, SPEC-10).
    async fn run(
        &self,
        ctx: &ExportContext<'_>,
        _params: &serde_json::Value,
    ) -> Result<ExportResult, ExportError> {
        if ctx.cancel.is_cancelled() {
            return Err(ExportError::Cancelled);
        }
        if !(ctx.elevated || ctx.env.is_elevated) {
            return Err(ExportError::NeedsElevation);
        }
        let exe = pnputil_exe(ctx.env);
        let Some(program) = exe.as_deref().and_then(Path::to_str) else {
            return Err(ExportError::Io(io::Error::new(
                io::ErrorKind::NotFound,
                "pnputil.exe not found: the Windows folder is unknown",
            )));
        };
        std::fs::create_dir_all(ctx.target_dir)?;
        let cmd = Cmd::new(program)
            .args(["/export-driver", "*"])
            .args([OsString::from(ctx.target_dir)])
            .timeout(EXPORT_TIMEOUT);
        let output = self.runner.run(cmd, ctx.cancel).await?;

        let files = exported_files(ctx.target_dir)?;
        if files.is_empty() {
            return Err(if output.code == 0 {
                ExportError::NotAvailable
            } else {
                ExportError::ProcessFailed {
                    code: output.code,
                    stderr: failure_text(&output),
                }
            });
        }
        let mut warnings = Vec::new();
        if output.code != 0 {
            let args = BTreeMap::from([("code".to_owned(), output.code.to_string())]);
            warnings.push(warning(ISSUE_EXIT_CODE, args));
        }
        Ok(ExportResult {
            files,
            warnings,
            restore_hint: restore_hint(),
        })
    }
}

/// Parameters of a planned finding (SPEC-06 §4.3).
pub(crate) fn default_params() -> serde_json::Value {
    serde_json::json!({ ONLY_THIRD_PARTY: true })
}

/// `pnputil /add-driver *.inf /subdirs /install` in the export folder (SPEC-13
/// FR-13-12, through the elevation helper).
fn restore_hint() -> RestoreHint {
    RestoreHint::Command {
        program: "pnputil".to_owned(),
        args: ["/add-driver", "*.inf", "/subdirs", "/install"]
            .map(str::to_owned)
            .to_vec(),
    }
}

/// `{WINDIR}\System32\pnputil.exe` (SPEC-06 §4.6 item 3: full path, no `PATH` lookup).
fn pnputil_exe(env: &Environment) -> Option<PathBuf> {
    windir(env).map(|dir| dir.join("System32").join(PNPUTIL_EXE))
}

/// The published names (`oem<N>.inf`, lowercase, without duplicates) in the output of
/// `pnputil /enum-drivers`. The labels of the output are localized, so only the
/// `oem<N>.inf` words are taken (NFR-06-02).
pub(crate) fn published_names(output: &str) -> BTreeSet<String> {
    output
        .split(|c: char| c.is_whitespace() || c == ':')
        .filter(|word| is_published_name(word))
        .map(str::to_ascii_lowercase)
        .collect()
}

/// Whether `word` is `oem<digits>.inf`, any case.
fn is_published_name(word: &str) -> bool {
    let lower = word.to_ascii_lowercase();
    lower
        .strip_prefix("oem")
        .and_then(|rest| rest.strip_suffix(".inf"))
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

/// Every file under `dir`, relative to it, sorted; links are not followed.
fn exported_files(dir: &Path) -> io::Result<Vec<ExportedFile>> {
    let mut rel_paths = Vec::new();
    collect_files(dir, Path::new(""), &mut rel_paths)?;
    rel_paths.sort();
    rel_paths
        .into_iter()
        .map(|rel_path| describe_file(dir, rel_path))
        .collect()
}

fn collect_files(root: &Path, rel: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in std::fs::read_dir(root.join(rel))? {
        let entry = entry?;
        let rel_path = rel.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_files(root, &rel_path, out)?;
        } else if file_type.is_file() {
            out.push(rel_path);
        }
    }
    Ok(())
}
