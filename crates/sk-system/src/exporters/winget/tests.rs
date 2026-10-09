use std::path::{Path, PathBuf};
use std::sync::Arc;

use sk_core::env::{Environment, KnownFolder};
use sk_core::model::{AppKind, Category, EvidenceSource, FindingId, Sensitivity, Target};
use sk_core::CancellationToken;

use super::*;
use crate::cmd::FakeCmdRunner;
use crate::exporters::test_util::{files_under, run_export, temp_dir};

/// A fake machine under `root` with `winget.exe` in `{LOCALAPPDATA}\Microsoft\WindowsApps`.
fn env_with_winget(root: &Path) -> Environment {
    let env = Environment::fake(root);
    let apps = windows_apps(&env);
    std::fs::create_dir_all(&apps).unwrap();
    std::fs::write(apps.join(EXE), b"").unwrap();
    env
}

fn windows_apps(env: &Environment) -> PathBuf {
    env.known_folder(KnownFolder::LocalAppData)
        .unwrap()
        .join("Microsoft")
        .join("WindowsApps")
}

fn exporter(runner: FakeCmdRunner) -> (WingetExporter, Arc<FakeCmdRunner>) {
    let runner = Arc::new(runner);
    let exporter = WingetExporter::with_runner(Arc::clone(&runner) as Arc<dyn CmdRunner>, "");
    (exporter, runner)
}

fn version_runner(code: i32, stdout: &str) -> FakeCmdRunner {
    FakeCmdRunner::new().output(EXE, code, stdout)
}

/// A runner whose `winget export` writes `content` to the `-o` file and exits with `code`.
fn export_runner(code: i32, content: &'static [u8], stdout: &'static str) -> FakeCmdRunner {
    FakeCmdRunner::new().on(EXE, move |cmd| {
        let args = cmd.get_args();
        let out = args
            .iter()
            .position(|a| a == "-o")
            .and_then(|i| args.get(i + 1))
            .unwrap();
        std::fs::write(out, content).unwrap();
        Ok(CmdOutput {
            code,
            stdout: stdout.to_owned(),
            ..CmdOutput::default()
        })
    })
}

#[test]
fn parses_winget_versions() {
    assert_eq!(
        parse_version("v1.9.25200\r\n"),
        Some(("1.9.25200".to_owned(), (1, 9)))
    );
    assert_eq!(
        parse_version("\n 1.4.10173 "),
        Some(("1.4.10173".to_owned(), (1, 4)))
    );
    assert_eq!(
        parse_version("v1.10.40-preview"),
        Some(("1.10.40-preview".to_owned(), (1, 10)))
    );
    assert_eq!(parse_version(""), None);
    assert_eq!(parse_version("v1"), None);
    assert_eq!(parse_version("Windows Package Manager"), None);
}

#[test]
fn locates_winget_in_windows_apps_then_absolute_path_entries() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join(EXE), b"").unwrap();
    let path_var = std::env::join_paths([dir.path().join("empty"), bin.clone()]).unwrap();

    assert_eq!(locate(&env, None), None);
    assert_eq!(locate(&env, Some(&path_var)), Some(bin.join(EXE)));
    // Relative PATH entries resolve against the working folder: never used.
    assert_eq!(locate(&env, Some(OsStr::new("bin"))), None);

    let env = env_with_winget(dir.path());
    assert_eq!(
        locate(&env, Some(&path_var)),
        Some(windows_apps(&env).join(EXE))
    );
}

#[test]
fn folder_named_winget_exe_is_not_winget() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    std::fs::create_dir_all(windows_apps(&env).join(EXE)).unwrap();
    assert_eq!(locate(&env, None), None);
}

#[test]
fn probe_reports_version_of_supported_winget() {
    let dir = temp_dir();
    let mut env = env_with_winget(dir.path());
    env.installed_programs = Vec::new();
    let (exporter, runner) = exporter(version_runner(0, "v1.9.25200\r\n"));

    let avail = exporter.probe(&env);
    assert_eq!(
        avail,
        Availability::Available {
            estimated_bytes: Some(ESTIMATE_BASE_BYTES),
            details: BTreeMap::from([("version".to_owned(), "1.9.25200".to_owned())]),
        }
    );
    assert_eq!(
        runner.calls(),
        [(EXE.to_owned(), vec!["--version".to_owned()])]
    );
}

