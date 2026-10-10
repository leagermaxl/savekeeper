//! Tests of the `printers` exporter on a fake WMI.

use std::path::Path;

use sk_core::CancellationToken;

use super::*;
use crate::exporters::test_util::{assert_system_settings_plan, files_under, run_export, temp_dir};
use crate::wql::{row, s, FakeWmi};

fn printer_row(name: &str, driver: &str, port: &str) -> WmiRow {
    row([
        ("Name", s(name)),
        ("DriverName", s(driver)),
        ("PortName", s(port)),
    ])
}

/// The printers of a stock Windows (as `Win32_Printer` shows them on ru-RU) and two of
/// the user.
fn rows() -> Vec<WmiRow> {
    vec![
        printer_row(
            "Microsoft XPS Document Writer",
            "Microsoft XPS Document Writer v4",
            "PORTPROMPT:",
        ),
        printer_row("Microsoft Print to PDF", "Microsoft Print To PDF", "PORTPROMPT:"),
        printer_row("Факс", "Microsoft Shared Fax Driver", "SHRFAX:"),
        printer_row("OneNote (Desktop)", "Send to Microsoft OneNote 16 Driver", "nul:"),
        printer_row(
            "OneNote for Windows 10",
            "Microsoft Software Printer Driver",
            "Microsoft.Office.OneNote_16001.14326.21802.0_x64__8wekyb3d8bbwe_microsoft.onenoteim_S-1-5-21",
        ),
        printer_row("Office HP", "HP Universal Printing PCL 6", "IP_192.168.1.20"),
        printer_row("brother", "Microsoft IPP Class Driver", "WSD-1234"),
    ]
}

fn exporter(wmi: FakeWmi) -> PrintersExporter {
    PrintersExporter::with_wmi(Arc::new(wmi))
}

#[test]
fn builtin_printers_are_left_out_and_the_rest_sorted() {
    let printers = user_printers(&rows());
    let names: Vec<_> = printers.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["brother", "Office HP"]);
    assert_eq!(
        printers[1],
        Printer {
            name: "Office HP".to_owned(),
            driver: Some("HP Universal Printing PCL 6".to_owned()),
            port: Some("IP_192.168.1.20".to_owned()),
        }
    );
}

#[test]
fn fax_is_builtin_by_name_or_port_and_rows_without_name_are_skipped() {
    let fax = Printer {
        name: "FAX".to_owned(),
        driver: None,
        port: None,
    };
    assert!(is_builtin(&fax));
    let by_port = Printer {
        name: "Telefax".to_owned(),
        driver: Some("Some driver".to_owned()),
        port: Some("shrfax:".to_owned()),
    };
    assert!(is_builtin(&by_port));
    // A multifunction printer that can fax is not the Windows fax.
    let mfp = Printer {
        name: "HP OfficeJet Pro 8710 Fax".to_owned(),
        driver: Some("HP OfficeJet Pro 8710 PCL-3".to_owned()),
        port: Some("USB001".to_owned()),
    };
    assert!(!is_builtin(&mfp));
    assert!(user_printers(&[row([("DriverName", s("x"))])]).is_empty());
}

#[test]
fn probe_counts_user_printers() {
    let wmi = FakeWmi::new().rows("Win32_Printer", rows());
    let avail = exporter(wmi).probe();
    assert_eq!(
        avail,
        Availability::Available {
            estimated_bytes: Some(2 * ESTIMATE_BYTES_PER_PRINTER),
            details: BTreeMap::from([("printers".to_owned(), "2".to_owned())]),
        }
    );
}

#[test]
fn probe_without_user_printers_failure_and_timeout() {
    let only_builtin = FakeWmi::new().rows("Win32_Printer", rows()[..5].to_vec());
    assert_eq!(exporter(only_builtin).probe(), unavailable(NONE));

    let failed = FakeWmi::new().error("Win32_Printer", WmiError::Failed("rpc".to_owned()));
    assert_eq!(exporter(failed).probe(), unavailable(QUERY_FAILED));

    let slow = FakeWmi::new().error("Win32_Printer", WmiError::Timeout);
    assert_eq!(
        exporter(slow).probe(),
        Availability::Available {
            estimated_bytes: None,
            details: BTreeMap::new(),
        }
    );
}

