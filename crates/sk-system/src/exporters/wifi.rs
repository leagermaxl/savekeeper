//! The `wifi` exporter (SPEC-06 §4.3): `netsh wlan export profile` of the saved Wi-Fi
//! profiles into one XML file per profile, restored with `netsh wlan add profile`
//! (SPEC-13 FR-13-11).
//!
//! Passwords are exported in clear text (`key=clear`) only with the user's consent,
//! `params.include_keys = true` (FR-06-06); otherwise every key element is removed from
//! the exported files, including the encrypted one `netsh` writes by default.

mod keys;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sk_core::env::{Environment, KnownFolder};
use sk_core::model::{Category, Finding, Sensitivity};

use super::{describe_file, failure_text, plan_finding, unavailable, warning, Plan, NOT_WINDOWS};
use crate::cmd::{run_blocking, Cmd, CmdRunner, SystemCmdRunner};
use crate::error::ExportError;
use crate::exporter::{Availability, ExportContext, ExportResult, RestoreHint, SystemExporter};
use crate::reg_export::windir;

/// Exporter identifier.
pub(crate) const ID: &str = "wifi";

/// Parameter: export the passwords in clear text (SPEC-06 §4.3, FR-06-06).
pub(crate) const INCLUDE_KEYS: &str = "include_keys";

/// Executable name of the network console tool.
pub(crate) const NETSH_EXE: &str = "netsh.exe";

/// Finding title (i18n key).
const TITLE: &str = "system.title.wifi";

/// `reason_key`: no saved profiles (SPEC-06 §5).
const NO_PROFILES: &str = "system.wifi.no_profiles";

/// Restore instructions (i18n key): `netsh wlan add profile` of each file.
const RESTORE_KEY: &str = "system.restore.wifi";

/// Warning: `netsh` wrote profiles but exited with a non-zero code. Args: `code`.
const ISSUE_EXIT_CODE: &str = "issue.system.wifi_export_exit_code";

/// Warning: with `include_keys` the key of a profile stayed encrypted (some profiles
/// need administrator rights, SPEC-06 §4.3). Args: `profile` (file name).
const ISSUE_KEY_PROTECTED: &str = "issue.system.wifi_key_protected";

/// Timeout of `netsh wlan show interfaces` in `detect` (FR-06-01: ≤ 500 ms). A timeout
/// counts as no answer, so the exporter is unavailable.
const DETECT_TIMEOUT: Duration = Duration::from_millis(500);

/// Size estimate of one exported profile.
const ESTIMATE_BYTES_PER_PROFILE: u64 = 1024;

/// Extension of the exported profiles.
const PROFILE_EXTENSION: &str = "xml";

/// `netsh wlan export profile` into XML files.
pub(crate) struct WifiExporter {
    runner: Arc<dyn CmdRunner>,
}

impl WifiExporter {
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

    /// `detect` without the platform check (SPEC-06 §4.3, §5): Available when
    /// `{PROGRAMDATA}\Microsoft\Wlansvc\Profiles\Interfaces\<GUID>\` holds `*.xml`
    /// profiles (the folders are readable by Users). `netsh wlan show interfaces` is asked
    /// only when a folder exists but cannot be read; its exit code 0 is not taken as a
    /// sign of an adapter otherwise, since `netsh` exits with 0 without one while the
    /// WLAN service runs. A missing or empty folder is `system.wifi.no_profiles`.
    pub(crate) fn probe(&self, env: &Environment) -> Availability {
        let scan = scan_profiles(env);
        if scan.profiles > 0 {
            return Availability::Available {
                estimated_bytes: Some(scan.profiles as u64 * ESTIMATE_BYTES_PER_PROFILE),
                details: BTreeMap::from([("profiles".to_owned(), scan.profiles.to_string())]),
            };
        }
        if scan.unreadable && self.wlan_service_answers(env) {
            return Availability::Available {
                estimated_bytes: None,
                details: BTreeMap::new(),
            };
        }
        unavailable(NO_PROFILES)
    }

