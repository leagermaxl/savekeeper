use std::path::{Path, PathBuf};

use sk_core::model::Sensitivity;
use sk_core::registry::{MemRegistry, RegistryReader};
use sk_core::CancellationToken;

use super::*;
use crate::cmd::{CmdRunner, FakeCmdRunner};
use crate::exporters::partial::ISSUE_KEY_MISSING;
use crate::exporters::test_util::{
    assert_system_settings_plan, files_under, reg_file_content, reg_runner, run_export, temp_dir,
};
use crate::reg_export::REG_EXE;

fn user_values() -> Vec<(&'static str, RegData)> {
    vec![
        (
            "TEMP",
            RegData::ExpandString(r"%USERPROFILE%\AppData\Local\Temp".to_owned()),
        ),
        (
            "Path",
            RegData::ExpandString(r"%USERPROFILE%\bin;C:\Tools".to_owned()),
        ),
        (
            "OneDrive",
            RegData::String(r"C:\Users\Test\OneDrive".to_owned()),
        ),
    ]
}

fn system_values() -> Vec<(&'static str, RegData)> {
    vec![
        ("windir", RegData::ExpandString("%SystemRoot%".to_owned())),
        ("NUMBER_OF_PROCESSORS", RegData::String("8".to_owned())),
    ]
}

/// Adds `hive\key` with `values` to `registry`.
fn add_values(registry: &mut MemRegistry, hive: RegHive, key: &str, values: Vec<(&str, RegData)>) {
    registry.add_key(hive, key);
    for (name, data) in values {
        registry.set_value(hive, key, name, data);
    }
}

/// The exporter whose `reg export` sees `registry_keys` and whose JSON files are read
/// from `values`.
fn exporter(
    runner: FakeCmdRunner,
    registry_keys: &[(RegHive, &str)],
    values: MemRegistry,
) -> (EnvVarsExporter, Arc<FakeCmdRunner>) {
    let mut registry = MemRegistry::new();
    for (hive, key) in registry_keys {
        registry.add_key(*hive, key);
    }
    let registry: Arc<dyn RegistryReader> = Arc::new(registry);
    let runner = Arc::new(runner);
    let reg = RegExport::new(Arc::clone(&runner) as Arc<dyn CmdRunner>, registry);
    (EnvVarsExporter::with(reg, Arc::new(values)), runner)
}

fn full_machine() -> (EnvVarsExporter, Arc<FakeCmdRunner>) {
    let mut values = MemRegistry::new();
    add_values(&mut values, RegHive::Hkcu, USER_KEY, user_values());
    add_values(&mut values, RegHive::Hklm, SYSTEM_KEY, system_values());
    exporter(reg_runner(), &[(RegHive::Hkcu, USER_KEY)], values)
}

fn read_json(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn plan_is_low_sensitivity_system_settings() {
    assert_system_settings_plan(&EnvVarsExporter::new(), TITLE, Sensitivity::Low);
}

#[test]
fn probe_is_always_available() {
    assert!(matches!(
        EnvVarsExporter::probe(),
        Availability::Available { .. }
    ));
}

#[cfg(not(windows))]
#[test]
fn detect_off_windows_is_not_windows() {
    let env = Environment::fake(Path::new("/fake"));
    assert_eq!(
        EnvVarsExporter::new().detect(&env),
        unavailable(NOT_WINDOWS)
    );
}

#[test]
fn json_keeps_raw_values_sorted_by_name() {
    let values = vec![
        RegValue {
            name: "path".to_owned(),
            data: RegData::ExpandString(r"%USERPROFILE%\bin".to_owned()),
        },
        RegValue {
            name: "Count".to_owned(),
            data: RegData::Dword(7),
        },
        RegValue {
            name: "Big".to_owned(),
            data: RegData::Qword(1 << 40),
        },
        RegValue {
            name: "List".to_owned(),
            data: RegData::MultiString(vec!["a".to_owned(), "b".to_owned()]),
        },
        RegValue {
            name: "Blob".to_owned(),
            data: RegData::Binary(vec![1, 2]),
        },
        RegValue {
            name: "Short".to_owned(),
            data: RegData::Other {
                kind: 4,
                bytes: vec![1, 2],
            },
        },
        RegValue {
            name: "Empty".to_owned(),
            data: RegData::Other {
                kind: 0,
                bytes: Vec::new(),
            },
        },
        RegValue {
            name: "Unknown".to_owned(),
            data: RegData::Other {
                kind: 0x1234,
                bytes: vec![1],
            },
        },
        RegValue {
            name: "Name".to_owned(),
            data: RegData::String("Ж".to_owned()),
        },
    ];
    let text = render_json(RegHive::Hkcu, USER_KEY, values);
    assert!(text.ends_with("}\n"));
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        json,
        json!({
            "key": r"HKCU\Environment",
            "variables": [
                { "name": "Big", "type": "REG_QWORD", "value": 1u64 << 40 },
                { "name": "Blob", "type": "REG_BINARY", "value": null },
                { "name": "Count", "type": "REG_DWORD", "value": 7 },
                { "name": "Empty", "type": "REG_NONE", "value": null },
                { "name": "List", "type": "REG_MULTI_SZ", "value": ["a", "b"] },
                { "name": "Name", "type": "REG_SZ", "value": "Ж" },
                { "name": "path", "type": "REG_EXPAND_SZ", "value": r"%USERPROFILE%\bin" },
                { "name": "Short", "type": "REG_DWORD", "value": null },
            ]
        })
    );
}

