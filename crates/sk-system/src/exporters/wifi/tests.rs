use std::path::{Path, PathBuf};
use std::sync::Arc;

use sk_core::config::SystemConfig;
use sk_core::env::{Environment, KnownFolder};
use sk_core::model::{AppKind, Category, EvidenceSource, FindingId, Sensitivity, Target};
use sk_core::CancellationToken;

use super::*;
use crate::cmd::{CmdOutput, FakeCmdRunner};
use crate::exporters::test_util::{files_under, temp_dir};

mod run;

const ENCRYPTED: &str = "01000000D08C9DDF0115D1118C7A00C04FC297EB";

/// A profile as `netsh wlan export profile` writes it, with the key element `key`.
fn profile_xml(ssid: &str, protected: bool, key: &str) -> String {
    format!(
        r#"<?xml version="1.0"?>
<WLANProfile xmlns="http://www.microsoft.com/networking/WLAN/profile/v1">
	<name>{ssid}</name>
	<MSM>
		<security>
			<authEncryption>
				<authentication>WPA2PSK</authentication>
				<encryption>AES</encryption>
			</authEncryption>
			<sharedKey>
				<keyType>passPhrase</keyType>
				<protected>{protected}</protected>
				<keyMaterial>{key}</keyMaterial>
			</sharedKey>
		</security>
	</MSM>
</WLANProfile>
"#
    )
}

/// [`profile_xml`] without the `<sharedKey>` element and its line.
fn profile_without_key(ssid: &str) -> String {
    format!(
        r#"<?xml version="1.0"?>
<WLANProfile xmlns="http://www.microsoft.com/networking/WLAN/profile/v1">
	<name>{ssid}</name>
	<MSM>
		<security>
			<authEncryption>
				<authentication>WPA2PSK</authentication>
				<encryption>AES</encryption>
			</authEncryption>
		</security>
	</MSM>
</WLANProfile>
"#
    )
}

fn fake_exporter(runner: FakeCmdRunner) -> (WifiExporter, Arc<FakeCmdRunner>) {
    let runner = Arc::new(runner);
    let exporter = WifiExporter::with_runner(Arc::clone(&runner) as Arc<dyn CmdRunner>);
    (exporter, runner)
}

/// The folder of the `folder=` argument of `netsh wlan export profile`.
fn export_folder(cmd: &Cmd) -> PathBuf {
    cmd.get_args()
        .iter()
        .find_map(|a| a.to_str()?.strip_prefix("folder="))
        .map(PathBuf::from)
        .unwrap()
}

/// A fake `netsh wlan export profile` writing two profiles like the real one: keys in
/// clear text with `key=clear` (the second one stays protected), encrypted without.
fn export_runner(code: i32) -> FakeCmdRunner {
    FakeCmdRunner::new().on(NETSH_EXE, move |cmd| {
        let folder = export_folder(cmd);
        let clear = cmd.get_args().iter().any(|a| a == "key=clear");
        let home = if clear {
            profile_xml("HomeNet", false, "secret-password")
        } else {
            profile_xml("HomeNet", true, ENCRYPTED)
        };
        std::fs::write(folder.join("Wi-Fi-HomeNet.xml"), home).unwrap();
        std::fs::write(
            folder.join("Wi-Fi-Office.xml"),
            profile_xml("Office", true, ENCRYPTED),
        )
        .unwrap();
        Ok(CmdOutput {
            code,
            ..CmdOutput::default()
        })
    })
}

async fn run_with(
    exporter: &WifiExporter,
    env: &Environment,
    target_dir: &Path,
    params: serde_json::Value,
    cancel: &CancellationToken,
) -> Result<ExportResult, ExportError> {
    let (events, _rx) = tokio::sync::mpsc::unbounded_channel();
    let ctx = ExportContext {
        env,
        target_dir,
        cancel,
        events: &events,
        elevated: false,
        config: &SystemConfig::default(),
    };
    exporter.run(&ctx, &params).await
}

/// [`run_with`] without keys and without cancellation.
async fn run_default(
    exporter: &WifiExporter,
    env: &Environment,
    target_dir: &Path,
) -> Result<ExportResult, ExportError> {
    run_with(
        exporter,
        env,
        target_dir,
        default_params(),
        &CancellationToken::new(),
    )
    .await
}

