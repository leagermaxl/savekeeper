//! Tests of the `windows-info` exporter on a fake WMI and registry.

use std::path::Path;

use sk_core::registry::MemRegistry;
use sk_core::CancellationToken;

use super::*;
use crate::exporters::test_util::{assert_system_settings_plan, files_under, run_export, temp_dir};
use crate::wql::{row, s, FakeWmi, WmiValue};

fn license(status: i64, channel: &str) -> WmiRow {
    row([
        ("LicenseStatus", WmiValue::Int(status)),
        ("ProductKeyChannel", s(channel)),
    ])
}

fn registry() -> Arc<MemRegistry> {
    let mut reg = MemRegistry::new();
    reg.set_string(
        RegHive::Hklm,
        CURRENT_VERSION_KEY,
        EDITION_VALUE,
        "Professional",
    );
    Arc::new(reg)
}

fn read_json(path: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn activation_prefers_the_licensed_row() {
    let rows = [license(5, "Retail"), license(1, "OEM:DM")];
    assert_eq!(
        activation(&rows),
        Activation {
            license_status: Some(1),
            channel: Some("OEM:DM".to_owned()),
        }
    );
    assert_eq!(activation(&rows[..1]).license_status, Some(5));
    assert_eq!(
        activation(&[]),
        Activation {
            license_status: None,
            channel: None,
        }
    );
}

#[test]
fn status_names() {
    let names: Vec<_> = (0..=7).map(|n| status_name(Some(n))).collect();
    assert_eq!(
        names,
        [
            "unlicensed",
            "licensed",
            "oob_grace",
            "oot_grace",
            "non_genuine_grace",
            "notification",
            "extended_grace",
            "unknown"
        ]
    );
    assert_eq!(status_name(None), "unknown");
}

/// The queries never select a key column (FR-06-05).
#[test]
fn queries_select_no_key() {
    let columns = |query: &str| -> Vec<String> {
        let select = query.split(" FROM ").next().unwrap();
        let list = select.strip_prefix("SELECT ").unwrap();
        list.split(',').map(|c| c.trim().to_owned()).collect()
    };
    assert_eq!(
        columns(ACTIVATION_QUERY),
        ["LicenseStatus", "ProductKeyChannel"]
    );
    assert_eq!(columns(OEM_KEY_QUERY), ["Version"]);
}

#[test]
fn probe_is_always_available() {
    assert_eq!(
        WindowsInfoExporter::probe(),
        Availability::Available {
            estimated_bytes: Some(ESTIMATE_BYTES),
            details: BTreeMap::new(),
        }
    );
}

#[cfg(not(windows))]
#[test]
fn detect_off_windows_is_unavailable() {
    let env = Environment::fake(Path::new("/fake"));
    assert_eq!(
        WindowsInfoExporter::new().detect(&env),
        unavailable(NOT_WINDOWS)
    );
}

#[test]
fn plan_is_system_settings_without_sensitivity() {
    assert_system_settings_plan(&WindowsInfoExporter::new(), TITLE, Sensitivity::None);
}

#[tokio::test]
async fn run_writes_windows_json_only_into_target_dir() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    let target = dir.path().join("backup").join("system").join(ID);
    let wmi = Arc::new(
        FakeWmi::new()
            .rows("SoftwareLicensingProduct", vec![license(1, "Retail")])
            .rows(
                "SoftwareLicensingService",
                vec![row([("Version", s("10.0"))])],
            ),
    );
    let exporter = WindowsInfoExporter::with(wmi.clone(), registry());

    let result = run_export(&exporter, &env, &target, &CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(
        wmi.calls(),
        [
            (CIMV2.to_owned(), ACTIVATION_QUERY.to_owned()),
            (CIMV2.to_owned(), OEM_KEY_QUERY.to_owned())
        ]
    );
    let doc = read_json(&target.join(FILE));
    assert_eq!(
        doc,
        json!({
            "product": "Windows 11 Pro",
            "edition": "Professional",
            "display_version": "24H2",
            "build": "26100.2033",
            "arch": "x86_64",
            "ui_language": "en-US",
            "machine_name": "FAKE-PC",
            "activation": { "status": "licensed", "license_status": 1, "channel": "Retail" },
            "oem_key_in_firmware": true,
        })
    );
    let content = std::fs::read(target.join(FILE)).unwrap();
    assert!(content.ends_with(b"\n"));
    assert_eq!(result.files[0].rel_path, Path::new(FILE));
    assert_eq!(result.files[0].bytes, content.len() as u64);
    assert_eq!(
        result.files[0].blake3,
        blake3::hash(&content).to_hex().as_str()
    );
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_eq!(result.restore_hint, RestoreHint::None);
    // P1: nothing outside the target folder.
    assert_eq!(
        files_under(dir.path()),
        [Path::new("backup").join("system").join(ID).join(FILE)]
    );
}

#[tokio::test]
async fn failed_queries_give_unknown_values_and_a_warning() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    let target = dir.path().join("out");
    let wmi = FakeWmi::new()
        .error("SoftwareLicensingProduct", WmiError::Timeout)
        .error("SoftwareLicensingService", WmiError::AccessDenied);
    let exporter = WindowsInfoExporter::with(Arc::new(wmi), Arc::new(MemRegistry::new()));

    let result = run_export(&exporter, &env, &target, &CancellationToken::new())
        .await
        .unwrap();

    let doc = read_json(&target.join(FILE));
    assert_eq!(
        doc["activation"],
        json!({ "status": "unknown", "license_status": null, "channel": null })
    );
    assert_eq!(doc["oem_key_in_firmware"], serde_json::Value::Null);
    assert_eq!(doc["edition"], serde_json::Value::Null);
    assert_eq!(result.warnings.len(), 1);
    assert_eq!(result.warnings[0].message_key, ISSUE_ACTIVATION_FAILED);
    assert_eq!(
        result.warnings[0].message_args["error"],
        "WMI query timed out"
    );
}

