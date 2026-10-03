use super::decode::{decode, decode_code_page, OutputEncoding};
use super::*;

/// `schtasks /query /fo csv /nh` on ru-RU, as written in cp866 (bytes produced by
/// .NET `Encoding.GetEncoding(866)`).
const SCHTASKS_RU_CP866: &[u8] =
    b"\x22\x5c\x8e\xa1\xad\xae\xa2\xab\xa5\xad\xa8\xa5 \xf0\xab\xaa\xa8\x22,\
\x2205.10.2026 3:00:00\x22,\x22\x83\xae\xe2\xae\xa2\xae\x22\x0d\x0a\
\x22\x5c\x90\xa5\xa7\xa5\xe0\xa2\xad\xa0\xef \xaa\xae\xaf\xa8\xef\x22,\x22\x8d/\x84\x22,\
\x22\x8e\xe2\xaa\xab\xee\xe7\xa5\xad\xae\x22\x0d\x0a";

const SCHTASKS_RU: &str = "\"\\Обновление Ёлки\",\"05.10.2026 3:00:00\",\"Готово\"\r\n\
\"\\Резервная копия\",\"Н/Д\",\"Отключено\"\r\n";

/// The same on en-US, as written in cp437 (bytes produced by .NET `GetEncoding(437)`).
const SCHTASKS_EN_CP437: &[u8] = b"\x22\x5cCaf\x82 Gr\x94\xe1e\x22,\x2210/5/2026 3:00:00 AM\x22,\
\x22Ready\x22\x0d\x0a\x22\x5cBackup \xab\x22,\x22N/A\x22,\x22Disabled\x22\x0d\x0a";

const SCHTASKS_EN: &str = "\"\\Café Größe\",\"10/5/2026 3:00:00 AM\",\"Ready\"\r\n\
\"\\Backup ½\",\"N/A\",\"Disabled\"\r\n";

#[test]
fn decodes_cp866_schtasks_output() {
    assert_eq!(decode_code_page(SCHTASKS_RU_CP866, Some(866)), SCHTASKS_RU);
}

#[test]
fn decodes_cp437_schtasks_output() {
    assert_eq!(decode_code_page(SCHTASKS_EN_CP437, Some(437)), SCHTASKS_EN);
}

#[test]
fn cp437_table_box_drawing_and_nbsp() {
    assert_eq!(
        decode_code_page(b"\xc9\xcd\xbb\xff", Some(437)),
        "╔═╗\u{00A0}"
    );
}

#[test]
fn decodes_other_code_pages_known_to_encoding_rs() {
    // "Пароль" in windows-1251.
    assert_eq!(
        decode_code_page(b"\xcf\xe0\xf0\xee\xeb\xfc", Some(1251)),
        "Пароль"
    );
}

#[test]
fn unknown_or_missing_code_page_falls_back_to_lossy_utf8() {
    let utf8 = "Größe".as_bytes();
    assert_eq!(decode_code_page(utf8, Some(850)), "Größe");
    assert_eq!(decode_code_page(utf8, None), "Größe");
    assert_eq!(decode_code_page(b"bad \xff", None), "bad \u{FFFD}");
}

#[test]
fn ascii_is_the_same_in_every_code_page() {
    assert_eq!(decode_code_page(b"Ready\r\n", Some(866)), "Ready\r\n");
    assert_eq!(decode_code_page(b"Ready\r\n", Some(437)), "Ready\r\n");
}

#[test]
fn utf8_output_drops_bom() {
    assert_eq!(
        decode(b"\xEF\xBB\xBF{\"a\":1}", OutputEncoding::Utf8),
        "{\"a\":1}"
    );
}

#[tokio::test]
async fn read_limited_keeps_limit_and_flags_rest() -> std::io::Result<()> {
    let data = [b'x'; 100];
    let (kept, truncated) = read_limited(Some(&data[..]), 30).await?;
    assert_eq!(kept.len(), 30);
    assert!(truncated);

    let (kept, truncated) = read_limited(Some(&data[..]), 100).await?;
    assert_eq!(kept.len(), 100);
    assert!(!truncated);

    let (kept, truncated) = read_limited(None::<&[u8]>, 10).await?;
    assert!(kept.is_empty() && !truncated);
    Ok(())
}

