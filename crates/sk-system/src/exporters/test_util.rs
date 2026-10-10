//! Helpers of the exporter tests.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sk_core::config::SystemConfig;
use sk_core::env::Environment;
use sk_core::model::{AppKind, Category, EvidenceSource, FindingId, Sensitivity, Target};
use sk_core::CancellationToken;
use tempfile::TempDir;

use crate::cmd::{Cmd, CmdOutput, FakeCmdRunner};
use crate::reg_export::REG_EXE;
use crate::{Availability, ExportContext, ExportError, ExportResult, SystemExporter};

/// A fresh folder under `<workspace>/target/tmp` (the same place as
/// `CARGO_TARGET_TMPDIR`), not in the system temp folder.
pub(crate) fn temp_dir() -> TempDir {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
    std::fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new()
        .prefix("sk-system-")
        .tempdir_in(base)
        .unwrap()
}

/// Runs `exporter` into `target_dir` with empty parameters and the default `system`
/// configuration.
pub(crate) async fn run_export(
    exporter: &dyn SystemExporter,
    env: &Environment,
    target_dir: &Path,
    cancel: &CancellationToken,
) -> Result<ExportResult, ExportError> {
    run_export_with_config(exporter, env, target_dir, cancel, &SystemConfig::default()).await
}

/// Runs `exporter` into `target_dir` with empty parameters and the `system`
/// configuration `config`.
pub(crate) async fn run_export_with_config(
    exporter: &dyn SystemExporter,
    env: &Environment,
    target_dir: &Path,
    cancel: &CancellationToken,
    config: &SystemConfig,
) -> Result<ExportResult, ExportError> {
    let (events, _rx) = tokio::sync::mpsc::unbounded_channel();
    let ctx = ExportContext {
        env,
        target_dir,
        cancel,
        events: &events,
        elevated: false,
        config,
    };
    exporter.run(&ctx, &serde_json::json!({})).await
}

/// All files under `dir`, relative to it, sorted (principle P1 checks).
pub(crate) fn files_under(dir: &Path) -> Vec<PathBuf> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                out.push(path.strip_prefix(root).unwrap().to_path_buf());
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// The `.reg` text `reg.exe export` writes for `branch` (`HKEY_CURRENT_USER\...`):
/// UTF-16LE with BOM.
pub(crate) fn reg_file_content(branch: &str) -> Vec<u8> {
    let text = format!(
        "Windows Registry Editor Version 5.00



[{branch}]



"
    );
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    bytes
}

/// A fake `reg.exe export <branch> <file> /y` that writes [`reg_file_content`] of the
/// branch to the file and exits with 0.
pub(crate) fn reg_runner() -> FakeCmdRunner {
    FakeCmdRunner::new().on(REG_EXE, fake_reg_export)
}

/// What the fake `reg.exe export` does: writes [`reg_file_content`] of the branch
/// (second argument) to the file (third argument), exit code 0.
pub(crate) fn fake_reg_export(cmd: &Cmd) -> Result<CmdOutput, ExportError> {
    let args = cmd.get_args();
    let branch = args[1]
        .to_string_lossy()
        .replacen("HKCU", "HKEY_CURRENT_USER", 1);
    std::fs::write(&args[2], reg_file_content(&branch)).unwrap();
    Ok(CmdOutput::default())
}

/// Checks `plan` of an exporter without parameters: a `SystemExport` finding of category
/// `SystemSettings` with `title` and `sensitivity`, `requires_elevation` only for
/// `NeedsElevation`, none for `Unavailable`.
pub(crate) fn assert_system_settings_plan(
    exporter: &dyn SystemExporter,
    title: &str,
    sensitivity: Sensitivity,
) {
    let env = Environment::fake(Path::new("/fake"));
    let id = exporter.id();
    let avail = Availability::Available {
        estimated_bytes: None,
        details: BTreeMap::new(),
    };
    let finding = exporter.plan(&env, &avail).unwrap();
    let target = Target::SystemExport {
        exporter_id: id.to_owned(),
        params: serde_json::json!({}),
    };
    assert_eq!(finding.id, FindingId::for_target(&target));
    assert_eq!(finding.target, target);
    assert_eq!(finding.category, Category::SystemSettings);
    assert_eq!(finding.sensitivity, sensitivity);
    assert_eq!(finding.title, title);
    assert!(!finding.requires_elevation);
    assert_eq!(finding.app.as_ref().map(|a| a.kind), Some(AppKind::System));
    assert_eq!(
        finding.evidence[0].source,
        EvidenceSource::System {
            exporter_id: id.to_owned()
        }
    );

    let elevated = Availability::NeedsElevation {
        estimated_bytes: None,
    };
    assert!(exporter.plan(&env, &elevated).unwrap().requires_elevation);
    let unavailable = Availability::Unavailable {
        reason_key: "system.not_windows".to_owned(),
    };
    assert!(exporter.plan(&env, &unavailable).is_none());
}
