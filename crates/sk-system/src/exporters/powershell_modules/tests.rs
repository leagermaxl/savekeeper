use std::path::{Path, PathBuf};

use sk_core::config::SystemConfig;
use sk_core::model::{AppKind, EvidenceSource, Target};
use sk_core::CancellationToken;

use super::manifest::{decode, module_version};
use super::*;
use crate::exporters::test_util::{files_under, run_export, temp_dir};

fn documents(root: &Path) -> PathBuf {
    root.join("Users").join("user").join("Documents")
}

fn modules_dir(root: &Path, edition: &str) -> PathBuf {
    documents(root).join(edition).join("Modules")
}

/// Writes `files` (relative path, content) under `dir`.
fn write_files(dir: &Path, files: &[(&str, &str)]) {
    for (rel, content) in files {
        let path = rel.split('/').fold(dir.to_path_buf(), |p, s| p.join(s));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
}

fn manifest(version: &str) -> String {
    format!("@{{\r\n    RootModule = 'M.psm1'\r\n    ModuleVersion = '{version}'\r\n}}\r\n")
}

/// A machine with a versioned module (two versions), an unversioned one and a Windows
/// PowerShell module with a big file.
fn machine(root: &Path) {
    let core = modules_dir(root, "PowerShell");
    write_files(
        &core,
        &[
            ("Pester/5.5.0/Pester.psd1", &manifest("5.5.0")),
            ("Pester/5.5.0/Pester.psm1", "function Invoke-Pester {}"),
            ("Pester/10.0.1/Pester.psd1", &manifest("10.0.1")),
            ("Pester/notes/readme.txt", "notes"),
            ("my-tools/my-tools.psd1", "@{ ModuleVersion = \"0.3\" }"),
            ("my-tools/my-tools.psm1", "function Get-X {}"),
            ("loose.txt", "not a module"),
        ],
    );
    let big = "x".repeat(1000);
    write_files(
        &modules_dir(root, "WindowsPowerShell"),
        &[("Big/Big.psm1", big.as_str())],
    );
}

fn read_json(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn plan_is_dev_environment_with_copy_files() {
    let env = Environment::fake(Path::new("/fake"));
    let exporter = PowershellModulesExporter::new();
    let avail = Availability::Available {
        estimated_bytes: None,
        details: BTreeMap::from([("modules".to_owned(), "2".to_owned())]),
    };
    let finding = exporter.plan(&env, &avail).unwrap();
    assert_eq!(
        finding.target,
        Target::SystemExport {
            exporter_id: ID.to_owned(),
            params: json!({ "copy_files": true }),
        }
    );
    assert_eq!(finding.category, Category::DevEnvironment);
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
    assert_eq!(finding.evidence[0].message_args["modules"], "2");
    let elevated = Availability::NeedsElevation {
        estimated_bytes: None,
    };
    assert!(exporter.plan(&env, &elevated).unwrap().requires_elevation);
    assert!(exporter.plan(&env, &unavailable(NONE)).is_none());
}

#[cfg(not(windows))]
#[test]
fn detect_off_windows_is_not_windows() {
    let env = Environment::fake(Path::new("/fake"));
    assert_eq!(
        PowershellModulesExporter::new().detect(&env),
        unavailable(NOT_WINDOWS)
    );
}

#[test]
fn copy_files_parameter() {
    assert!(copy_files(&default_params()));
    assert!(copy_files(&json!({})));
    assert!(copy_files(&json!({ "copy_files": "no" })));
    assert!(!copy_files(&json!({ "copy_files": false })));
}

#[test]
fn module_version_of_manifests() {
    assert_eq!(module_version(&manifest("1.2.3")), Some("1.2.3".to_owned()));
    assert_eq!(
        module_version("@{ moduleversion=\"2.0\"; Author = 'x' }"),
        Some("2.0".to_owned())
    );
    assert_eq!(
        module_version("@{\n# ModuleVersion = '9.9'\nModuleVersion = 3.1 # note\n}"),
        Some("3.1".to_owned())
    );
    assert_eq!(
        module_version("<#\nModuleVersion = '0.0'\n#>\n@{\n  ModuleVersion = '4.0.0'\n}"),
        Some("4.0.0".to_owned())
    );
    assert_eq!(
        module_version("<# c #> ModuleVersion = '5.0'"),
        Some("5.0".to_owned())
    );
    assert_eq!(module_version("@{ ModuleVersionX = '1.0' }"), None);
    assert_eq!(module_version("@{ ModuleVersion = '' }"), None);
    assert_eq!(module_version("@{ ModuleVersion = 'unclosed }"), None);
    assert_eq!(module_version("Модуль\n@{ RootModule = 'x' }"), None);
}

#[test]
fn manifests_are_decoded_by_bom() {
    let text = "ModuleVersion = '1.0' # Ж";
    let mut le = vec![0xFF, 0xFE];
    le.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    let mut be = vec![0xFE, 0xFF];
    be.extend(text.encode_utf16().flat_map(u16::to_be_bytes));
    let mut utf8 = b"\xEF\xBB\xBF".to_vec();
    utf8.extend(text.as_bytes());
    for bytes in [le, be, utf8, text.as_bytes().to_vec()] {
        assert_eq!(decode(&bytes), text);
    }
}

#[test]
fn versions_of_versioned_and_plain_modules() {
    let dir = temp_dir();
    machine(dir.path());
    let core = modules_dir(dir.path(), "PowerShell");
    // Version folders sorted numerically; a folder without a manifest keeps its name.
    write_files(&core, &[("Pester/2.0/readme.txt", "")]);
    assert_eq!(
        versions(&core.join("Pester"), "Pester"),
        [
            ModuleVersion {
                version: Some("2.0".to_owned()),
                path: "Pester/2.0".to_owned()
            },
            ModuleVersion {
                version: Some("5.5.0".to_owned()),
                path: "Pester/5.5.0".to_owned()
            },
            ModuleVersion {
                version: Some("10.0.1".to_owned()),
                path: "Pester/10.0.1".to_owned()
            },
        ]
    );
    assert_eq!(
        versions(&core.join("my-tools"), "my-tools"),
        [ModuleVersion {
            version: Some("0.3".to_owned()),
            path: "my-tools".to_owned()
        }]
    );
    let big = modules_dir(dir.path(), "WindowsPowerShell").join("Big");
    assert_eq!(
        versions(&big, "Big"),
        [ModuleVersion {
            version: None,
            path: "Big".to_owned()
        }]
    );
}

#[test]
fn probe_counts_module_folders() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    assert_eq!(PowershellModulesExporter::probe(&env), unavailable(NONE));
    // Loose files are not modules.
    write_files(&modules_dir(dir.path(), "PowerShell"), &[("loose.txt", "")]);
    assert_eq!(PowershellModulesExporter::probe(&env), unavailable(NONE));
    machine(dir.path());
    assert_eq!(
        PowershellModulesExporter::probe(&env),
        Availability::Available {
            estimated_bytes: None,
            details: BTreeMap::from([("modules".to_owned(), "3".to_owned())]),
        }
    );
}

#[tokio::test]
async fn run_lists_and_copies_modules_into_target_dir_only() {
    let dir = temp_dir();
    let root = dir.path().join("machine");
    machine(&root);
    let env = Environment::fake(&root);
    let target = dir.path().join("backup").join("system").join(ID);

    let result = run_export(
        &PowershellModulesExporter::new(),
        &env,
        &target,
        &CancellationToken::new(),
    )
    .await
    .unwrap();

    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_eq!(
        result.restore_hint,
        RestoreHint::Manual {
            key: RESTORE_KEY.to_owned()
        }
    );
    assert_eq!(result.files[0].rel_path, PathBuf::from(MODULES_JSON));
    for file in &result.files {
        let bytes = std::fs::read(target.join(&file.rel_path)).unwrap();
        assert_eq!(file.bytes, bytes.len() as u64);
        assert_eq!(file.blake3, blake3::hash(&bytes).to_hex().to_string());
    }
    let json = read_json(&target.join(MODULES_JSON));
    assert_eq!(
        json,
        json!({ "modules": [
            { "name": "my-tools", "version": "0.3", "edition": "PowerShell", "path": "my-tools", "copied": true },
            { "name": "Pester", "version": "5.5.0", "edition": "PowerShell", "path": "Pester/5.5.0", "copied": true },
            { "name": "Pester", "version": "10.0.1", "edition": "PowerShell", "path": "Pester/10.0.1", "copied": true },
            { "name": "Big", "version": null, "edition": "WindowsPowerShell", "path": "Big", "copied": true },
        ]})
    );
    let copied = |edition: &str, rel: &str| {
        let path = rel
            .split('/')
            .fold(target.join(MODULES_DIR).join(edition), |p, s| p.join(s));
        std::fs::read_to_string(path).unwrap()
    };
    assert_eq!(copied("PowerShell", "Pester/notes/readme.txt"), "notes");
    assert_eq!(
        copied("PowerShell", "Pester/5.5.0/Pester.psd1"),
        manifest("5.5.0")
    );
    assert_eq!(copied("WindowsPowerShell", "Big/Big.psm1").len(), 1000);
    // P1: sources unchanged, every written file is in the target folder and reported.
    let core = modules_dir(&root, "PowerShell");
    assert_eq!(
        std::fs::read_to_string(core.join("my-tools").join("my-tools.psm1")).unwrap(),
        "function Get-X {}"
    );
    let mut expected: Vec<_> = result
        .files
        .iter()
        .map(|f| PathBuf::from("system").join(ID).join(&f.rel_path))
        .collect();
    expected.sort();
    assert_eq!(files_under(&dir.path().join("backup")), expected);
    assert_eq!(result.files.len(), 1 + 7);
}

#[tokio::test]
async fn without_copy_files_only_the_list_is_written() {
    let dir = temp_dir();
    machine(dir.path());
    let env = Environment::fake(dir.path());
    let target = dir.path().join("out");
    let (events, _rx) = tokio::sync::mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let ctx = ExportContext {
        env: &env,
        target_dir: &target,
        cancel: &cancel,
        events: &events,
        elevated: false,
        config: &SystemConfig::default(),
    };
    let result = PowershellModulesExporter::new()
        .run(&ctx, &json!({ "copy_files": false }))
        .await
        .unwrap();
    let names: Vec<_> = result.files.iter().map(|f| f.rel_path.clone()).collect();
    assert_eq!(names, [PathBuf::from(MODULES_JSON)]);
    let json = read_json(&target.join(MODULES_JSON));
    assert!(json["modules"]
        .as_array()
        .unwrap()
        .iter()
        .all(|m| m["copied"] == false));
    assert_eq!(files_under(&target), [PathBuf::from(MODULES_JSON)]);
}

#[test]
fn copy_limit_keeps_the_smallest_modules() {
    let module = |edition, name: &str, bytes| Module {
        edition,
        name: name.to_owned(),
        dir: PathBuf::new(),
        versions: Vec::new(),
        files: vec![ModuleFile {
            rel_path: PathBuf::from("f"),
            bytes,
        }],
        copied: false,
    };
    let mut modules = vec![
        module("PowerShell", "Az", 900),
        module("PowerShell", "small", 100),
        module("WindowsPowerShell", "mid", 400),
        module("PowerShell", "mid", 400),
    ];
    let mut failures = Partial::default();
    select_for_copy(&mut modules, 1000, &mut failures);
    let copied: Vec<_> = modules.iter().map(|m| m.copied).collect();
    assert_eq!(copied, [false, true, true, true]);
    let result = failures.finish(Vec::new(), RestoreHint::None);
    assert!(
        matches!(result, Err(ExportError::NotAvailable)),
        "{result:?}"
    );

    let mut failures = Partial::default();
    select_for_copy(&mut modules, 450, &mut failures);
    let copied: Vec<_> = modules.iter().map(|m| m.copied).collect();
    assert_eq!(copied, [false, true, false, false]);
}

#[tokio::test]
async fn module_over_the_limit_is_listed_with_a_warning() {
    let dir = temp_dir();
    let root = dir.path();
    let core = modules_dir(root, "PowerShell");
    write_files(&core, &[("small/small.psm1", "x")]);
    write_files(&core, &[("Huge/Huge.dll", "0123456789")]);
    let env = Environment::fake(root);
    let target = root.join("out");

    let exporter = PowershellModulesExporter::with_copy_limit(5);
    let result = run_export(&exporter, &env, &target, &CancellationToken::new())
        .await
        .unwrap();

    let names: Vec<_> = result.files.iter().map(|f| f.rel_path.clone()).collect();
    assert_eq!(
        names,
        [
            PathBuf::from(MODULES_JSON),
            Path::new(MODULES_DIR)
                .join("PowerShell")
                .join("small")
                .join("small.psm1"),
        ]
    );
    assert_eq!(result.warnings.len(), 1, "{:?}", result.warnings);
    let warning = &result.warnings[0];
    assert_eq!(warning.message_key, ISSUE_NOT_COPIED);
    assert_eq!(warning.message_args["module"], "Huge");
    assert_eq!(warning.message_args["edition"], "PowerShell");
    assert_eq!(warning.message_args["bytes"], "10");
    let json = read_json(&target.join(MODULES_JSON));
    assert_eq!(json["modules"][0]["name"], "Huge");
    assert_eq!(json["modules"][0]["copied"], false);
    assert_eq!(json["modules"][1]["copied"], true);
}

#[tokio::test]
async fn no_modules_is_not_available_and_cancellation_stops() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    let target = dir.path().join("out");
    let result = run_export(
        &PowershellModulesExporter::new(),
        &env,
        &target,
        &CancellationToken::new(),
    )
    .await;
    assert!(
        matches!(result, Err(ExportError::NotAvailable)),
        "{result:?}"
    );

    machine(dir.path());
    let cancel = CancellationToken::new();
    cancel.cancel();
    let result = run_export(&PowershellModulesExporter::new(), &env, &target, &cancel).await;
    assert!(matches!(result, Err(ExportError::Cancelled)), "{result:?}");
    assert!(!target.exists());
}

/// `detect` on this machine (no files are written).
#[cfg(windows)]
#[test]
fn real_powershell_modules_detect() {
    let env = Environment::detect().unwrap();
    let started = std::time::Instant::now();
    let avail = PowershellModulesExporter::new().detect(&env);
    println!("powershell-modules: {avail:?} in {:?}", started.elapsed());
}

/// The real export may copy up to 200 MB: manual run only, into `target/tmp`.
#[cfg(windows)]
#[tokio::test]
#[ignore = "copies this machine's PowerShell modules (up to 200 MB)"]
async fn real_powershell_modules_export() {
    let env = Environment::detect().unwrap();
    let dir = temp_dir();
    let result = run_export(
        &PowershellModulesExporter::new(),
        &env,
        &dir.path().join(ID),
        &CancellationToken::new(),
    )
    .await;
    match result {
        Ok(result) => println!(
            "{} files, warnings: {:#?}",
            result.files.len(),
            result.warnings
        ),
        Err(err) => println!("{err}"),
    }
}
