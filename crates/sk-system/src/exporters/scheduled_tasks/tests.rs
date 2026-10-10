use std::path::{Path, PathBuf};

use sk_core::model::Sensitivity;
use sk_core::CancellationToken;

use super::*;
use crate::cmd::FakeCmdRunner;
use crate::exporters::test_util::{assert_system_settings_plan, files_under, run_export, temp_dir};

/// `schtasks /query /fo csv /nh` of a machine (columns after the name are localized).
const CSV: &str = "\r\n\
\"\\Adobe Acrobat Update Task\",\"10.10.2026 23:00:00\",\"Готово\"\r\n\
\"\\Adobe Acrobat Update Task\",\"10.10.2026 23:00:00\",\"Готово\"\r\n\
\"\\Microsoft\\Windows\\Defrag\\ScheduledDefrag\",\"N/A\",\"Готово\"\r\n\
\"\\MICROSOFT\\Office\\Update\",\"N/A\",\"Ready\"\r\n\
\"\\MicrosoftEdgeUpdateTaskMachineCore\",\"N/A\",\"Ready\"\r\n\
\"\\ASUS\\Framework Service\",\"N/A\",\"Готово\"\r\n\
\"\\Обновление Браузера\",\"N/A\",\"Готово\"\r\n\
INFO: some localized message\r\n";

/// Tasks of [`CSV`] outside `\Microsoft\`, in order.
const TASKS: [&str; 4] = [
    r"\Adobe Acrobat Update Task",
    r"\MicrosoftEdgeUpdateTaskMachineCore",
    r"\ASUS\Framework Service",
    r"\Обновление Браузера",
];

/// The XML `schtasks /query /xml /tn <name>` prints for `name`.
fn task_xml(name: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-16\"?>\r\r\n<Task version=\"1.2\">\r\r\n  \
         <RegistrationInfo>\r\r\n    <URI>{name}</URI>\r\r\n  </RegistrationInfo>\r\r\n</Task>\r\r\n"
    )
}

