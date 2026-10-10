//! `WifiExporter::run`: keys, warnings, failures and principle P1.

use super::*;

#[tokio::test]
async fn run_without_keys_removes_them_and_writes_only_target_dir() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    let target = dir.path().join("backup").join("system").join(ID);
    let (exporter, runner) = fake_exporter(export_runner(0));
    let cancel = CancellationToken::new();

    let result = run_with(&exporter, &env, &target, default_params(), &cancel)
        .await
        .unwrap();

    let netsh = windir(&env).unwrap().join(r"System32\netsh.exe");
    let folder = format!("folder={}", target.display());
    assert_eq!(
        runner.calls(),
        [(
            NETSH_EXE.to_owned(),
            vec![
                "wlan".to_owned(),
                "export".to_owned(),
                "profile".to_owned(),
                folder
            ]
        )]
    );
    assert!(netsh_exe(&env).is_some_and(|exe| exe == netsh));
    let names: Vec<_> = result.files.iter().map(|f| f.rel_path.clone()).collect();
    assert_eq!(
        names,
        ["Wi-Fi-HomeNet.xml", "Wi-Fi-Office.xml"].map(PathBuf::from)
    );
    for (file, ssid) in result.files.iter().zip(["HomeNet", "Office"]) {
        let content = read(&target.join(&file.rel_path));
        assert_eq!(content, profile_without_key(ssid));
        assert!(!content.contains("keyMaterial"));
        assert_eq!(file.bytes, content.len() as u64);
        assert_eq!(
            file.blake3,
            blake3::hash(content.as_bytes()).to_hex().as_str()
        );
    }
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_eq!(
        result.restore_hint,
        RestoreHint::Manual {
            key: RESTORE_KEY.to_owned()
        }
    );
    // P1: nothing outside the target folder in the whole temporary folder.
    assert_eq!(
        files_under(dir.path()),
        names
            .iter()
            .map(|n| Path::new("backup").join("system").join(ID).join(n))
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn run_with_keys_keeps_clear_keys_and_warns_about_protected_ones() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    let target = dir.path().join(ID);
    let (exporter, runner) = fake_exporter(export_runner(0));
    let params = serde_json::json!({ "include_keys": true });

    let result = run_with(&exporter, &env, &target, params, &CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(runner.calls()[0].1.last().unwrap(), "key=clear");
    assert_eq!(result.files.len(), 2);
    assert_eq!(
        read(&target.join("Wi-Fi-HomeNet.xml")),
        profile_xml("HomeNet", false, "secret-password")
    );
    assert_eq!(
        read(&target.join("Wi-Fi-Office.xml")),
        profile_xml("Office", true, ENCRYPTED)
    );
    assert_eq!(
        result.warnings,
        [warning(
            ISSUE_KEY_PROTECTED,
            BTreeMap::from([("profile".to_owned(), "Wi-Fi-Office.xml".to_owned())])
        )]
    );
}

#[tokio::test]
async fn non_zero_exit_with_profiles_is_a_warning() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    let target = dir.path().join(ID);
    let (exporter, _) = fake_exporter(export_runner(1));
    let result = run_default(&exporter, &env, &target).await.unwrap();
    assert_eq!(result.files.len(), 2);
    assert_eq!(
        result.warnings,
        [warning(
            ISSUE_EXIT_CODE,
            BTreeMap::from([("code".to_owned(), "1".to_owned())])
        )]
    );
}

#[tokio::test]
async fn no_profiles_is_not_available_or_process_failure() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());

    let (exporter, _) = fake_exporter(FakeCmdRunner::new().output(NETSH_EXE, 0, ""));
    let result = run_default(&exporter, &env, &dir.path().join("a")).await;
    assert!(
        matches!(result, Err(ExportError::NotAvailable)),
        "{result:?}"
    );

    let runner = FakeCmdRunner::new().on(NETSH_EXE, |_| {
        Ok(CmdOutput {
            code: 1,
            stdout: "The Wireless AutoConfig Service (wlansvc) is not running.".to_owned(),
            ..CmdOutput::default()
        })
    });
    let (exporter, _) = fake_exporter(runner);
    let result = run_default(&exporter, &env, &dir.path().join("b")).await;
    assert!(
        matches!(&result, Err(ExportError::ProcessFailed { code: 1, stderr }) if stderr.contains("wlansvc")),
        "{result:?}"
    );
}

#[tokio::test]
async fn run_without_windows_folder_fails_before_starting() {
    let dir = temp_dir();
    let mut env = Environment::fake(dir.path());
    env.known_folders.remove(&KnownFolder::WinDir);
    let (exporter, runner) = fake_exporter(export_runner(0));
    let target = dir.path().join(ID);
    let result = run_default(&exporter, &env, &target).await;
    assert!(matches!(result, Err(ExportError::Io(_))), "{result:?}");
    assert!(runner.calls().is_empty());
    assert!(!target.exists());
}

#[tokio::test]
async fn cancellation_before_the_run_starts_nothing() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    let target = dir.path().join(ID);
    let (exporter, runner) = fake_exporter(export_runner(0));
    let cancel = CancellationToken::new();
    cancel.cancel();
    let result = run_with(&exporter, &env, &target, default_params(), &cancel).await;
    assert!(matches!(result, Err(ExportError::Cancelled)), "{result:?}");
    assert!(runner.calls().is_empty());
    assert!(!target.exists());
}

#[tokio::test]
async fn interrupted_export_leaves_no_keys() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    let target = dir.path().join(ID);
    // netsh wrote a profile, then the run was cancelled or timed out.
    for error in [ExportError::Cancelled, ExportError::Timeout] {
        let failing = matches!(error, ExportError::Timeout);
        let runner = FakeCmdRunner::new().on(NETSH_EXE, move |cmd| {
            let file = export_folder(cmd).join("Wi-Fi-HomeNet.xml");
            std::fs::write(file, profile_xml("HomeNet", true, ENCRYPTED)).unwrap();
            Err(if failing {
                ExportError::Timeout
            } else {
                ExportError::Cancelled
            })
        });
        let (exporter, _) = fake_exporter(runner);
        let result = run_default(&exporter, &env, &target).await;
        assert_eq!(
            std::mem::discriminant(&result.unwrap_err()),
            std::mem::discriminant(&error)
        );
        assert_eq!(
            read(&target.join("Wi-Fi-HomeNet.xml")),
            profile_without_key("HomeNet")
        );
    }
}

/// SPEC-06 §8: `include_keys=false` → no `<keyMaterial>` in the real export. Writes the
/// Wi-Fi profiles of this machine (without keys) under `target/tmp`.
#[cfg(windows)]
#[ignore = "depends on the machine: needs saved Wi-Fi profiles"]
#[tokio::test]
async fn real_wifi_export_without_keys() {
    let env = Environment::detect().unwrap();
    let exporter = WifiExporter::new();
    println!("detect: {:?}", exporter.detect(&env));
    let dir = temp_dir();
    let target = dir.path().join(ID);
    let result = run_default(&exporter, &env, &target).await.unwrap();
    assert!(!result.files.is_empty());
    for file in &result.files {
        let content = std::fs::read(target.join(&file.rel_path)).unwrap();
        let text = String::from_utf8_lossy(&content);
        assert!(!text.contains("keyMaterial"), "{}", file.rel_path.display());
        assert!(!keys::has_protected_key(&content));
    }
}