#[test]
fn probe_without_winget_is_missing() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    let (exporter, runner) = exporter(version_runner(0, "v1.9.25200"));
    assert_eq!(exporter.probe(&env), unavailable(MISSING));
    assert!(runner.calls().is_empty(), "nothing is started");
}

#[test]
fn probe_of_old_or_broken_winget() {
    let dir = temp_dir();
    let env = env_with_winget(dir.path());
    let cases = [
        (version_runner(0, "v1.3.2691"), OUTDATED),
        (version_runner(0, "v0.9.1"), OUTDATED),
        (version_runner(1, "v1.9.25200"), MISSING),
        (version_runner(0, "not a version"), MISSING),
        (
            FakeCmdRunner::new().on(EXE, |_| Err(ExportError::Timeout)),
            MISSING,
        ),
    ];
    for (runner, reason) in cases {
        let (exporter, _) = exporter(runner);
        assert_eq!(exporter.probe(&env), unavailable(reason));
    }
}

#[tokio::test]
async fn probe_works_inside_a_runtime() {
    let dir = temp_dir();
    let env = env_with_winget(dir.path());
    let (exporter, _) = exporter(version_runner(0, "v1.4.0"));
    assert!(matches!(
        exporter.probe(&env),
        Availability::Available { .. }
    ));
}

#[cfg(not(windows))]
#[test]
fn detect_off_windows_is_not_windows() {
    let dir = temp_dir();
    let env = env_with_winget(dir.path());
    let (exporter, runner) = exporter(version_runner(0, "v1.9.25200"));
    assert_eq!(exporter.detect(&env), unavailable(NOT_WINDOWS));
    assert!(runner.calls().is_empty());
}

#[test]
fn plan_is_system_settings_finding() {
    let env = Environment::fake(Path::new("/fake"));
    let exporter = WingetExporter::new();
    let avail = Availability::Available {
        estimated_bytes: Some(1000),
        details: BTreeMap::from([("version".to_owned(), "1.9.25200".to_owned())]),
    };
    let finding = exporter.plan(&env, &avail).unwrap();

    let target = Target::SystemExport {
        exporter_id: "winget".to_owned(),
        params: serde_json::json!({}),
    };
    assert_eq!(finding.id, FindingId::for_target(&target));
    assert_eq!(finding.target, target);
    assert_eq!(finding.category, Category::SystemSettings);
    assert_eq!(finding.sensitivity, Sensitivity::None);
    assert!(!finding.requires_elevation);
    assert_eq!(finding.title, TITLE);
    assert_eq!(finding.app.as_ref().map(|a| a.kind), Some(AppKind::System));
    let evidence = &finding.evidence[0];
    assert_eq!(
        evidence.source,
        EvidenceSource::System {
            exporter_id: "winget".to_owned()
        }
    );
    assert_eq!(evidence.message_args["version"], "1.9.25200");

    let elevated = exporter
        .plan(
            &env,
            &Availability::NeedsElevation {
                estimated_bytes: None,
            },
        )
        .unwrap();
    assert!(elevated.requires_elevation);
    assert!(exporter.plan(&env, &unavailable(MISSING)).is_none());
}

