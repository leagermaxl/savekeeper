//! Tests of the WMI abstraction.

use super::*;

/// A source that answers only after a delay.
struct SlowWmi(Duration);

impl WmiSource for SlowWmi {
    fn query(&self, _ns: &str, _q: &str, _timeout: Duration) -> Result<Vec<WmiRow>, WmiError> {
        std::thread::sleep(self.0);
        Ok(Vec::new())
    }
}

#[test]
fn row_accessors_read_only_their_type() {
    let r = row([
        ("Name", s("HP")),
        ("Count", WmiValue::Int(3)),
        ("Flag", WmiValue::Bool(true)),
        ("Empty", WmiValue::Null),
    ]);
    assert_eq!(text(&r, "Name"), Some("HP"));
    assert_eq!(text(&r, "Count"), None);
    assert_eq!(text(&r, "Missing"), None);
    assert_eq!(int(&r, "Count"), Some(3));
    assert_eq!(int(&r, "Name"), None);
    assert_eq!(int(&r, "Empty"), None);
}

#[test]
fn fake_answers_by_class_and_records_calls() {
    let fake = FakeWmi::new()
        .rows("Win32_Printer", vec![row([("Name", s("HP"))])])
        .error("Win32_EncryptableVolume", WmiError::AccessDenied);
    let timeout = Duration::from_secs(1);
    assert_eq!(
        fake.query(CIMV2, "SELECT Name FROM Win32_Printer", timeout)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        fake.query("ns", "SELECT * FROM Win32_EncryptableVolume", timeout),
        Err(WmiError::AccessDenied)
    );
    assert!(matches!(
        fake.query(CIMV2, "SELECT * FROM Other", timeout),
        Err(WmiError::Failed(_))
    ));
    assert_eq!(fake.calls().len(), 3);
    assert_eq!(fake.calls()[0].0, CIMV2);
}

#[test]
fn errors_map_to_export_errors() {
    assert!(matches!(
        ExportError::from(WmiError::Timeout),
        ExportError::Timeout
    ));
    assert!(matches!(
        ExportError::from(WmiError::Cancelled),
        ExportError::Cancelled
    ));
    assert!(matches!(
        ExportError::from(WmiError::Unsupported),
        ExportError::NotAvailable
    ));
    match ExportError::from(WmiError::AccessDenied) {
        ExportError::Io(err) => assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied),
        other => panic!("{other:?}"),
    }
    match ExportError::from(WmiError::Failed("boom".to_owned())) {
        ExportError::Io(err) => assert!(err.to_string().contains("boom")),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn query_async_returns_rows_and_stops_on_cancel() {
    let fake: Arc<dyn WmiSource> =
        Arc::new(FakeWmi::new().rows("Win32_Printer", vec![row([("Name", s("HP"))])]));
    let cancel = CancellationToken::new();
    let rows = query_async(
        fake.clone(),
        CIMV2,
        "SELECT Name FROM Win32_Printer",
        Duration::from_secs(1),
        &cancel,
    )
    .await
    .unwrap();
    assert_eq!(text(&rows[0], "Name"), Some("HP"));

    cancel.cancel();
    let cancelled = query_async(fake, CIMV2, "x", Duration::from_secs(1), &cancel).await;
    assert_eq!(cancelled, Err(WmiError::Cancelled));

    // Cancellation during a slow query returns at once.
    let slow: Arc<dyn WmiSource> = Arc::new(SlowWmi(Duration::from_millis(500)));
    let cancel = CancellationToken::new();
    let started = Instant::now();
    let canceller = cancel.clone();
    let (result, ()) = tokio::join!(
        query_async(slow, CIMV2, "x", Duration::from_secs(5), &cancel),
        async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            canceller.cancel();
        }
    );
    assert_eq!(result, Err(WmiError::Cancelled));
    assert!(started.elapsed() < Duration::from_millis(450));
}

#[cfg(not(windows))]
#[test]
fn system_wmi_is_unsupported_off_windows() {
    let result = SystemWmi.query(CIMV2, "SELECT * FROM Win32_Printer", Duration::from_secs(1));
    assert_eq!(result, Err(WmiError::Unsupported));
}

/// The real WMI answers a simple query; integers and strings are converted.
#[cfg(windows)]
#[test]
fn system_wmi_reads_the_operating_system() {
    let rows = SystemWmi
        .query(
            CIMV2,
            "SELECT Caption, OSLanguage FROM Win32_OperatingSystem",
            Duration::from_secs(30),
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert!(text(&rows[0], "Caption").is_some_and(|c| c.contains("Windows")));
    assert!(int(&rows[0], "OSLanguage").is_some());
}

/// An unknown class is a failure, not a panic or an empty result.
#[cfg(windows)]
#[test]
fn system_wmi_reports_an_invalid_class() {
    let result = SystemWmi.query(
        CIMV2,
        "SELECT * FROM Sk_NoSuchClass",
        Duration::from_secs(30),
    );
    assert!(matches!(result, Err(WmiError::Failed(_))), "{result:?}");
}

/// A zero timeout gives up before WMI answers; the worker thread is left to finish.
#[cfg(windows)]
#[test]
fn system_wmi_times_out() {
    let result = SystemWmi.query(
        CIMV2,
        "SELECT Name FROM Win32_Printer",
        Duration::from_millis(0),
    );
    assert_eq!(result, Err(WmiError::Timeout));
}
