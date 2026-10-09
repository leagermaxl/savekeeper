use std::path::{Path, PathBuf};
use std::sync::Arc;

use sk_core::model::Sensitivity;
use sk_core::registry::{MemRegistry, RegistryReader};
use sk_core::CancellationToken;

use super::*;
use crate::cmd::{CmdOutput, CmdRunner, FakeCmdRunner};
use crate::exporters::partial::ISSUE_KEY_MISSING;
use crate::exporters::test_util::{
    assert_system_settings_plan, fake_reg_export, files_under, reg_runner, run_export, temp_dir,
};
use crate::reg_export::{reg_exe, REG_EXE};

/// File names of [`KEYS`], in order.
const FILES: [&str; 7] = [
    "control-panel-keyboard.reg",
    "control-panel-mouse.reg",
    "control-panel-international.reg",
    "keyboard-layout.reg",
    "software-microsoft-windows-currentversion-explorer-advanced.reg",
    "console.reg",
    "software-microsoft-windows-currentversion-run.reg",
];

fn registry_with(keys: &[&str]) -> Arc<dyn RegistryReader> {
    let mut registry = MemRegistry::new();
    for key in keys {
        registry.add_key(RegHive::Hkcu, key);
    }
    Arc::new(registry)
}

fn exporter(runner: FakeCmdRunner, keys: &[&str]) -> (RegistryUserExporter, Arc<FakeCmdRunner>) {
    let runner = Arc::new(runner);
    let reg = RegExport::new(
        Arc::clone(&runner) as Arc<dyn CmdRunner>,
        registry_with(keys),
    );
    (RegistryUserExporter::with(reg), runner)
}

#[test]
fn plan_is_low_sensitivity_system_settings() {
    assert_system_settings_plan(&RegistryUserExporter::new(), TITLE, Sensitivity::Low);
}

#[test]
fn probe_is_always_available() {
    assert!(matches!(
        RegistryUserExporter::probe(),
        Availability::Available { .. }
    ));
}

#[cfg(not(windows))]
#[test]
fn detect_off_windows_is_not_windows() {
    let env = Environment::fake(Path::new("/fake"));
    assert_eq!(
        RegistryUserExporter::new().detect(&env),
        unavailable(NOT_WINDOWS)
    );
}

#[tokio::test]
async fn run_exports_every_key_into_target_dir_only() {
    let dir = temp_dir();
    let env = Environment::fake(&dir.path().join("machine"));
    let target = dir.path().join("backup").join("system").join(ID);
    let (exporter, runner) = exporter(reg_runner(), &KEYS);

    let result = run_export(&exporter, &env, &target, &CancellationToken::new())
        .await
        .unwrap();

    let names: Vec<_> = result.files.iter().map(|f| f.rel_path.clone()).collect();
    assert_eq!(names, FILES.map(PathBuf::from));
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_eq!(result.restore_hint, restore_hint());

    let windir = env.known_folder(sk_core::env::KnownFolder::WinDir).unwrap();
    let calls = runner.calls();
    assert_eq!(calls.len(), KEYS.len());
    for ((program, args), (key, file)) in calls.iter().zip(KEYS.iter().zip(FILES)) {
        assert_eq!(program, REG_EXE);
        let out = target.join(file).to_string_lossy().into_owned();
        assert_eq!(
            args,
            &[
                "export".to_owned(),
                format!(r"HKCU\{key}"),
                out,
                "/y".to_owned()
            ]
        );
    }
    let reg = reg_exe(windir);
    assert!(
        reg.ends_with(Path::new("System32").join(REG_EXE)),
        "{reg:?}"
    );
    // P1: nothing written outside the target folder.
    let mut expected: Vec<_> = FILES
        .iter()
        .map(|f| PathBuf::from("system").join(ID).join(f))
        .collect();
    expected.sort();
    assert_eq!(files_under(&dir.path().join("backup")), expected);
}

#[tokio::test]
async fn missing_and_failing_keys_are_warnings() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    let present: Vec<&str> = KEYS.iter().copied().filter(|k| *k != "Console").collect();
    // `reg export` of the Run key fails, the others succeed.
    let run_key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    let failing = FakeCmdRunner::new().on(REG_EXE, move |cmd| {
        if cmd.get_args()[1] != run_key {
            return fake_reg_export(cmd);
        }
        Ok(CmdOutput {
            code: 1,
            stderr: "access denied".to_owned(),
            ..CmdOutput::default()
        })
    });
    let (exporter, _) = exporter(failing, &present);

    let result = run_export(
        &exporter,
        &env,
        &dir.path().join("out"),
        &CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(result.files.len(), KEYS.len() - 2);
    assert_eq!(result.warnings.len(), 2, "{:?}", result.warnings);
    let missing = &result.warnings[0];
    assert_eq!(missing.message_key, ISSUE_KEY_MISSING);
    assert_eq!(missing.message_args["key"], r"HKCU\Console");
    let failed = &result.warnings[1];
    assert_eq!(failed.message_key, ISSUE_EXPORT_FAILED);
    assert_eq!(failed.message_args["key"], run_key);
    assert!(
        failed.message_args["error"].contains("access denied"),
        "{failed:?}"
    );
}

#[tokio::test]
async fn no_key_exported_is_the_first_error() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    let (exporter, runner) = exporter(reg_runner(), &[]);
    let result = run_export(
        &exporter,
        &env,
        &dir.path().join("out"),
        &CancellationToken::new(),
    )
    .await;
    assert!(
        matches!(result, Err(ExportError::NotAvailable)),
        "{result:?}"
    );
    assert!(runner.calls().is_empty());
}

#[tokio::test]
async fn cancellation_stops_the_export() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    let (exporter, runner) = exporter(reg_runner(), &KEYS);
    let cancel = CancellationToken::new();
    cancel.cancel();
    let result = run_export(&exporter, &env, &dir.path().join("out"), &cancel).await;
    assert!(matches!(result, Err(ExportError::Cancelled)), "{result:?}");
    assert!(runner.calls().is_empty());
}

/// Real `reg export` of the user's keys into `target/tmp`; the registry is only read.
#[cfg(windows)]
#[tokio::test]
async fn real_registry_user_export() {
    let env = Environment::detect().unwrap();
    let dir = temp_dir();
    let target = dir.path().join(ID);
    let result = run_export(
        &RegistryUserExporter::new(),
        &env,
        &target,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let mouse = PathBuf::from("control-panel-mouse.reg");
    assert!(
        result.files.iter().any(|f| f.rel_path == mouse),
        "{result:?}"
    );
    for file in &result.files {
        let bytes = std::fs::read(target.join(&file.rel_path)).unwrap();
        assert_eq!(
            &bytes[..2],
            [0xFF, 0xFE],
            "{:?} has no UTF-16LE BOM",
            file.rel_path
        );
        assert_eq!(file.bytes, bytes.len() as u64);
    }
}
