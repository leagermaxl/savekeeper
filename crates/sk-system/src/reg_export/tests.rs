use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use sk_core::registry::{MemRegistry, RegValue};

use super::*;
use crate::cmd::{CmdOutput, FakeCmdRunner};
use crate::exporters::test_util::{files_under, reg_file_content, reg_runner, temp_dir};

const MOUSE: &str = r"Control Panel\Mouse";

fn registry_with(keys: &[(RegHive, &str)]) -> Arc<MemRegistry> {
    let mut registry = MemRegistry::new();
    for (hive, key) in keys {
        registry.add_key(*hive, key);
    }
    Arc::new(registry)
}

fn reg_export(
    runner: FakeCmdRunner,
    registry: Arc<dyn RegistryReader>,
) -> (RegExport, Arc<FakeCmdRunner>) {
    let runner = Arc::new(runner);
    let export = RegExport::new(Arc::clone(&runner) as Arc<dyn CmdRunner>, registry);
    (export, runner)
}

/// A key that exists at the first check and is gone afterwards.
#[derive(Default)]
struct VanishingKey(AtomicUsize);

impl RegistryReader for VanishingKey {
    fn key_state(&self, _hive: RegHive, _key: &str) -> KeyState {
        match self.0.fetch_add(1, Ordering::SeqCst) {
            0 => KeyState::Present,
            _ => KeyState::Missing,
        }
    }
    fn string_value(&self, _hive: RegHive, _key: &str, _name: &str) -> Option<String> {
        None
    }
    fn dword_value(&self, _hive: RegHive, _key: &str, _name: &str) -> Option<u32> {
        None
    }
    fn subkeys(&self, _hive: RegHive, _key: &str) -> Vec<String> {
        Vec::new()
    }
    fn values(&self, _hive: RegHive, _key: &str) -> Option<Vec<RegValue>> {
        None
    }
}

fn failing_runner(code: i32, stdout: &str, stderr: &str) -> FakeCmdRunner {
    let (stdout, stderr) = (stdout.to_owned(), stderr.to_owned());
    FakeCmdRunner::new().on(REG_EXE, move |_| {
        Ok(CmdOutput {
            code,
            stdout: stdout.clone(),
            stderr: stderr.clone(),
            truncated: false,
        })
    })
}

