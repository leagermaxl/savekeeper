use std::path::{Path, PathBuf};

use sk_core::model::Sensitivity;
use sk_core::CancellationToken;

use super::*;
use crate::exporters::test_util::{assert_system_settings_plan, files_under, run_export, temp_dir};

/// The stock `hosts` of Windows: comments only.
const STOCK: &str = "# Copyright (c) 1993-2009 Microsoft Corp.\r\n\
#\r\n\
# This is a sample HOSTS file used by Microsoft TCP/IP for Windows.\r\n\
#\r\n\
# localhost name resolution is handled within DNS itself.\r\n\
#\t127.0.0.1       localhost\r\n\
#\t::1             localhost\r\n";

/// Writes `content` as the `hosts` file of the fake machine under `root`.
fn write_hosts(root: &Path, content: &[u8]) -> PathBuf {
    let dir = root
        .join("Windows")
        .join("System32")
        .join("drivers")
        .join("etc");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(FILE);
    std::fs::write(&path, content).unwrap();
    path
}

#[test]
fn plan_is_system_settings_without_sensitivity() {
    assert_system_settings_plan(&HostsExporter, TITLE, Sensitivity::None);
}

#[cfg(not(windows))]
#[test]
fn detect_off_windows_is_not_windows() {
    let env = Environment::fake(Path::new("/fake"));
    assert_eq!(HostsExporter.detect(&env), unavailable(NOT_WINDOWS));
}

#[test]
fn entry_lines() {
    assert!(is_entry(b"127.0.0.1 example.test\r\n"));
    assert!(is_entry(b"  \t0.0.0.0 ads.test # blocked\n"));
    assert!(!is_entry(b"# 127.0.0.1 localhost\r\n"));
    assert!(!is_entry(b"   #comment"));
    assert!(!is_entry(b" \t \r\n"));
    assert!(!is_entry(b""));
}

#[test]
fn stock_file_is_default() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    write_hosts(dir.path(), STOCK.as_bytes());
    assert_eq!(HostsExporter::probe(&env), unavailable(DEFAULT));
    // An empty file and a BOM followed by comments are stock as well.
    write_hosts(dir.path(), b"");
    assert_eq!(HostsExporter::probe(&env), unavailable(DEFAULT));
    write_hosts(dir.path(), b"\xEF\xBB\xBF# comment\r\n\r\n");
    assert_eq!(HostsExporter::probe(&env), unavailable(DEFAULT));
}

#[test]
fn file_with_an_entry_is_available_with_its_size() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    let content = format!("{STOCK}\r\n127.0.0.1 dev.local\r\n");
    write_hosts(dir.path(), content.as_bytes());
    assert_eq!(
        HostsExporter::probe(&env),
        Availability::Available {
            estimated_bytes: Some(content.len() as u64),
            details: Default::default(),
        }
    );
    // An entry right after a UTF-8 BOM.
    write_hosts(dir.path(), b"\xEF\xBB\xBF10.0.0.1 nas\n");
    assert!(matches!(
        HostsExporter::probe(&env),
        Availability::Available { .. }
    ));
}

#[test]
fn missing_file_is_unavailable() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    assert_eq!(HostsExporter::probe(&env), unavailable(MISSING));
}

#[tokio::test]
async fn run_copies_the_file_into_target_dir_only() {
    let dir = temp_dir();
    let machine = dir.path().join("machine");
    let env = Environment::fake(&machine);
    let content = b"127.0.0.1 dev.local\r\n# note\r\n";
    let src = write_hosts(&machine, content);
    let target = dir.path().join("backup").join("system").join(ID);

    let result = run_export(&HostsExporter, &env, &target, &CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(result.files.len(), 1);
    let file = &result.files[0];
    assert_eq!(file.rel_path, PathBuf::from(FILE));
    assert_eq!(file.bytes, content.len() as u64);
    assert_eq!(file.blake3, blake3::hash(content).to_hex().to_string());
    assert_eq!(std::fs::read(target.join(FILE)).unwrap(), content);
    assert!(result.warnings.is_empty());
    assert_eq!(
        result.restore_hint,
        RestoreHint::Manual {
            key: RESTORE_KEY.to_owned()
        }
    );
    // P1: the source is unchanged and nothing is written outside the target folder.
    assert_eq!(std::fs::read(&src).unwrap(), content);
    assert_eq!(
        files_under(&dir.path().join("backup")),
        [PathBuf::from("system").join(ID).join(FILE)]
    );
}

#[tokio::test]
async fn run_without_the_file_is_not_available() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    let target = dir.path().join("out");
    let result = run_export(&HostsExporter, &env, &target, &CancellationToken::new()).await;
    assert!(
        matches!(result, Err(ExportError::NotAvailable)),
        "{result:?}"
    );
    assert!(!target.exists());
}

#[tokio::test]
async fn cancelled_run_writes_nothing() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    write_hosts(dir.path(), b"127.0.0.1 a\n");
    let cancel = CancellationToken::new();
    cancel.cancel();
    let target = dir.path().join("out");
    let result = run_export(&HostsExporter, &env, &target, &cancel).await;
    assert!(matches!(result, Err(ExportError::Cancelled)), "{result:?}");
    assert!(!target.exists());
}

/// The real `hosts` is detected without error and, when it has entries, copied (read
/// only, into `target/tmp`).
#[cfg(windows)]
#[tokio::test]
async fn real_hosts_detect_and_run() {
    let env = Environment::detect().unwrap();
    let avail = HostsExporter.detect(&env);
    println!("hosts: {avail:?}");
    if let Availability::Available { .. } = avail {
        let dir = temp_dir();
        let result = run_export(
            &HostsExporter,
            &env,
            &dir.path().join(ID),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(result.files[0].rel_path, PathBuf::from(FILE));
    }
}