    /// Whether `netsh wlan show interfaces` exits with 0. Only the exit code is used,
    /// the output is localized (NFR-06-02).
    fn wlan_service_answers(&self, env: &Environment) -> bool {
        let Some(exe) = netsh_exe(env) else {
            return false;
        };
        let Some(program) = exe.to_str() else {
            return false;
        };
        let cmd = Cmd::new(program)
            .args(["wlan", "show", "interfaces"])
            .timeout(DETECT_TIMEOUT);
        match run_blocking(self.runner.as_ref(), cmd) {
            Ok(output) => {
                tracing::debug!(code = output.code, "netsh wlan show interfaces");
                output.code == 0
            }
            Err(err) => {
                tracing::debug!(error = %err, "netsh wlan show interfaces did not run");
                false
            }
        }
    }
}

#[async_trait]
impl SystemExporter for WifiExporter {
    fn id(&self) -> &'static str {
        ID
    }

    /// Saved profiles, or a working WLAN service when the profile folder cannot be read
    /// (see [`WifiExporter::probe`]). Reasons:
    /// `system.wifi.no_profiles`, `system.not_windows`.
    fn detect(&self, env: &Environment) -> Availability {
        if cfg!(windows) {
            self.probe(env)
        } else {
            unavailable(NOT_WINDOWS)
        }
    }

    /// The finding is planned without keys (`include_keys: false`): the user turns them
    /// on explicitly in the UI (FR-06-06), which makes the finding [`sensitivity`] High.
    fn plan(&self, _env: &Environment, avail: &Availability) -> Option<Finding> {
        let params = default_params();
        let plan = Plan {
            exporter_id: ID,
            title: TITLE,
            category: Category::SystemSettings,
            sensitivity: sensitivity(&params),
            params,
        };
        plan_finding(plan, avail)
    }

    /// Runs `{WINDIR}\System32\netsh.exe wlan export profile folder=<target_dir>`, with
    /// `key=clear` only when `params.include_keys` is `true`.
    ///
    /// Without keys the `<sharedKey>` elements are removed from every exported file,
    /// also when the run failed or was cancelled. With keys, a profile whose key stayed
    /// encrypted gives the warning `issue.system.wifi_key_protected`. Exported profiles
    /// with a non-zero exit code are a success with a warning; no profile at all is
    /// [`ExportError::ProcessFailed`] for a non-zero exit code and
    /// [`ExportError::NotAvailable`] otherwise.
    async fn run(
        &self,
        ctx: &ExportContext<'_>,
        params: &serde_json::Value,
    ) -> Result<ExportResult, ExportError> {
        if ctx.cancel.is_cancelled() {
            return Err(ExportError::Cancelled);
        }
        let include_keys = include_keys(params);
        let exe = netsh_exe(ctx.env);
        let Some(program) = exe.as_deref().and_then(Path::to_str) else {
            return Err(ExportError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "netsh.exe not found: the Windows folder is unknown",
            )));
        };
        std::fs::create_dir_all(ctx.target_dir)?;
        let mut folder = OsString::from("folder=");
        folder.push(ctx.target_dir);
        let mut cmd = Cmd::new(program)
            .args(["wlan", "export", "profile"])
            .args([folder]);
        if include_keys {
            cmd = cmd.args(["key=clear"]);
        }
        let output = self.runner.run(cmd, ctx.cancel).await;

        let profiles = profile_files(ctx.target_dir);
        let stripped = match (&profiles, include_keys) {
            (Ok(names), false) => strip_keys(ctx.target_dir, names),
            _ => Ok(()),
        };
        let output = output?;
        let names = profiles?;
        stripped?;

        if names.is_empty() {
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
        let mut files = Vec::with_capacity(names.len());
        for name in names {
            if include_keys {
                let data = std::fs::read(ctx.target_dir.join(&name))?;
                if keys::has_protected_key(&data) {
                    let profile = name.to_string_lossy().into_owned();
                    let args = BTreeMap::from([("profile".to_owned(), profile)]);
                    warnings.push(warning(ISSUE_KEY_PROTECTED, args));
                }
            }
            files.push(describe_file(ctx.target_dir, &name)?);
        }
        Ok(ExportResult {
            files,
            warnings,
            restore_hint: RestoreHint::Manual {
                key: RESTORE_KEY.to_owned(),
            },
        })
    }
}

