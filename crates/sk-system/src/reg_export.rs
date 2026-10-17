//! Export of registry branches to `.reg` files with `reg.exe export` (SPEC-06 §4.5).
//!
//! Used by the `registry-user` and `env-vars` exporters and, through [`export_registry`],
//! by the backup for `Target::Registry` findings (SPEC-10). The registry is only read
//! (principle P1): `reg export` reads the branch and writes the given file. When
//! `reg.exe` cannot be used (blocked by policy, missing, not allowed to start), the own
//! serializer of [`reg_file`] writes the same format (SPEC-06 §5, T-06-10).

mod reg_file;
#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sk_core::env::{Environment, KnownFolder};
use sk_core::model::RegHive;
use sk_core::registry::{normalize_key, KeyState, RegistryReader, SystemRegistry};
use sk_core::CancellationToken;

use crate::cmd::{Cmd, CmdRunner, SystemCmdRunner};
use crate::error::ExportError;
use crate::exporter::ExportedFile;
use crate::exporters::{describe_file, failure_text, write_file};

/// Executable name of the registry console tool.
pub(crate) const REG_EXE: &str = "reg.exe";

/// Key of the group policy value [`DISABLE_REGISTRY_TOOLS`] (under `HKCU` and `HKLM`).
const POLICY_SYSTEM_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Policies\System";

/// Policy "Prevent access to registry editing tools": `reg.exe` refuses to run when the
/// value is not 0.
const DISABLE_REGISTRY_TOOLS: &str = "DisableRegistryTools";

/// Exports the registry branch `hive\key` (with its subkeys) to `out_file` with
/// `{WINDIR}\System32\reg.exe export "<HIVE>\<key>" "<out_file>" /y` (SPEC-06 §4.5).
///
/// The file is in the standard `.reg` format (UTF-16LE with BOM) and is restored by a
/// double click. Only `out_file` is written (an existing file is replaced); its folder is
/// created if missing. `ExportedFile::rel_path` is the file name of `out_file`, i.e.
/// relative to its folder.
///
/// Fallback (SPEC-06 §5, T-06-10): when `reg.exe` cannot be used, the branch is read
/// through the registry reader (`values`, `subkeys`, SPEC-02 §3.4) and written by an own
/// serializer in the same format (`Windows Registry Editor Version 5.00`, accepted by
/// `reg import`). This happens when the policy "Prevent access to registry editing tools"
/// (`DisableRegistryTools` ≠ 0 in `HKCU` or `HKLM`
/// `Software\Microsoft\Windows\CurrentVersion\Policies\System`) is set, so `reg.exe` is
/// not started; when the Windows folder is unknown; and when `reg.exe` cannot be started
/// (missing, blocked by software restriction policies or AppLocker). A non-zero exit
/// code of a started `reg.exe` is not a reason for the fallback. Subkeys the serializer
/// cannot read are left out (logged).
///
/// Errors: [`ExportError::NotAvailable`] when the key does not exist (a warning for the
/// backup, not an error); [`ExportError::ProcessFailed`] when `reg.exe` fails otherwise;
/// [`ExportError::Timeout`] / [`ExportError::Cancelled`] from the process or the
/// serializer; [`ExportError::Io`] when the file cannot be written or read back, or the
/// serializer cannot read the key itself (`PermissionDenied`). Outside Windows every key
/// is missing, so the result is `NotAvailable`.
pub async fn export_registry(
    hive: RegHive,
    key: &str,
    out_file: &Path,
    cancel: &CancellationToken,
) -> Result<ExportedFile, ExportError> {
    let windir = std::env::var_os("SystemRoot")
        .or_else(|| std::env::var_os("windir"))
        .map(PathBuf::from);
    RegExport::system()
        .export(windir.as_deref(), hive, key, out_file, cancel)
        .await
}

/// `reg export` with replaceable process runner and registry (tests).
#[derive(Clone)]
pub(crate) struct RegExport {
    runner: Arc<dyn CmdRunner>,
    registry: Arc<dyn RegistryReader>,
}

impl RegExport {
    /// Real processes and the registry of this machine.
    pub(crate) fn system() -> Self {
        Self {
            runner: Arc::new(SystemCmdRunner),
            registry: Arc::new(SystemRegistry),
        }
    }

    /// The given runner and registry (tests).
    #[cfg(test)]
    pub(crate) fn new(runner: Arc<dyn CmdRunner>, registry: Arc<dyn RegistryReader>) -> Self {
        Self { runner, registry }
    }

