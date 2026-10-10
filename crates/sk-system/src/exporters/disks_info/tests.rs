//! Tests of the `disks-info` exporter on a fake WMI.

use std::path::Path;

use sk_core::env::{DriveKind, DriveMedia};
use sk_core::CancellationToken;

use super::*;
use crate::exporters::test_util::{assert_system_settings_plan, files_under, run_export, temp_dir};
use crate::wql::{row, s, FakeWmi, WmiValue};

fn volume(letter: &str, protection: i64, conversion: i64) -> WmiRow {
    row([
        ("DriveLetter", s(letter)),
        ("ProtectionStatus", WmiValue::Int(protection)),
        ("ConversionStatus", WmiValue::Int(conversion)),
    ])
}

/// Drives C: (fake), D:, E: and a network drive Z:.
fn env_with_drives(root: &Path) -> Environment {
    let mut env = Environment::fake(root);
    let c = env.drives[0].clone();
    for (letter, kind) in [
        ('D', DriveKind::Fixed),
        ('E', DriveKind::Removable),
        ('Z', DriveKind::Network),
    ] {
        env.drives.push(DriveInfo {
            letter,
            kind,
            media: DriveMedia::Unknown,
            label: Some(format!("Disk {letter}")),
            ..c.clone()
        });
    }
    env
}

fn bitlocker_wmi() -> Arc<FakeWmi> {
    Arc::new(FakeWmi::new().rows(
        "Win32_EncryptableVolume",
        vec![
            volume("C:", 1, 1),
            volume("d:", 0, 0),
            volume("E:", 0, 1),
            row([
                ("DriveLetter", WmiValue::Null),
                ("ProtectionStatus", WmiValue::Int(0)),
            ]),
        ],
    ))
}