fn profiles_dir(env: &Environment) -> PathBuf {
    env.known_folder(KnownFolder::ProgramData)
        .unwrap()
        .join(r"Microsoft\Wlansvc\Profiles\Interfaces")
}

fn read(path: &Path) -> String {
    String::from_utf8(std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn only_boolean_true_includes_keys() {
    assert!(include_keys(&serde_json::json!({ "include_keys": true })));
    for params in [
        serde_json::json!({}),
        serde_json::json!({ "include_keys": false }),
        serde_json::json!({ "include_keys": "true" }),
        serde_json::json!({ "include_keys": 1 }),
        serde_json::Value::Null,
    ] {
        assert!(!include_keys(&params), "{params}");
    }
    assert_eq!(
        default_params(),
        serde_json::json!({ "include_keys": false })
    );
}

#[test]
fn keys_make_the_finding_highly_sensitive() {
    let with_keys = serde_json::json!({ "include_keys": true });
    assert_eq!(sensitivity(&with_keys), Sensitivity::High);
    assert_eq!(sensitivity(&default_params()), Sensitivity::None);
}

#[test]
fn plan_is_system_settings_finding_without_keys() {
    let env = Environment::fake(Path::new("/fake"));
    let (exporter, _) = fake_exporter(FakeCmdRunner::new());
    let avail = Availability::Available {
        estimated_bytes: Some(2048),
        details: BTreeMap::from([("profiles".to_owned(), "2".to_owned())]),
    };
    let finding = exporter.plan(&env, &avail).unwrap();
    let target = Target::SystemExport {
        exporter_id: ID.to_owned(),
        params: serde_json::json!({ "include_keys": false }),
    };
    assert_eq!(finding.id, FindingId::for_target(&target));
    assert_eq!(finding.target, target);
    assert_eq!(finding.category, Category::SystemSettings);
    assert_eq!(finding.sensitivity, Sensitivity::None);
    assert_eq!(finding.title, TITLE);
    assert!(!finding.requires_elevation);
    assert_eq!(finding.app.as_ref().map(|a| a.kind), Some(AppKind::System));
    assert_eq!(
        finding.evidence[0].source,
        EvidenceSource::System {
            exporter_id: ID.to_owned()
        }
    );
    assert_eq!(finding.evidence[0].message_args["profiles"], "2");

    let elevated = Availability::NeedsElevation {
        estimated_bytes: None,
    };
    assert!(exporter.plan(&env, &elevated).unwrap().requires_elevation);
    assert!(exporter.plan(&env, &unavailable(NO_PROFILES)).is_none());
}

#[test]
fn probe_counts_saved_profiles_without_netsh() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    let interfaces = profiles_dir(&env);
    let first = interfaces.join("{0A1B2C3D-0000-0000-0000-000000000001}");
    let second = interfaces.join("{0A1B2C3D-0000-0000-0000-000000000002}");
    std::fs::create_dir_all(&first).unwrap();
    std::fs::create_dir_all(&second).unwrap();
    std::fs::write(first.join("{AAAA}.xml"), b"<WLANProfile/>").unwrap();
    std::fs::write(first.join("{BBBB}.XML"), b"<WLANProfile/>").unwrap();
    std::fs::write(first.join("notes.txt"), b"").unwrap();
    std::fs::write(second.join("{CCCC}.xml"), b"<WLANProfile/>").unwrap();
    std::fs::write(interfaces.join("stray.xml"), b"").unwrap();
    let (exporter, runner) = fake_exporter(FakeCmdRunner::new().output(NETSH_EXE, 1, ""));

    assert_eq!(
        exporter.probe(&env),
        Availability::Available {
            estimated_bytes: Some(3 * ESTIMATE_BYTES_PER_PROFILE),
            details: BTreeMap::from([("profiles".to_owned(), "3".to_owned())]),
        }
    );
    assert!(runner.calls().is_empty(), "nothing is started");
}

/// The interface folder exists but cannot be read: `Interfaces` is a file, so
/// `read_dir` fails with an error other than `NotFound` on every OS.
fn unreadable_profiles_dir(env: &Environment) {
    let interfaces = profiles_dir(env);
    std::fs::create_dir_all(interfaces.parent().unwrap()).unwrap();
    std::fs::write(&interfaces, b"").unwrap();
}

fn show_interfaces_call() -> (String, Vec<String>) {
    (
        NETSH_EXE.to_owned(),
        ["wlan", "show", "interfaces"].map(str::to_owned).to_vec(),
    )
}

#[test]
fn probe_without_profiles_is_unavailable_without_netsh() {
    // A PC on a cable: `netsh wlan show interfaces` exits with 0 while WlanSvc runs, which
    // is not a sign of an adapter (SPEC-06 §5).
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    let (exporter, runner) = fake_exporter(FakeCmdRunner::new().output(NETSH_EXE, 0, "..."));
    // The folder is missing.
    assert_eq!(exporter.probe(&env), unavailable(NO_PROFILES));
    // Interface folders without profiles (adapter removed, profiles deleted).
    std::fs::create_dir_all(profiles_dir(&env).join("{EMPTY}")).unwrap();
    std::fs::write(profiles_dir(&env).join("stray.xml"), b"").unwrap();
    assert_eq!(exporter.probe(&env), unavailable(NO_PROFILES));
    // ProgramData is unknown.
    let mut env = env;
    env.known_folders.remove(&KnownFolder::ProgramData);
    assert_eq!(exporter.probe(&env), unavailable(NO_PROFILES));
    assert!(runner.calls().is_empty(), "{:?}", runner.calls());
}

#[test]
fn probe_asks_the_wlan_service_only_when_the_folder_cannot_be_read() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    unreadable_profiles_dir(&env);

    let (exporter, runner) = fake_exporter(FakeCmdRunner::new().output(NETSH_EXE, 0, "..."));
    assert_eq!(
        exporter.probe(&env),
        Availability::Available {
            estimated_bytes: None,
            details: BTreeMap::new(),
        }
    );
    let netsh = windir(&env).unwrap().join(r"System32\netsh.exe");
    assert_eq!(netsh_exe(&env), Some(netsh));
    assert_eq!(runner.calls(), [show_interfaces_call()]);

    // The service is not running: netsh exits with 1.
    let (exporter, runner) = fake_exporter(FakeCmdRunner::new().output(NETSH_EXE, 1, ""));
    assert_eq!(exporter.probe(&env), unavailable(NO_PROFILES));
    assert_eq!(runner.calls(), [show_interfaces_call()]);
    // netsh cannot be started.
    let (exporter, _) = fake_exporter(FakeCmdRunner::new());
    assert_eq!(exporter.probe(&env), unavailable(NO_PROFILES));
    // The Windows folder is unknown: nothing to start.
    let mut env = env;
    env.known_folders.remove(&KnownFolder::WinDir);
    let (exporter, runner) = fake_exporter(FakeCmdRunner::new().output(NETSH_EXE, 0, ""));
    assert_eq!(exporter.probe(&env), unavailable(NO_PROFILES));
    assert!(runner.calls().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn probe_works_inside_a_runtime() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    unreadable_profiles_dir(&env);
    let (exporter, runner) = fake_exporter(FakeCmdRunner::new().output(NETSH_EXE, 0, ""));
    assert!(matches!(
        exporter.probe(&env),
        Availability::Available { .. }
    ));
    assert_eq!(runner.calls(), [show_interfaces_call()]);
}

#[cfg(not(windows))]
#[test]
fn detect_off_windows_is_not_windows() {
    let (exporter, runner) = fake_exporter(FakeCmdRunner::new().output(NETSH_EXE, 0, ""));
    let env = Environment::fake(Path::new("/fake"));
    assert_eq!(exporter.detect(&env), unavailable(NOT_WINDOWS));
    assert!(runner.calls().is_empty());
}

#[test]
fn strip_keys_goes_through_all_files_and_returns_the_first_error() {
    let dir = temp_dir();
    std::fs::write(
        dir.path().join("Wi-Fi-B.xml"),
        profile_xml("B", true, ENCRYPTED),
    )
    .unwrap();
    let names = ["Wi-Fi-A.xml", "Wi-Fi-B.xml", "Wi-Fi-C.xml"].map(OsString::from);

    let err = strip_keys(dir.path(), &names).unwrap_err();

    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    assert_eq!(
        read(&dir.path().join("Wi-Fi-B.xml")),
        profile_without_key("B")
    );
}