    /// Exports `hive\key` to `out_file` with `reg.exe` from `windir`, or with the own
    /// serializer when `reg.exe` cannot be used; see [`export_registry`] for the
    /// behaviour and errors.
    pub(crate) async fn export(
        &self,
        windir: Option<&Path>,
        hive: RegHive,
        key: &str,
        out_file: &Path,
        cancel: &CancellationToken,
    ) -> Result<ExportedFile, ExportError> {
        if cancel.is_cancelled() {
            return Err(ExportError::Cancelled);
        }
        let key = normalize_key(key);
        if self.registry.key_state(hive, &key) == KeyState::Missing {
            return Err(ExportError::NotAvailable);
        }
        let (Some(dir), Some(name)) = (out_file.parent(), out_file.file_name()) else {
            return Err(ExportError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "registry export file has no name",
            )));
        };
        std::fs::create_dir_all(dir)?;

        let branch = full_key(hive, &key);
        if self.registry_tools_disabled() {
            tracing::warn!(key = %branch, "registry tools disabled by policy: own .reg serializer used");
            return self.serialize(hive, &key, dir, name, cancel).await;
        }
        let reg_exe = windir.map(reg_exe);
        let Some(program) = reg_exe.as_deref().and_then(Path::to_str) else {
            tracing::warn!(key = %branch, "Windows folder unknown, no reg.exe: own .reg serializer used");
            return self.serialize(hive, &key, dir, name, cancel).await;
        };
        let args: [&OsStr; 4] = [
            "export".as_ref(),
            branch.as_ref(),
            out_file.as_os_str(),
            "/y".as_ref(),
        ];
        let cmd = Cmd::new(program).args(args);
        let output = match self.runner.run(cmd, cancel).await {
            Err(ExportError::Io(err)) => {
                // Missing, or not allowed to start (software restriction, AppLocker).
                tracing::warn!(key = %branch, error = %err, "reg.exe cannot be started: own .reg serializer used");
                return self.serialize(hive, &key, dir, name, cancel).await;
            }
            result => result?,
        };
        if output.code != 0 {
            // The key may have disappeared since the check above.
            if self.registry.key_state(hive, &key) == KeyState::Missing {
                return Err(ExportError::NotAvailable);
            }
            return Err(ExportError::ProcessFailed {
                code: output.code,
                stderr: failure_text(&output),
            });
        }
        Ok(describe_file(dir, Path::new(name))?)
    }

    /// Whether the policy "Prevent access to registry editing tools" is set
    /// ([`DISABLE_REGISTRY_TOOLS`] ≠ 0 under [`POLICY_SYSTEM_KEY`] of `HKCU` or `HKLM`).
    fn registry_tools_disabled(&self) -> bool {
        [RegHive::Hkcu, RegHive::Hklm].into_iter().any(|hive| {
            self.registry
                .dword_value(hive, POLICY_SYSTEM_KEY, DISABLE_REGISTRY_TOOLS)
                .is_some_and(|value| value != 0)
        })
    }

    /// Writes `hive\key` with the own serializer ([`reg_file`]) to `dir/name`, in a
    /// blocking task. Nothing is written when the branch cannot be read or the export is
    /// cancelled; a file that fails while being written is removed again.
    async fn serialize(
        &self,
        hive: RegHive,
        key: &str,
        dir: &Path,
        name: &OsStr,
        cancel: &CancellationToken,
    ) -> Result<ExportedFile, ExportError> {
        let registry = Arc::clone(&self.registry);
        let key = key.to_owned();
        let dir = dir.to_path_buf();
        let name = PathBuf::from(name);
        let cancel = cancel.clone();
        tokio::task::spawn_blocking(move || {
            let text = reg_file::serialize_branch(registry.as_ref(), hive, &key, &cancel)?;
            Ok(write_file(&dir, &name, &reg_file::encode_reg_file(&text))?)
        })
        .await
        .map_err(|err| ExportError::Io(std::io::Error::other(err.to_string())))?
    }
}

/// `{WINDIR}\System32\reg.exe` (SPEC-06 §4.6 item 3: full path, no `PATH` lookup).
pub(crate) fn reg_exe(windir: &Path) -> PathBuf {
    windir.join("System32").join(REG_EXE)
}

/// The Windows folder of `env`, where `reg.exe` is looked up.
pub(crate) fn windir(env: &Environment) -> Option<&Path> {
    env.known_folder(KnownFolder::WinDir)
}

/// `HKCU\<key>` / `HKLM\<key>`: the branch as `reg.exe` and the warnings name it.
pub(crate) fn full_key(hive: RegHive, key: &str) -> String {
    let root = match hive {
        RegHive::Hkcu => "HKCU",
        RegHive::Hklm => "HKLM",
    };
    let key = normalize_key(key);
    if key.is_empty() {
        root.to_owned()
    } else {
        format!("{root}\\{key}")
    }
}

/// File name `slug(key).reg` that is not in `taken` yet; collisions get the suffix
/// `-2`, `-3`, ... (SPEC-06 §4.5). The name is added to `taken`.
///
/// The slug is the key lowercased, with every run of non-alphanumeric characters
/// (including `\`) replaced by `-`; an empty slug becomes `key`.
pub(crate) fn reg_file_name(key: &str, taken: &mut BTreeSet<String>) -> String {
    let base = match slug(key) {
        slug if slug.is_empty() => "key".to_owned(),
        slug => slug,
    };
    let mut name = format!("{base}.reg");
    let mut n = 2u32;
    while taken.contains(&name) {
        name = format!("{base}-{n}.reg");
        n += 1;
    }
    taken.insert(name.clone());
    name
}

/// Lowercase alphanumeric words of `key` joined by `-`.
pub(crate) fn slug(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    for c in key.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_owned()
}