/// A fake `schtasks.exe`: the listing gives `csv` (exit code `code`), `/xml` prints the
/// task XML, except for the tasks in `failing`, which exit with 1.
fn runner(csv: &'static str, code: i32, failing: &'static [&'static str]) -> FakeCmdRunner {
    FakeCmdRunner::new().on(SCHTASKS_EXE, move |cmd| {
        let args: Vec<String> = cmd
            .get_args()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        if args[..2] == ["/query", "/xml"] {
            let name = &args[3];
            if failing.contains(&name.as_str()) {
                return Ok(CmdOutput {
                    code: 1,
                    stderr: "ERROR: The system cannot find the file specified.".to_owned(),
                    ..CmdOutput::default()
                });
            }
            return Ok(CmdOutput {
                stdout: task_xml(name),
                ..CmdOutput::default()
            });
        }
        assert_eq!(args, ["/query", "/fo", "csv", "/nh"]);
        Ok(CmdOutput {
            code,
            stdout: csv.to_owned(),
            ..CmdOutput::default()
        })
    })
}

fn exporter(runner: FakeCmdRunner) -> (ScheduledTasksExporter, Arc<FakeCmdRunner>) {
    let runner = Arc::new(runner);
    let exporter = ScheduledTasksExporter::with_runner(Arc::clone(&runner) as Arc<dyn CmdRunner>);
    (exporter, runner)
}

/// Decodes a UTF-16LE file with BOM.
fn read_utf16(path: &Path) -> String {
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(&bytes[..2], [0xFF, 0xFE]);
    let units: Vec<u16> = bytes[2..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&c| u16::from_le_bytes(c))
        .collect();
    String::from_utf16(&units).unwrap()
}

#[test]
fn plan_is_low_sensitivity_system_settings() {
    assert_system_settings_plan(&ScheduledTasksExporter::new(), TITLE, Sensitivity::Low);
}

#[cfg(not(windows))]
#[test]
fn detect_off_windows_is_not_windows() {
    let env = Environment::fake(Path::new("/fake"));
    assert_eq!(
        ScheduledTasksExporter::new().detect(&env),
        unavailable(NOT_WINDOWS)
    );
}

#[test]
fn user_tasks_skip_microsoft_folder_duplicates_and_other_lines() {
    assert_eq!(user_tasks(CSV), TASKS);
    assert_eq!(
        user_tasks("\"\\Quoted \"\"name\"\"\",\"N/A\"\n\"\\Unclosed\n\"TaskName\",\"Next\"\n"),
        ["\\Quoted \"name\""]
    );
    assert!(user_tasks("").is_empty());
}

#[test]
fn task_files_keep_folders_and_are_valid_unique_names() {
    let mut taken = BTreeSet::new();
    let mut file = |name| task_file(name, &mut taken);
    assert_eq!(file(r"\Adobe Update"), PathBuf::from("Adobe Update.xml"));
    assert_eq!(
        file(r"\ASUS\Framework Service"),
        Path::new("ASUS").join("Framework Service.xml")
    );
    assert_eq!(file(r"\adobe update"), PathBuf::from("adobe update-2.xml"));
    assert_eq!(file(r"\?? Yandex ???"), PathBuf::from("__ Yandex ___.xml"));
    assert_eq!(file(r"\a:b*c."), PathBuf::from("a_b_c.xml"));
    assert_eq!(file(r"\..\x"), Path::new("_").join("x.xml"));
    assert_eq!(file(r"\CON"), PathBuf::from("_CON.xml"));
    assert_eq!(file(r"\com1.task"), PathBuf::from("_com1.task.xml"));
    assert_eq!(file(r"\COM0"), PathBuf::from("COM0.xml"));
    assert_eq!(file(r"\COMЯЯ"), PathBuf::from("COMЯЯ.xml"));
    assert_eq!(file(r"\"), PathBuf::from("_.xml"));
}

#[test]
fn utf16_file_has_bom_and_crlf_line_ends() {
    let bytes = utf16_file("<a>\r\r\nЖ</a>\r\n");
    let text: Vec<u16> = "<a>\r\nЖ</a>\r\n".encode_utf16().collect();
    let mut expected = vec![0xFF, 0xFE];
    expected.extend(text.iter().flat_map(|u| u.to_le_bytes()));
    assert_eq!(bytes, expected);
}

#[test]
fn probe_counts_tasks_outside_microsoft() {
    let env = Environment::fake(Path::new("/fake"));
    let (exporter, runner) = exporter(runner(CSV, 0, &[]));
    assert_eq!(
        exporter.probe(&env),
        Availability::Available {
            estimated_bytes: Some(4 * ESTIMATE_BYTES_PER_TASK),
            details: BTreeMap::from([("tasks".to_owned(), "4".to_owned())]),
        }
    );
    let calls = runner.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, SCHTASKS_EXE);
}

#[test]
fn probe_without_user_tasks_or_with_a_failed_query_is_unavailable() {
    let env = Environment::fake(Path::new("/fake"));
    let only_windows = "\"\\Microsoft\\Windows\\A\",\"N/A\",\"Ready\"\r\n";
    let (exporter, _) = self::exporter(runner(only_windows, 0, &[]));
    assert_eq!(exporter.probe(&env), unavailable(NONE));
    let (exporter, _) = self::exporter(runner(CSV, 1, &[]));
    assert_eq!(exporter.probe(&env), unavailable(QUERY_FAILED));
    let (exporter, _) = self::exporter(FakeCmdRunner::new());
    assert_eq!(exporter.probe(&env), unavailable(QUERY_FAILED));
}

#[test]
fn probe_timeout_is_available_without_details() {
    let env = Environment::fake(Path::new("/fake"));
    let slow = FakeCmdRunner::new().on(SCHTASKS_EXE, |_| Err(ExportError::Timeout));
    let (exporter, _) = exporter(slow);
    assert_eq!(
        exporter.probe(&env),
        Availability::Available {
            estimated_bytes: None,
            details: BTreeMap::new(),
        }
    );
}

#[tokio::test]
async fn run_writes_one_xml_per_user_task_into_target_dir_only() {
    let dir = temp_dir();
    let env = Environment::fake(&dir.path().join("machine"));
    let target = dir.path().join("backup").join("system").join(ID);
    let (exporter, runner) = exporter(runner(CSV, 0, &[]));

    let result = run_export(&exporter, &env, &target, &CancellationToken::new())
        .await
        .unwrap();

    let tasks = Path::new(TASKS_DIR);
    let names: Vec<_> = result.files.iter().map(|f| f.rel_path.clone()).collect();
    assert_eq!(
        names,
        [
            tasks.join("Adobe Acrobat Update Task.xml"),
            tasks.join("MicrosoftEdgeUpdateTaskMachineCore.xml"),
            tasks.join("ASUS").join("Framework Service.xml"),
            tasks.join("Обновление Браузера.xml"),
        ]
    );
    for file in &result.files {
        let bytes = std::fs::read(target.join(&file.rel_path)).unwrap();
        assert_eq!(file.bytes, bytes.len() as u64);
        assert_eq!(file.blake3, blake3::hash(&bytes).to_hex().to_string());
    }
    assert_eq!(
        read_utf16(&target.join(&names[2])),
        task_xml(r"\ASUS\Framework Service").replace("\r\r\n", "\r\n")
    );
    // Not elevated: the note about hidden tasks, no warnings.
    assert_eq!(result.warnings.len(), 1, "{:?}", result.warnings);
    assert_eq!(result.warnings[0].severity, IssueSeverity::Info);
    assert_eq!(result.warnings[0].message_key, ISSUE_HIDDEN);
    assert_eq!(
        result.restore_hint,
        RestoreHint::Manual {
            key: RESTORE_KEY.to_owned()
        }
    );
    let calls = runner.calls();
    assert_eq!(calls.len(), 1 + TASKS.len());
    assert_eq!(calls[2].1, ["/query", "/xml", "/tn", TASKS[1]]);
    // P1: nothing written outside the target folder.
    let mut expected: Vec<_> = names
        .iter()
        .map(|f| PathBuf::from("system").join(ID).join(f))
        .collect();
    expected.sort();
    assert_eq!(files_under(&dir.path().join("backup")), expected);
}

#[tokio::test]
async fn failed_task_is_a_warning_and_elevated_run_has_no_note() {
    let dir = temp_dir();
    let mut env = Environment::fake(dir.path());
    env.is_elevated = true;
    let (exporter, _) = exporter(runner(CSV, 0, &[r"\ASUS\Framework Service"]));

    let result = run_export(
        &exporter,
        &env,
        &dir.path().join("out"),
        &CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(result.files.len(), 3);
    assert_eq!(result.warnings.len(), 1, "{:?}", result.warnings);
    let warning = &result.warnings[0];
    assert_eq!(warning.severity, IssueSeverity::Warning);
    assert_eq!(warning.message_key, ISSUE_TASK_FAILED);
    assert_eq!(warning.message_args["task"], r"\ASUS\Framework Service");
    assert!(
        warning.message_args["error"].contains("cannot find"),
        "{warning:?}"
    );
}

#[tokio::test]
async fn every_task_failing_is_the_first_error() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    let (exporter, _) = exporter(runner(CSV, 0, &TASKS));
    let target = dir.path().join("out");
    let result = run_export(&exporter, &env, &target, &CancellationToken::new()).await;
    assert!(
        matches!(result, Err(ExportError::ProcessFailed { code: 1, .. })),
        "{result:?}"
    );
    assert!(!target.exists());
}

#[tokio::test]
async fn failed_listing_and_no_user_tasks_are_errors() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    let target = dir.path().join("out");
    let (exporter, _) = self::exporter(runner(CSV, 5, &[]));
    let result = run_export(&exporter, &env, &target, &CancellationToken::new()).await;
    assert!(
        matches!(result, Err(ExportError::ProcessFailed { code: 5, .. })),
        "{result:?}"
    );
    let (exporter, _) = self::exporter(runner("", 0, &[]));
    let result = run_export(&exporter, &env, &target, &CancellationToken::new()).await;
    assert!(
        matches!(result, Err(ExportError::NotAvailable)),
        "{result:?}"
    );
    assert!(!target.exists());
}

#[tokio::test]
async fn cancellation_stops_the_export() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    let (exporter, runner) = exporter(runner(CSV, 0, &[]));
    let cancel = CancellationToken::new();
    cancel.cancel();
    let target = dir.path().join("out");
    let result = run_export(&exporter, &env, &target, &cancel).await;
    assert!(matches!(result, Err(ExportError::Cancelled)), "{result:?}");
    assert!(runner.calls().is_empty());
    assert!(!target.exists());
}

/// `detect` on this machine: prints the result and the time (the query may time out).
#[cfg(windows)]
#[test]
fn real_scheduled_tasks_detect() {
    let env = Environment::detect().unwrap();
    let started = std::time::Instant::now();
    let avail = ScheduledTasksExporter::new().detect(&env);
    let elapsed = started.elapsed();
    println!("scheduled-tasks: {avail:?} in {elapsed:?}");
    assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
}

/// The real export of this machine's tasks (one `schtasks` call per task): manual run
/// only, into `target/tmp`.
#[cfg(windows)]
#[tokio::test]
#[ignore = "runs schtasks for every task of this machine"]
async fn real_scheduled_tasks_export() {
    let env = Environment::detect().unwrap();
    let dir = temp_dir();
    let target = dir.path().join(ID);
    let result = run_export(
        &ScheduledTasksExporter::new(),
        &env,
        &target,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    println!("{result:#?}");
    let first = &result.files[0];
    assert!(read_utf16(&target.join(&first.rel_path)).starts_with("<?xml"));
}
