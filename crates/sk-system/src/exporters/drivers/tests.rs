//! Tests of the `drivers` exporter on a fake `pnputil`. No test starts the real
//! `pnputil /export-driver` (it needs administrator rights).

use sk_core::CancellationToken;

use super::*;
use crate::cmd::{CmdOutput, FakeCmdRunner};
use crate::exporters::test_util::{files_under, temp_dir};

/// `pnputil /enum-drivers` on ru-RU (decoded): localized labels, `oem<N>.inf` values.
const ENUM_RU: &str = "Служебная программа PnP (Майкрософт)\r\n\r\n\
Опубликованное имя:     oem50.inf\r\n\
Исходное имя:      adafruitcircuitplayground.inf\r\n\
Имя поставщика:      Adafruit Industries LLC\r\n\r\n\
Опубликованное имя:     oem113.inf\r\n\
Исходное имя:      amdacpbus.inf\r\n\r\n\
Published Name:     OEM7.INF\r\n\
Original Name:      oemsetup.inf\r\n";

fn exporter(runner: FakeCmdRunner) -> (DriversExporter, Arc<FakeCmdRunner>) {
    let runner = Arc::new(runner);
    (DriversExporter::with_runner(runner.clone()), runner)
}

/// A fake `pnputil /export-driver * <dir>` that writes two packages and exits with `code`.
fn export_runner(code: i32) -> FakeCmdRunner {
    FakeCmdRunner::new().on(PNPUTIL_EXE, move |cmd| {
        let dir = PathBuf::from(&cmd.get_args()[2]);
        for (folder, inf) in [
            ("oem1.inf_amd64_aa", "oem1.inf"),
            ("oem2.inf_amd64_bb", "oem2.inf"),
        ] {
            std::fs::create_dir_all(dir.join(folder)).unwrap();
            std::fs::write(dir.join(folder).join(inf), b"[Version]\r\n").unwrap();
            std::fs::write(dir.join(folder).join("driver.sys"), b"MZ").unwrap();
        }
        Ok(CmdOutput {
            code,
            stdout: "Exporting driver package...".to_owned(),
            ..CmdOutput::default()
        })
    })
}

async fn run(
    exporter: &DriversExporter,
    env: &Environment,
    target: &Path,
    elevated: bool,
    cancel: &CancellationToken,
) -> Result<ExportResult, ExportError> {
    let (events, _rx) = tokio::sync::mpsc::unbounded_channel();
    let ctx = ExportContext {
        env,
        target_dir: target,
        cancel,
        events: &events,
        elevated,
    };
    exporter.run(&ctx, &default_params()).await
}

#[test]
fn published_names_are_counted_without_localized_labels() {
    let names = published_names(ENUM_RU);
    assert_eq!(
        names.into_iter().collect::<Vec<_>>(),
        ["oem113.inf", "oem50.inf", "oem7.inf"]
    );
    assert!(published_names("oem.inf oemx1.inf oem1.inf.bak").is_empty());
}

#[test]
fn probe_without_admin_needs_elevation_with_the_estimate() {
    let env = Environment::fake(Path::new("/fake"));
    let (exporter, runner) = exporter(FakeCmdRunner::new().output(PNPUTIL_EXE, 0, ENUM_RU));
    assert_eq!(
        exporter.probe(&env),
        Availability::NeedsElevation {
            estimated_bytes: Some(3 * ESTIMATE_BYTES_PER_DRIVER)
        }
    );
    assert_eq!(
        runner.calls(),
        [(PNPUTIL_EXE.to_owned(), vec!["/enum-drivers".to_owned()])]
    );
}

#[test]
fn probe_with_admin_is_available_and_failures_leave_the_size_unknown() {
    let mut env = Environment::fake(Path::new("/fake"));
    env.is_elevated = true;
    let (ok, _) = exporter(FakeCmdRunner::new().output(PNPUTIL_EXE, 0, ENUM_RU));
    assert_eq!(
        ok.probe(&env),
        Availability::Available {
            estimated_bytes: Some(3 * ESTIMATE_BYTES_PER_DRIVER),
            details: BTreeMap::from([("drivers".to_owned(), "3".to_owned())]),
        }
    );

    let (failed, _) = exporter(FakeCmdRunner::new().output(PNPUTIL_EXE, 1, ""));
    assert_eq!(
        failed.probe(&env),
        Availability::Available {
            estimated_bytes: None,
            details: BTreeMap::new(),
        }
    );

    env.is_elevated = false;
    let (slow, _) = exporter(FakeCmdRunner::new().on(PNPUTIL_EXE, |_| Err(ExportError::Timeout)));
    assert_eq!(
        slow.probe(&env),
        Availability::NeedsElevation {
            estimated_bytes: None
        }
    );
}

#[cfg(not(windows))]
#[test]
fn detect_off_windows_is_unavailable() {
    let env = Environment::fake(Path::new("/fake"));
    let (exporter, runner) = exporter(FakeCmdRunner::new().output(PNPUTIL_EXE, 0, ENUM_RU));
    assert_eq!(exporter.detect(&env), unavailable(NOT_WINDOWS));
    assert!(runner.calls().is_empty());
}