#[tokio::test]
async fn run_writes_user_json_reg_and_system_json_into_target_dir_only() {
    let dir = temp_dir();
    let env = Environment::fake(&dir.path().join("machine"));
    let target = dir.path().join("backup").join("system").join(ID);
    let (exporter, runner) = full_machine();

    let result = run_export(&exporter, &env, &target, &CancellationToken::new())
        .await
        .unwrap();

    let names: Vec<_> = result.files.iter().map(|f| f.rel_path.clone()).collect();
    assert_eq!(names, [USER_JSON, USER_REG, SYSTEM_JSON].map(PathBuf::from));
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_eq!(
        result.restore_hint,
        RestoreHint::Manual {
            key: RESTORE_KEY.to_owned()
        }
    );
    for file in &result.files {
        let bytes = std::fs::read(target.join(&file.rel_path)).unwrap();
        assert_eq!(file.bytes, bytes.len() as u64);
        assert_eq!(file.blake3, blake3::hash(&bytes).to_hex().to_string());
    }

    let user = read_json(&target.join(USER_JSON));
    assert_eq!(user["key"], r"HKCU\Environment");
    let names: Vec<_> = user["variables"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["OneDrive", "Path", "TEMP"]);
    assert_eq!(user["variables"][1]["value"], r"%USERPROFILE%\bin;C:\Tools");

    let system = read_json(&target.join(SYSTEM_JSON));
    assert_eq!(
        system["key"],
        r"HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment"
    );
    assert_eq!(system["variables"].as_array().unwrap().len(), 2);

    assert_eq!(
        std::fs::read(target.join(USER_REG)).unwrap(),
        reg_file_content(r"HKEY_CURRENT_USER\Environment")
    );
    let out = target.join(USER_REG).to_string_lossy().into_owned();
    assert_eq!(
        runner.calls(),
        [(
            REG_EXE.to_owned(),
            ["export", r"HKCU\Environment", &out, "/y"]
                .map(str::to_owned)
                .to_vec()
        )]
    );
    // P1: nothing written outside the target folder.
    let mut expected: Vec<_> = [USER_JSON, USER_REG, SYSTEM_JSON]
        .iter()
        .map(|f| PathBuf::from("system").join(ID).join(f))
        .collect();
    expected.sort();
    assert_eq!(files_under(&dir.path().join("backup")), expected);
}

#[tokio::test]
async fn unreadable_system_key_and_missing_user_key_are_warnings() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    let mut values = MemRegistry::new();
    add_values(&mut values, RegHive::Hkcu, USER_KEY, user_values());
    add_values(&mut values, RegHive::Hklm, SYSTEM_KEY, system_values());
    values.add_denied_key(RegHive::Hklm, SYSTEM_KEY);
    // `reg export` sees no user key: the `.reg` file is skipped with a warning.
    let (exporter, runner) = exporter(reg_runner(), &[], values);

    let result = run_export(
        &exporter,
        &env,
        &dir.path().join("out"),
        &CancellationToken::new(),
    )
    .await
    .unwrap();

    let names: Vec<_> = result.files.iter().map(|f| f.rel_path.clone()).collect();
    assert_eq!(names, [PathBuf::from(USER_JSON)]);
    assert!(runner.calls().is_empty());
    assert_eq!(result.warnings.len(), 2, "{:?}", result.warnings);
    assert_eq!(result.warnings[0].message_key, ISSUE_KEY_MISSING);
    assert_eq!(result.warnings[0].message_args["key"], r"HKCU\Environment");
    let read = &result.warnings[1];
    assert_eq!(read.message_key, ISSUE_READ_FAILED);
    assert_eq!(
        read.message_args["key"],
        r"HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment"
    );
    assert!(
        read.message_args["error"].contains("access denied"),
        "{read:?}"
    );
}

#[tokio::test]
async fn nothing_exported_is_the_first_error() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    let (exporter, _) = exporter(reg_runner(), &[], MemRegistry::new());
    let result = run_export(
        &exporter,
        &env,
        &dir.path().join("out"),
        &CancellationToken::new(),
    )
    .await;
    assert!(
        matches!(result, Err(ExportError::NotAvailable)),
        "{result:?}"
    );
    assert!(!dir.path().join("out").exists());
}

#[tokio::test]
async fn cancellation_stops_the_export() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    let (exporter, runner) = full_machine();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let result = run_export(&exporter, &env, &dir.path().join("out"), &cancel).await;
    assert!(matches!(result, Err(ExportError::Cancelled)), "{result:?}");
    assert!(runner.calls().is_empty());
    assert!(!dir.path().join("out").exists());
}

/// SPEC-06 §6: the real `env-vars` export contains `Path` (read only, into `target/tmp`).
#[cfg(windows)]
#[tokio::test]
async fn real_env_vars_export_contains_path() {
    let env = Environment::detect().unwrap();
    let dir = temp_dir();
    let target = dir.path().join(ID);
    let result = run_export(
        &EnvVarsExporter::new(),
        &env,
        &target,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let names: Vec<_> = result.files.iter().map(|f| f.rel_path.clone()).collect();
    assert_eq!(
        names,
        [USER_JSON, USER_REG, SYSTEM_JSON].map(PathBuf::from),
        "{result:?}"
    );

    let has_path = |json: &serde_json::Value| {
        json["variables"].as_array().unwrap().iter().any(|v| {
            v["name"]
                .as_str()
                .is_some_and(|n| n.eq_ignore_ascii_case("Path"))
        })
    };
    // The system `Path` always exists; a user `Path` is usual but not guaranteed.
    assert!(has_path(&read_json(&target.join(SYSTEM_JSON))));
    let user = read_json(&target.join(USER_JSON));
    assert_eq!(user["key"], r"HKCU\Environment");
    println!("user Path present: {}", has_path(&user));
    let reg = std::fs::read(target.join(USER_REG)).unwrap();
    assert_eq!(&reg[..2], [0xFF, 0xFE]);
}