#[tokio::test]
async fn no_oem_key_and_no_license_rows() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    let target = dir.path().join("out");
    let wmi = FakeWmi::new()
        .rows("SoftwareLicensingProduct", Vec::new())
        .rows("SoftwareLicensingService", Vec::new());
    let exporter = WindowsInfoExporter::with(Arc::new(wmi), registry());

    let result = run_export(&exporter, &env, &target, &CancellationToken::new())
        .await
        .unwrap();

    let doc = read_json(&target.join(FILE));
    assert_eq!(doc["activation"]["status"], "unknown");
    assert_eq!(doc["oem_key_in_firmware"], false);
    assert!(result.warnings.is_empty());
}

#[tokio::test]
async fn cancelled_run_writes_nothing() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    let cancel = CancellationToken::new();
    cancel.cancel();
    let exporter = WindowsInfoExporter::with(Arc::new(FakeWmi::new()), registry());
    let err = run_export(&exporter, &env, &dir.path().join("out"), &cancel)
        .await
        .unwrap_err();
    assert!(matches!(err, ExportError::Cancelled), "{err:?}");
    assert!(files_under(dir.path()).is_empty());
}

/// The real activation query of this machine: a known status, no key in the file.
#[cfg(windows)]
#[tokio::test]
async fn run_on_this_machine_reads_the_activation() {
    let dir = temp_dir();
    let env = Environment::detect().unwrap();
    let target = dir.path().join("out");
    let result = run_export(
        &WindowsInfoExporter::new(),
        &env,
        &target,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let doc = read_json(&target.join(FILE));
    assert!(doc["activation"]["license_status"].is_i64(), "{doc}");
    assert!(doc["oem_key_in_firmware"].is_boolean(), "{doc}");
    assert!(doc["edition"].is_string(), "{doc}");
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
}