/// Parameters of a planned finding: no keys (FR-06-06).
pub(crate) fn default_params() -> serde_json::Value {
    serde_json::json!({ INCLUDE_KEYS: false })
}

/// Whether `params` ask for the passwords in clear text: only a boolean `true` does.
pub(crate) fn include_keys(params: &serde_json::Value) -> bool {
    params
        .get(INCLUDE_KEYS)
        .and_then(serde_json::Value::as_bool)
        == Some(true)
}

/// Sensitivity of the finding with `params`: High with keys (FR-06-06), else none.
pub(crate) fn sensitivity(params: &serde_json::Value) -> Sensitivity {
    if include_keys(params) {
        Sensitivity::High
    } else {
        Sensitivity::None
    }
}

/// `{WINDIR}\System32\netsh.exe` (SPEC-06 §4.6 item 3: full path, no `PATH` lookup).
fn netsh_exe(env: &Environment) -> Option<PathBuf> {
    windir(env).map(|dir| dir.join("System32").join(NETSH_EXE))
}

/// What the profile folders show.
#[derive(Debug, Default, PartialEq, Eq)]
struct ProfileScan {
    /// `*.xml` files in the interface folders.
    profiles: usize,
    /// A folder exists but cannot be read (an error other than `NotFound`).
    unreadable: bool,
}

/// Saved profiles: `*.xml` files in the interface folders of
/// `{PROGRAMDATA}\Microsoft\Wlansvc\Profiles\Interfaces`. A missing folder holds none.
fn scan_profiles(env: &Environment) -> ProfileScan {
    let mut scan = ProfileScan::default();
    let Some(program_data) = env.known_folder(KnownFolder::ProgramData) else {
        return scan;
    };
    let interfaces = program_data.join(r"Microsoft\Wlansvc\Profiles\Interfaces");
    let entries = match std::fs::read_dir(&interfaces) {
        Ok(entries) => entries,
        Err(err) => {
            scan.unreadable = is_unreadable(&err);
            return scan;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                scan.unreadable |= is_unreadable(&err);
                continue;
            }
        };
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        match std::fs::read_dir(entry.path()) {
            Ok(files) => {
                scan.profiles += files
                    .flatten()
                    .filter(|file| is_profile(&file.path()))
                    .count();
            }
            Err(err) => scan.unreadable |= is_unreadable(&err),
        }
    }
    scan
}

/// Whether `err` of `read_dir` means an existing folder that cannot be read.
fn is_unreadable(err: &std::io::Error) -> bool {
    tracing::debug!(error = %err, "Wi-Fi profile folder not read");
    err.kind() != std::io::ErrorKind::NotFound
}

/// Whether `path` is a profile file (`*.xml`, any case).
fn is_profile(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case(PROFILE_EXTENSION))
}

/// Names of the profile files in `dir`, sorted. File names come from the interface name
/// and the SSID (`Wi-Fi-<SSID>.xml`; `netsh` replaces invalid characters itself).
fn profile_files(dir: &Path) -> std::io::Result<Vec<OsString>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if is_profile(&entry.path()) {
            names.push(entry.file_name());
        }
    }
    names.sort();
    Ok(names)
}

/// Removes the keys from every profile file `names` in `dir`. A file that fails does not
/// stop the others; the first error is returned at the end.
fn strip_keys(dir: &Path, names: &[OsString]) -> std::io::Result<()> {
    let mut first_error = None;
    for name in names {
        match keys::strip_file(&dir.join(name)) {
            Ok(true) => tracing::debug!("Wi-Fi key removed from an exported profile"),
            Ok(false) => {}
            Err(err) => {
                tracing::warn!(error = %err, "Wi-Fi key not removed from an exported profile");
                first_error.get_or_insert(err);
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}