#[test]
fn full_key_names_hive_and_normalized_key() {
    assert_eq!(full_key(RegHive::Hkcu, MOUSE), r"HKCU\Control Panel\Mouse");
    assert_eq!(
        full_key(RegHive::Hklm, r"\SOFTWARE//Foo\"),
        r"HKLM\SOFTWARE\Foo"
    );
    assert_eq!(full_key(RegHive::Hkcu, ""), "HKCU");
}

#[test]
fn reg_file_names_are_slugs_with_collision_suffixes() {
    let mut taken = BTreeSet::new();
    assert_eq!(reg_file_name(MOUSE, &mut taken), "control-panel-mouse.reg");
    assert_eq!(
        reg_file_name(r"Software\Microsoft\Windows\CurrentVersion\Run", &mut taken),
        "software-microsoft-windows-currentversion-run.reg"
    );
    assert_eq!(
        reg_file_name(r"Control Panel/Mouse", &mut taken),
        "control-panel-mouse-2.reg"
    );
    assert_eq!(
        reg_file_name(r"\control panel\mouse\", &mut taken),
        "control-panel-mouse-3.reg"
    );
    assert_eq!(
        reg_file_name(r"Software\7-Zip (x64)", &mut taken),
        "software-7-zip-x64.reg"
    );
    assert_eq!(
        reg_file_name(r"Software\Программа", &mut taken),
        "software-программа.reg"
    );
    assert_eq!(reg_file_name("", &mut taken), "key.reg");
    assert_eq!(reg_file_name(r"\ ", &mut taken), "key-2.reg");
}

#[tokio::test]
async fn exports_key_with_reg_exe_from_windows_folder() {
    let dir = temp_dir();
    let windir = dir.path().join("Windows");
    let out_file = dir.path().join("backup").join("mouse.reg");
    let (export, runner) = reg_export(reg_runner(), registry_with(&[(RegHive::Hkcu, MOUSE)]));

    let file = export
        .export(
            Some(&windir),
            RegHive::Hkcu,
            MOUSE,
            &out_file,
            &CancellationToken::new(),
        )
        .await
        .unwrap();

    let content = reg_file_content(r"HKEY_CURRENT_USER\Control Panel\Mouse");
    assert_eq!(file.rel_path, PathBuf::from("mouse.reg"));
    assert_eq!(file.bytes, content.len() as u64);
    assert_eq!(file.blake3, blake3::hash(&content).to_hex().to_string());
    assert_eq!(std::fs::read(&out_file).unwrap(), content);

    let out = out_file.to_string_lossy().into_owned();
    let expected = ["export", r"HKCU\Control Panel\Mouse", &out, "/y"];
    assert_eq!(
        runner.calls(),
        [(REG_EXE.to_owned(), expected.map(str::to_owned).to_vec())]
    );
    assert_eq!(reg_exe(&windir), windir.join("System32").join(REG_EXE));
    // P1: only the output file is written.
    assert_eq!(
        files_under(dir.path()),
        [PathBuf::from("backup").join("mouse.reg")]
    );
}

#[tokio::test]
async fn missing_key_is_not_available_without_running_reg() {
    let dir = temp_dir();
    let (export, runner) = reg_export(reg_runner(), registry_with(&[]));
    let result = export
        .export(
            Some(dir.path()),
            RegHive::Hkcu,
            r"Software\NoSuchKey",
            &dir.path().join("out.reg"),
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
async fn key_gone_during_export_is_not_available() {
    let dir = temp_dir();
    let (export, _) = reg_export(
        failing_runner(1, "", "ERROR: key not found"),
        Arc::new(VanishingKey::default()),
    );
    let result = export
        .export(
            Some(dir.path()),
            RegHive::Hkcu,
            MOUSE,
            &dir.path().join("out.reg"),
            &CancellationToken::new(),
        )
        .await;
    assert!(
        matches!(result, Err(ExportError::NotAvailable)),
        "{result:?}"
    );
}

#[tokio::test]
async fn failing_reg_exe_is_process_failure() {
    let dir = temp_dir();
    let registry = registry_with(&[(RegHive::Hkcu, MOUSE)]);
    let out_file = dir.path().join("out.reg");
    let cancel = CancellationToken::new();

    let (export, _) = reg_export(
        failing_runner(1, "", " access denied \r\n"),
        registry.clone(),
    );
    let result = export
        .export(Some(dir.path()), RegHive::Hkcu, MOUSE, &out_file, &cancel)
        .await;
    assert!(
        matches!(&result, Err(ExportError::ProcessFailed { code: 1, stderr }) if stderr == "access denied"),
        "{result:?}"
    );

    // Without stderr the reason is taken from stdout.
    let (export, _) = reg_export(failing_runner(5, "blocked by policy", ""), registry.clone());
    let result = export
        .export(Some(dir.path()), RegHive::Hkcu, MOUSE, &out_file, &cancel)
        .await;
    assert!(
        matches!(&result, Err(ExportError::ProcessFailed { code: 5, stderr }) if stderr == "blocked by policy"),
        "{result:?}"
    );

    // Exit code 0 without the file: the file cannot be described.
    let (export, _) = reg_export(failing_runner(0, "", ""), registry);
    let result = export
        .export(Some(dir.path()), RegHive::Hkcu, MOUSE, &out_file, &cancel)
        .await;
    assert!(matches!(result, Err(ExportError::Io(_))), "{result:?}");
}

#[tokio::test]
async fn unknown_windows_folder_and_cancellation() {
    let dir = temp_dir();
    let registry = registry_with(&[(RegHive::Hkcu, MOUSE)]);
    let out_file = dir.path().join("out.reg");
    let (export, runner) = reg_export(reg_runner(), registry);

    let result = export
        .export(
            None,
            RegHive::Hkcu,
            MOUSE,
            &out_file,
            &CancellationToken::new(),
        )
        .await;
    assert!(
        matches!(&result, Err(ExportError::Io(err)) if err.kind() == std::io::ErrorKind::NotFound),
        "{result:?}"
    );

    let cancel = CancellationToken::new();
    cancel.cancel();
    let result = export
        .export(Some(dir.path()), RegHive::Hkcu, MOUSE, &out_file, &cancel)
        .await;
    assert!(matches!(result, Err(ExportError::Cancelled)), "{result:?}");
    assert!(runner.calls().is_empty());
    assert!(!out_file.exists());
}

#[cfg(not(windows))]
#[tokio::test]
async fn export_registry_off_windows_is_not_available() {
    let dir = temp_dir();
    let result = export_registry(
        RegHive::Hkcu,
        MOUSE,
        &dir.path().join("m.reg"),
        &CancellationToken::new(),
    )
    .await;
    assert!(
        matches!(result, Err(ExportError::NotAvailable)),
        "{result:?}"
    );
}

#[cfg(windows)]
mod windows {
    //! Real `reg.exe export` of existing `HKCU` keys: the registry is only read.

    use super::*;

    /// `.reg` text of an exported file: UTF-16LE after the BOM.
    fn reg_text(path: &Path) -> String {
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(&bytes[..2], [0xFF, 0xFE], "no UTF-16LE BOM");
        let (pairs, _) = bytes[2..].as_chunks::<2>();
        let units: Vec<u16> = pairs.iter().map(|pair| u16::from_le_bytes(*pair)).collect();
        String::from_utf16(&units).unwrap()
    }

    #[tokio::test]
    async fn exports_control_panel_mouse() {
        let dir = temp_dir();
        let out_file = dir.path().join("mouse").join("control-panel-mouse.reg");
        let file = export_registry(RegHive::Hkcu, MOUSE, &out_file, &CancellationToken::new())
            .await
            .unwrap();

        assert_eq!(file.rel_path, PathBuf::from("control-panel-mouse.reg"));
        assert_eq!(file.bytes, std::fs::metadata(&out_file).unwrap().len());
        let bytes = std::fs::read(&out_file).unwrap();
        assert_eq!(file.blake3, blake3::hash(&bytes).to_hex().to_string());
        let text = reg_text(&out_file);
        assert!(
            text.starts_with("Windows Registry Editor Version 5.00"),
            "{text:.200}"
        );
        assert!(
            text.contains(r"[HKEY_CURRENT_USER\Control Panel\Mouse]"),
            "{text:.400}"
        );
    }

    #[tokio::test]
    async fn missing_key_is_not_available() {
        let dir = temp_dir();
        let out_file = dir.path().join("missing.reg");
        let result = export_registry(
            RegHive::Hkcu,
            r"Software\SaveKeeperTest\NoSuchKey-2b9f0c4e",
            &out_file,
            &CancellationToken::new(),
        )
        .await;
        assert!(
            matches!(result, Err(ExportError::NotAvailable)),
            "{result:?}"
        );
        assert!(!out_file.exists());
    }
}