fn read_json(path: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn bitlocker_column(doc: &serde_json::Value) -> Vec<(String, String)> {
    doc["drives"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            (
                d["letter"].as_str().unwrap().to_owned(),
                d["bitlocker"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

#[test]
fn bitlocker_status_of_volume_objects() {
    assert_eq!(BitLocker::of(&volume("C:", 1, 1)), BitLocker::On);
    assert_eq!(BitLocker::of(&volume("C:", 0, 0)), BitLocker::Off);
    for conversion in 1..=5 {
        assert_eq!(
            BitLocker::of(&volume("C:", 0, conversion)),
            BitLocker::Suspended
        );
    }
    assert_eq!(BitLocker::of(&volume("C:", 2, 1)), BitLocker::Unknown);
    assert_eq!(
        BitLocker::of(&row([("ProtectionStatus", WmiValue::Int(0))])),
        BitLocker::Unknown
    );
    assert_eq!(BitLocker::of(&row([])), BitLocker::Unknown);
}

#[test]
fn probe_is_always_available_with_the_drive_count() {
    let env = env_with_drives(Path::new("/fake"));
    assert_eq!(
        DisksInfoExporter::probe(&env),
        Availability::Available {
            estimated_bytes: Some(4 * ESTIMATE_BYTES_PER_DRIVE),
            details: BTreeMap::from([("drives".to_owned(), "4".to_owned())]),
        }
    );
    let mut empty = env;
    empty.drives.clear();
    assert!(matches!(
        DisksInfoExporter::probe(&empty),
        Availability::Available {
            estimated_bytes: Some(ESTIMATE_BYTES_PER_DRIVE),
            ..
        }
    ));
}

#[cfg(not(windows))]
#[test]
fn detect_off_windows_is_unavailable() {
    let env = Environment::fake(Path::new("/fake"));
    assert_eq!(
        DisksInfoExporter::new().detect(&env),
        unavailable(NOT_WINDOWS)
    );
}

#[cfg(windows)]
#[test]
fn detect_on_windows_needs_no_wmi() {
    let env = Environment::fake(Path::new("/fake"));
    let wmi = Arc::new(FakeWmi::new());
    let avail = DisksInfoExporter::with_wmi(wmi.clone()).detect(&env);
    assert!(matches!(avail, Availability::Available { .. }));
    assert!(wmi.calls().is_empty());
}

#[test]
fn plan_is_system_settings_without_sensitivity() {
    assert_system_settings_plan(&DisksInfoExporter::new(), TITLE, Sensitivity::None);
}

#[tokio::test]
async fn elevated_run_writes_drives_with_bitlocker_status() {
    let dir = temp_dir();
    let mut env = env_with_drives(dir.path());
    env.is_elevated = true;
    let target = dir.path().join("backup").join("system").join(ID);
    let wmi = bitlocker_wmi();
    let exporter = DisksInfoExporter::with_wmi(wmi.clone());

    let result = run_export(&exporter, &env, &target, &CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(
        wmi.calls(),
        [(BITLOCKER_NAMESPACE.to_owned(), BITLOCKER_QUERY.to_owned())]
    );
    let doc = read_json(&target.join(FILE));
    assert_eq!(
        bitlocker_column(&doc),
        [
            ("C", "on"),
            ("D", "off"),
            ("E", "suspended"),
            ("Z", "unknown")
        ]
        .map(|(l, b)| (l.to_owned(), b.to_owned()))
    );
    let c = &doc["drives"][0];
    assert_eq!(c["kind"], "fixed");
    assert_eq!(c["media"], "ssd");
    assert_eq!(c["fs"], "NTFS");
    assert_eq!(c["volume_serial"], 0x1234_5678);
    assert_eq!(c["total_bytes"], 512u64 * 1024 * 1024 * 1024);
    assert_eq!(c["free_bytes"], 128u64 * 1024 * 1024 * 1024);
    assert_eq!(doc["drives"][3]["kind"], "network");
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_eq!(result.restore_hint, RestoreHint::None);
    let content = std::fs::read(target.join(FILE)).unwrap();
    assert!(content.ends_with(b"\n"));
    assert_eq!(result.files[0].rel_path, Path::new(FILE));
    assert_eq!(result.files[0].bytes, content.len() as u64);
    assert_eq!(
        result.files[0].blake3,
        blake3::hash(&content).to_hex().as_str()
    );
    // P1: nothing outside the target folder.
    assert_eq!(
        files_under(dir.path()),
        [Path::new("backup").join("system").join(ID).join(FILE)]
    );
}

#[tokio::test]
async fn run_without_admin_does_not_ask_wmi_and_warns() {
    let dir = temp_dir();
    let env = env_with_drives(dir.path());
    let target = dir.path().join("out");
    let wmi = bitlocker_wmi();
    let exporter = DisksInfoExporter::with_wmi(wmi.clone());

    let result = run_export(&exporter, &env, &target, &CancellationToken::new())
        .await
        .unwrap();

    assert!(wmi.calls().is_empty());
    let doc = read_json(&target.join(FILE));
    assert!(bitlocker_column(&doc).iter().all(|(_, b)| b == "unknown"));
    assert_eq!(result.warnings.len(), 1);
    assert_eq!(result.warnings[0].message_key, ISSUE_BITLOCKER_NEEDS_ADMIN);
}

#[tokio::test]
async fn failed_bitlocker_query_is_a_warning() {
    let dir = temp_dir();
    let mut env = env_with_drives(dir.path());
    env.is_elevated = true;
    let target = dir.path().join("out");
    let wmi = FakeWmi::new().error(
        "Win32_EncryptableVolume",
        WmiError::Failed("invalid namespace".to_owned()),
    );
    let exporter = DisksInfoExporter::with_wmi(Arc::new(wmi));

    let result = run_export(&exporter, &env, &target, &CancellationToken::new())
        .await
        .unwrap();

    let doc = read_json(&target.join(FILE));
    assert!(bitlocker_column(&doc).iter().all(|(_, b)| b == "unknown"));
    assert_eq!(result.warnings.len(), 1);
    assert_eq!(result.warnings[0].message_key, ISSUE_BITLOCKER_FAILED);
    assert!(result.warnings[0].message_args["error"].contains("invalid namespace"));
}

#[tokio::test]
async fn cancelled_run_writes_nothing() {
    let dir = temp_dir();
    let mut env = env_with_drives(dir.path());
    env.is_elevated = true;
    let cancel = CancellationToken::new();
    cancel.cancel();
    let exporter = DisksInfoExporter::with_wmi(bitlocker_wmi());
    let err = run_export(&exporter, &env, &dir.path().join("out"), &cancel)
        .await
        .unwrap_err();
    assert!(matches!(err, ExportError::Cancelled), "{err:?}");
    assert!(files_under(dir.path()).is_empty());
}