#[test]
fn plan_requires_elevation_with_third_party_params() {
    let env = Environment::fake(Path::new("/fake"));
    let exporter = DriversExporter::new();
    let avail = Availability::NeedsElevation {
        estimated_bytes: None,
    };
    let finding = exporter.plan(&env, &avail).unwrap();
    assert!(finding.requires_elevation);
    assert_eq!(finding.category, Category::SystemSettings);
    assert_eq!(finding.sensitivity, Sensitivity::None);
    assert_eq!(finding.title, TITLE);
    assert_eq!(
        finding.target,
        sk_core::model::Target::SystemExport {
            exporter_id: ID.to_owned(),
            params: serde_json::json!({ "only_third_party": true }),
        }
    );
    let available = Availability::Available {
        estimated_bytes: None,
        details: BTreeMap::new(),
    };
    assert!(!exporter.plan(&env, &available).unwrap().requires_elevation);
    assert!(exporter.plan(&env, &unavailable(NOT_WINDOWS)).is_none());
}

#[tokio::test]
async fn run_without_admin_starts_nothing() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    let (exporter, runner) = exporter(export_runner(0));
    let err = run(
        &exporter,
        &env,
        &dir.path().join("out"),
        false,
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, ExportError::NeedsElevation), "{err:?}");
    assert!(runner.calls().is_empty());
    assert!(files_under(dir.path()).is_empty());
}

#[tokio::test]
async fn elevated_run_exports_into_target_dir_only() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    let target = dir.path().join("backup").join("system").join(ID);
    let (exporter, runner) = exporter(export_runner(0));

    let result = run(&exporter, &env, &target, true, &CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(
        runner.calls(),
        [(
            PNPUTIL_EXE.to_owned(),
            vec![
                "/export-driver".to_owned(),
                "*".to_owned(),
                target.to_string_lossy().into_owned()
            ]
        )]
    );
    let names: Vec<_> = result.files.iter().map(|f| f.rel_path.clone()).collect();
    let expected: Vec<PathBuf> = [
        ("oem1.inf_amd64_aa", "driver.sys"),
        ("oem1.inf_amd64_aa", "oem1.inf"),
        ("oem2.inf_amd64_bb", "driver.sys"),
        ("oem2.inf_amd64_bb", "oem2.inf"),
    ]
    .iter()
    .map(|(d, f)| Path::new(d).join(f))
    .collect();
    assert_eq!(names, expected);
    assert_eq!(result.files[0].bytes, 2);
    assert_eq!(
        result.files[0].blake3,
        blake3::hash(b"MZ").to_hex().as_str()
    );
    assert!(result.warnings.is_empty());
    assert_eq!(result.restore_hint, restore_hint());
    // P1: nothing outside the target folder.
    let under_target: Vec<_> = expected
        .iter()
        .map(|p| Path::new("backup").join("system").join(ID).join(p))
        .collect();
    assert_eq!(files_under(dir.path()), under_target);
}

#[tokio::test]
async fn exit_codes_and_empty_exports() {
    let dir = temp_dir();
    let mut env = Environment::fake(dir.path());
    env.is_elevated = true;
    let cancel = CancellationToken::new();

    let (partly, _) = exporter(export_runner(5));
    let result = run(&partly, &env, &dir.path().join("a"), false, &cancel)
        .await
        .unwrap();
    assert_eq!(result.files.len(), 4);
    assert_eq!(result.warnings.len(), 1);
    assert_eq!(result.warnings[0].message_key, ISSUE_EXIT_CODE);
    assert_eq!(result.warnings[0].message_args["code"], "5");

    let (nothing, _) = exporter(FakeCmdRunner::new().output(PNPUTIL_EXE, 0, ""));
    let err = run(&nothing, &env, &dir.path().join("b"), false, &cancel)
        .await
        .unwrap_err();
    assert!(matches!(err, ExportError::NotAvailable), "{err:?}");

    let (failed, _) = exporter(FakeCmdRunner::new().output(PNPUTIL_EXE, 2, "access denied"));
    let err = run(&failed, &env, &dir.path().join("c"), false, &cancel)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, ExportError::ProcessFailed { code: 2, stderr } if stderr == "access denied"),
        "{err:?}"
    );

    let (slow, _) = exporter(FakeCmdRunner::new().on(PNPUTIL_EXE, |_| Err(ExportError::Timeout)));
    let err = run(&slow, &env, &dir.path().join("d"), false, &cancel)
        .await
        .unwrap_err();
    assert!(matches!(err, ExportError::Timeout), "{err:?}");

    cancel.cancel();
    let (cancelled, runner) = exporter(export_runner(0));
    let err = run(&cancelled, &env, &dir.path().join("e"), false, &cancel)
        .await
        .unwrap_err();
    assert!(matches!(err, ExportError::Cancelled), "{err:?}");
    assert!(runner.calls().is_empty());
}

/// `pnputil /enum-drivers` (no administrator rights needed) answers within the
/// `detect` budget; the export itself is never started here.
#[cfg(windows)]
#[test]
fn detect_on_this_machine_is_fast() {
    let env = Environment::detect().unwrap();
    let started = std::time::Instant::now();
    let avail = DriversExporter::new().detect(&env);
    let elapsed = started.elapsed();
    assert!(elapsed < Duration::from_millis(700), "{elapsed:?}");
    if env.is_elevated {
        assert!(matches!(avail, Availability::Available { .. }), "{avail:?}");
    } else {
        assert!(
            matches!(avail, Availability::NeedsElevation { .. }),
            "{avail:?}"
        );
    }
}