#[test]
fn log_command_keeps_raw_paths() {
    // Single backslashes: the log writer replaces known-folder paths by raw matching.
    let cmd = Cmd::new(r"C:\Windows\System32\reg.exe").args([
        "export",
        r"HKCU\Console",
        r"C:\Users\max\out.reg",
    ]);
    assert_eq!(
        cmd.log_command(),
        r"C:\Windows\System32\reg.exe export HKCU\Console C:\Users\max\out.reg"
    );
}

#[test]
fn builder_collects_program_args_and_defaults() {
    let cmd = Cmd::new(r"C:\Windows\System32\reg.exe")
        .args(["export", r"HKCU\Console"])
        .args([OsStr::new("out.reg")]);
    assert_eq!(
        cmd.get_program(),
        OsStr::new(r"C:\Windows\System32\reg.exe")
    );
    assert_eq!(cmd.get_args(), ["export", r"HKCU\Console", "out.reg"]);
    assert_eq!(cmd.timeout, DEFAULT_TIMEOUT);
    assert_eq!(cmd.encoding, OutputEncoding::Oem);
}

#[tokio::test]
async fn cancelled_token_does_not_start_the_process() {
    let cancel = CancellationToken::new();
    cancel.cancel();
    let mut spawned = false;
    let result = Cmd::new("definitely-not-a-program-sk")
        .run_observed(&cancel, |_| spawned = true)
        .await;
    assert!(matches!(result, Err(ExportError::Cancelled)));
    assert!(!spawned);
}

#[tokio::test]
async fn missing_program_is_io_error() {
    let result = Cmd::new("definitely-not-a-program-sk")
        .run(&CancellationToken::new())
        .await;
    assert!(matches!(result, Err(ExportError::Io(_))));
}

#[tokio::test]
async fn fake_runner_answers_by_file_name_and_records_calls() {
    let fake = FakeCmdRunner::new().output("schtasks.exe", 0, "\"\\Task\",\"N/A\",\"Ready\"");
    let runner: &dyn CmdRunner = &fake;
    let cancel = CancellationToken::new();

    let cmd = Cmd::new(r"C:\Windows\System32\SCHTASKS.EXE").args(["/query", "/fo", "csv"]);
    let out = runner.run(cmd, &cancel).await;
    assert!(matches!(&out, Ok(o) if o.code == 0 && o.stdout.contains("Task")));

    let missing = runner.run(Cmd::new("netsh.exe"), &cancel).await;
    assert!(matches!(missing, Err(ExportError::Io(_))));

    assert_eq!(
        fake.calls(),
        vec![
            (
                "schtasks.exe".to_owned(),
                vec!["/query".to_owned(), "/fo".to_owned(), "csv".to_owned()]
            ),
            ("netsh.exe".to_owned(), Vec::new()),
        ]
    );
}

#[cfg(windows)]
mod windows {
    //! Real processes; all of them are harmless (`ping` to localhost, `cmd /c`) and are
    //! killed by the tests themselves.

    use std::path::PathBuf;
    use std::process::Command as StdCommand;

    use super::*;

    /// The tests count `PING.EXE` processes, so they must not overlap.
    static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    fn system32(exe: &str) -> PathBuf {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
        PathBuf::from(root).join("System32").join(exe)
    }

    fn system32_str(exe: &str) -> String {
        system32(exe).to_string_lossy().into_owned()
    }