#[cfg(not(windows))]
#[test]
fn detect_off_windows_is_unavailable() {
    let env = Environment::fake(Path::new("/fake"));
    let wmi = FakeWmi::new().rows("Win32_Printer", rows());
    assert_eq!(exporter(wmi).detect(&env), unavailable(NOT_WINDOWS));
}

#[test]
fn plan_is_system_settings_without_sensitivity() {
    assert_system_settings_plan(&PrintersExporter::new(), TITLE, Sensitivity::None);
}

#[tokio::test]
async fn run_writes_printers_json_only_into_target_dir() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    let target = dir.path().join("backup").join("system").join(ID);
    let wmi = Arc::new(FakeWmi::new().rows("Win32_Printer", rows()));
    let exporter = PrintersExporter::with_wmi(wmi.clone());

    let result = run_export(&exporter, &env, &target, &CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(wmi.calls(), [(CIMV2.to_owned(), QUERY.to_owned())]);
    let content = std::fs::read_to_string(target.join(FILE)).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(
        doc,
        json!({ "printers": [
            { "name": "brother", "driver": "Microsoft IPP Class Driver", "port": "WSD-1234" },
            { "name": "Office HP", "driver": "HP Universal Printing PCL 6", "port": "IP_192.168.1.20" },
        ] })
    );
    assert!(content.ends_with('\n'));
    assert_eq!(result.files.len(), 1);
    assert_eq!(result.files[0].rel_path, Path::new(FILE));
    assert_eq!(result.files[0].bytes, content.len() as u64);
    assert_eq!(
        result.files[0].blake3,
        blake3::hash(content.as_bytes()).to_hex().as_str()
    );
    assert!(result.warnings.is_empty());
    assert_eq!(result.restore_hint, RestoreHint::None);
    // P1: nothing outside the target folder.
    assert_eq!(
        files_under(dir.path()),
        [Path::new("backup").join("system").join(ID).join(FILE)]
    );
}

#[tokio::test]
async fn run_without_user_printers_or_with_failed_query_writes_nothing() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    let target = dir.path().join("out");
    let cancel = CancellationToken::new();

    let only_builtin = exporter(FakeWmi::new().rows("Win32_Printer", rows()[..3].to_vec()));
    let err = run_export(&only_builtin, &env, &target, &cancel)
        .await
        .unwrap_err();
    assert!(matches!(err, ExportError::NotAvailable), "{err:?}");

    let slow = exporter(FakeWmi::new().error("Win32_Printer", WmiError::Timeout));
    let err = run_export(&slow, &env, &target, &cancel).await.unwrap_err();
    assert!(matches!(err, ExportError::Timeout), "{err:?}");

    let denied = exporter(FakeWmi::new().error("Win32_Printer", WmiError::AccessDenied));
    let err = run_export(&denied, &env, &target, &cancel)
        .await
        .unwrap_err();
    assert!(matches!(err, ExportError::Io(_)), "{err:?}");

    cancel.cancel();
    let any = exporter(FakeWmi::new().rows("Win32_Printer", rows()));
    let err = run_export(&any, &env, &target, &cancel).await.unwrap_err();
    assert!(matches!(err, ExportError::Cancelled), "{err:?}");

    assert!(files_under(dir.path()).is_empty());
}

/// The real `Win32_Printer` query answers within the `detect` budget (FR-06-01).
#[cfg(windows)]
#[test]
fn detect_on_this_machine_is_fast() {
    let env = Environment::fake(Path::new("/fake"));
    let started = std::time::Instant::now();
    let avail = PrintersExporter::new().detect(&env);
    let elapsed = started.elapsed();
    assert!(elapsed < Duration::from_millis(700), "{elapsed:?}");
    assert_ne!(avail, unavailable(QUERY_FAILED));
}