#[tokio::test]
async fn run_exports_into_target_dir_only() {
    const JSON: &[u8] = br#"{"Sources":[{"Packages":[{"PackageIdentifier":"Git.Git"}]}]}"#;
    let dir = temp_dir();
    let env = env_with_winget(&dir.path().join("machine"));
    let target = dir.path().join("backup").join("system").join("winget");
    let (exporter, runner) = exporter(export_runner(0, JSON, ""));

    let result = run_export(&exporter, &env, &target, &CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(result.files.len(), 1);
    let file = &result.files[0];
    assert_eq!(file.rel_path, PathBuf::from(EXPORT_FILE));
    assert_eq!(file.bytes, JSON.len() as u64);
    assert_eq!(file.blake3, blake3::hash(JSON).to_hex().to_string());
    assert!(result.warnings.is_empty());
    assert_eq!(result.restore_hint, restore_hint());

    let out = target.join(EXPORT_FILE).to_string_lossy().into_owned();
    let expected_args = [
        "export",
        "-o",
        &out,
        "--include-versions",
        "--accept-source-agreements",
        "--disable-interactivity",
    ];
    assert_eq!(
        runner.calls(),
        [(EXE.to_owned(), expected_args.map(str::to_owned).to_vec())]
    );
    // P1: nothing written outside the target folder.
    assert_eq!(
        files_under(&dir.path().join("backup")),
        [PathBuf::from("system").join("winget").join(EXPORT_FILE)]
    );
}

#[tokio::test]
async fn non_zero_exit_with_export_file_is_a_warning() {
    let dir = temp_dir();
    let env = env_with_winget(dir.path());
    let target = dir.path().join("out");
    let (exporter, _) = exporter(export_runner(-1_978_335_212, b"{}", ""));

    let result = run_export(&exporter, &env, &target, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.files.len(), 1);
    assert_eq!(result.warnings.len(), 1);
    assert_eq!(result.warnings[0].message_key, ISSUE_EXIT_CODE);
    assert_eq!(result.warnings[0].message_args["code"], "-1978335212");
}

#[tokio::test]
async fn missing_or_empty_export_file_is_process_failure() {
    let dir = temp_dir();
    let env = env_with_winget(dir.path());
    let target = dir.path().join("out");

    let runner = FakeCmdRunner::new().on(EXE, |_| {
        Ok(CmdOutput {
            code: 5,
            stdout: "Failed to export\r\n".to_owned(),
            ..CmdOutput::default()
        })
    });
    let (failing, _) = exporter(runner);
    let result = run_export(&failing, &env, &target, &CancellationToken::new()).await;
    assert!(
        matches!(&result, Err(ExportError::ProcessFailed { code: 5, stderr }) if stderr == "Failed to export"),
        "{result:?}"
    );

    let (empty, _) = exporter(export_runner(0, b"", "done"));
    let result = run_export(&empty, &env, &target, &CancellationToken::new()).await;
    assert!(
        matches!(&result, Err(ExportError::ProcessFailed { code: 0, .. })),
        "{result:?}"
    );
}

#[tokio::test]
async fn run_without_winget_is_not_available() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    let target = dir.path().join("out");
    let (exporter, runner) = exporter(export_runner(0, b"{}", ""));

    let result = run_export(&exporter, &env, &target, &CancellationToken::new()).await;
    assert!(
        matches!(result, Err(ExportError::NotAvailable)),
        "{result:?}"
    );
    assert!(runner.calls().is_empty());
    assert!(!target.exists());
}

#[tokio::test]
async fn timeout_and_cancellation_are_passed_on() {
    let dir = temp_dir();
    let env = env_with_winget(dir.path());
    let target = dir.path().join("out");

    let (hanging, _) = exporter(FakeCmdRunner::new().on(EXE, |_| Err(ExportError::Timeout)));
    let result = run_export(&hanging, &env, &target, &CancellationToken::new()).await;
    assert!(matches!(result, Err(ExportError::Timeout)), "{result:?}");

    let (exporter, _) = exporter(export_runner(0, b"{}", ""));
    let cancel = CancellationToken::new();
    cancel.cancel();
    let result = run_export(&exporter, &env, &target, &cancel).await;
    assert!(matches!(result, Err(ExportError::Cancelled)), "{result:?}");
    assert!(!target.join(EXPORT_FILE).exists());
}

/// Manual run on a machine with winget (SPEC-06 §6): detection only.
#[cfg(windows)]
#[ignore = "depends on the machine: needs winget"]
#[test]
fn real_winget_detect() {
    let env = Environment::detect().unwrap();
    let avail = WingetExporter::new().detect(&env);
    assert!(
        matches!(&avail, Availability::Available { details, .. } if details.contains_key("version")),
        "{avail:?}"
    );
}

/// Manual run on a machine with winget (SPEC-06 §6): exports into `target/tmp`.
#[cfg(windows)]
#[ignore = "depends on the machine: needs winget and network"]
#[tokio::test]
async fn real_winget_export() {
    let env = Environment::detect().unwrap();
    let exporter = WingetExporter::new();
    let avail = exporter.detect(&env);
    assert!(matches!(avail, Availability::Available { .. }), "{avail:?}");

    let dir = temp_dir();
    let target = dir.path().join("winget");
    let result = run_export(&exporter, &env, &target, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.files[0].rel_path, PathBuf::from(EXPORT_FILE));
    let bytes = std::fs::read(target.join(EXPORT_FILE)).unwrap();
    let text = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    let json: serde_json::Value = serde_json::from_slice(text).unwrap();
    assert!(json.get("Sources").is_some(), "{json}");
}