    /// Pids of running processes matching the `tasklist` filter, from its CSV output
    /// (`"PING.EXE","1234",...`); locale-independent. A failing `tasklist` fails the
    /// test, so an empty list really means "no such process".
    fn pids_of(filter: &str) -> Vec<u32> {
        let output = StdCommand::new(system32("tasklist.exe"))
            .args(["/FI", filter, "/FO", "CSV", "/NH"])
            .output();
        let output = match output {
            Ok(output) if output.status.success() => output,
            other => panic!("tasklist /FI \"{filter}\" failed: {other:?}"),
        };
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| line.split("\",\"").nth(1)?.parse().ok())
            .collect()
    }

    fn alive(pid: u32) -> bool {
        pids_of(&format!("PID eq {pid}")).contains(&pid)
    }

    fn ping_pids() -> Vec<u32> {
        pids_of("IMAGENAME eq PING.EXE")
    }

    #[tokio::test]
    async fn timeout_kills_process() {
        let _serial = SERIAL.lock().await;
        let timeout = Duration::from_secs(1);
        let mut started = Instant::now();
        let mut pid = None;
        let mut alive_at_start = false;
        let result = Cmd::new(&system32_str("PING.EXE"))
            .args(["-n", "30", "127.0.0.1"])
            .timeout(timeout)
            .run_observed(&CancellationToken::new(), |p| {
                pid = Some(p);
                // Proves that `alive` sees the running process at all.
                alive_at_start = alive(p);
                // The timeout clock starts after this callback.
                started = Instant::now();
            })
            .await;
        let elapsed = started.elapsed();

        assert!(alive_at_start, "ping {pid:?} was not visible to tasklist");
        assert!(matches!(result, Err(ExportError::Timeout)), "{result:?}");
        assert!(elapsed <= timeout + Duration::from_secs(1), "{elapsed:?}");
        let pid = pid.unwrap_or_default();
        assert!(pid != 0 && !alive(pid), "ping {pid} is still running");
    }

    #[tokio::test]
    async fn cancel_kills_process_tree() {
        let _serial = SERIAL.lock().await;
        let before = ping_pids();
        let cancel = CancellationToken::new();
        let mut cmd_pid = None;
        let cmd = Cmd::new(&system32_str("cmd.exe"))
            .args(["/c", &system32_str("PING.EXE"), "-n", "30", "127.0.0.1"])
            .run_observed(&cancel, |p| cmd_pid = Some(p));
        let canceller = async {
            // Give cmd.exe time to start ping, remember the grandchild, then cancel.
            let mut grandchildren = Vec::new();
            for _ in 0..20 {
                tokio::time::sleep(Duration::from_millis(250)).await;
                grandchildren = ping_pids()
                    .into_iter()
                    .filter(|p| !before.contains(p))
                    .collect();
                if !grandchildren.is_empty() {
                    break;
                }
            }
            cancel.cancel();
            (grandchildren, Instant::now())
        };
        let (result, (grandchildren, cancelled_at)) = tokio::join!(cmd, canceller);

        assert!(matches!(result, Err(ExportError::Cancelled)), "{result:?}");
        assert!(cancelled_at.elapsed() <= Duration::from_secs(2));
        assert!(!grandchildren.is_empty(), "ping did not start");
        let cmd_pid = cmd_pid.unwrap_or_default();
        assert!(
            cmd_pid != 0 && !alive(cmd_pid),
            "cmd.exe {cmd_pid} is still running"
        );
        for pid in grandchildren {
            assert!(!alive(pid), "ping {pid} survived the cancellation");
        }
    }

    #[tokio::test]
    async fn collects_exit_code_and_output() -> Result<(), ExportError> {
        let out = Cmd::new(&system32_str("cmd.exe"))
            .args(["/c", "echo hello& echo oops 1>&2& exit 3"])
            .run(&CancellationToken::new())
            .await?;
        assert_eq!(out.code, 3);
        assert_eq!(out.stdout.trim_end(), "hello");
        assert_eq!(out.stderr.trim_end(), "oops");
        assert!(!out.truncated);
        Ok(())
    }

    #[tokio::test]
    async fn applies_cwd_and_env() -> Result<(), ExportError> {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let out = Cmd::new(&system32_str("cmd.exe"))
            .args(["/c", "cd& echo %SK_CMD_TEST%"])
            .cwd(&dir)
            .env("SK_CMD_TEST", "value-42")
            .run(&CancellationToken::new())
            .await?;
        let mut lines = out.stdout.lines();
        let cwd = lines.next().unwrap_or_default();
        assert_eq!(
            cwd.trim_end_matches('\\').to_lowercase(),
            dir.to_string_lossy().trim_end_matches('\\').to_lowercase()
        );
        assert_eq!(lines.next(), Some("value-42"));
        Ok(())
    }
}
