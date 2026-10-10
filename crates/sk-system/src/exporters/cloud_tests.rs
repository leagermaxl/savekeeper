//! Cloud-only files in the copying exporters `fonts`, `hosts` and `powershell-modules`
//! (SPEC-06 T-06-17, SPEC-10 FR-10-15): such a file is never opened, it is skipped with
//! the warning `issue.system.cloud_only` {path}.
//!
//! The cloud state is faked by [`FakeSourceFs`], which also counts opens; on Windows the
//! real check is tested with the OFFLINE attribute set on files the test creates in its
//! own temporary folder.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sk_core::env::Environment;
use sk_core::registry::{MemRegistry, RegistryReader};
use sk_core::CancellationToken;

use super::fonts::{self, FontsExporter};
use super::hosts::{self, HostsExporter};
use super::partial::{Partial, ISSUE_CLOUD_ONLY, ISSUE_FILE_COPY_FAILED};
use super::powershell_modules::{self, PowershellModulesExporter};
use super::test_util::{files_under, reg_runner, run_export, temp_dir, FakeSourceFs};
use crate::cmd::CmdRunner;
use crate::error::ExportError;
use crate::exporter::{Availability, ExportResult, RestoreHint, SystemExporter};
use crate::reg_export::RegExport;

/// Writes `files` (relative path with `/`, content) under `dir`; returns their paths.
fn write_files(dir: &Path, files: &[(&str, &str)]) -> Vec<PathBuf> {
    files
        .iter()
        .map(|(rel, content)| {
            let path = rel.split('/').fold(dir.to_path_buf(), |p, s| p.join(s));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, content).unwrap();
            path
        })
        .collect()
}

/// `{LOCALAPPDATA}\Microsoft\Windows\Fonts` of `Environment::fake(root)`.
fn user_font_dir(root: &Path) -> PathBuf {
    let local = ["Users", "user", "AppData", "Local", "Microsoft", "Windows"];
    local
        .iter()
        .fold(root.to_path_buf(), |p, s| p.join(s))
        .join("Fonts")
}

/// A machine under `root` with an empty `{WINDIR}\Fonts`, so that only user fonts count.
fn font_machine(root: &Path) -> Environment {
    std::fs::create_dir_all(root.join("Windows").join("Fonts")).unwrap();
    Environment::fake(root)
}

/// `{WINDIR}\System32\drivers\etc` of `Environment::fake(root)`.
fn hosts_dir(root: &Path) -> PathBuf {
    root.join("Windows")
        .join("System32")
        .join("drivers")
        .join("etc")
}

/// `{DOCUMENTS}\PowerShell\Modules` of `Environment::fake(root)`.
fn modules_dir(root: &Path) -> PathBuf {
    root.join("Users")
        .join("user")
        .join("Documents")
        .join("PowerShell")
        .join("Modules")
}

/// The `fonts` exporter without HKCU font registrations, reading through `source`.
fn fonts_exporter(source: &Arc<FakeSourceFs>) -> FontsExporter {
    let registry: Arc<dyn RegistryReader> = Arc::new(MemRegistry::new());
    let reg = RegExport::new(
        Arc::new(reg_runner()) as Arc<dyn CmdRunner>,
        Arc::clone(&registry),
    );
    FontsExporter::with(reg, registry).with_source(source.shared())
}

/// The `path` arguments of the cloud-only warnings of `result`.
fn cloud_only_paths(result: &ExportResult) -> Vec<String> {
    result
        .warnings
        .iter()
        .filter(|w| w.message_key == ISSUE_CLOUD_ONLY)
        .map(|w| {
            assert_eq!(w.message_args.len(), 1, "{w:?}");
            w.message_args["path"].clone()
        })
        .collect()
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[test]
fn cloud_only_is_warned_once_per_path_and_is_not_an_error() {
    let mut partial = Partial::default();
    partial.cloud_only(Path::new("a.ttf"));
    partial.cloud_only(Path::new("a.ttf"));
    partial.cloud_only(Path::new("b.ttf"));
    // Only cloud-only skips: a result without files that says why.
    let result = partial.finish(Vec::new(), RestoreHint::None).unwrap();
    assert!(result.files.is_empty());
    assert_eq!(cloud_only_paths(&result), ["a.ttf", "b.ttf"]);

    // A failed part still decides the error when nothing was written.
    let mut partial = Partial::default();
    partial.cloud_only(Path::new("a.ttf"));
    let err = std::io::Error::other("denied");
    partial
        .add_path(ISSUE_FILE_COPY_FAILED, Path::new("c.ttf"), err.into())
        .unwrap();
    let result = partial.finish(Vec::new(), RestoreHint::None);
    assert!(matches!(result, Err(ExportError::Io(_))), "{result:?}");

    // Nothing at all is still `NotAvailable`.
    let result = Partial::default().finish(Vec::new(), RestoreHint::None);
    assert!(
        matches!(result, Err(ExportError::NotAvailable)),
        "{result:?}"
    );
}

#[tokio::test]
async fn fonts_skip_cloud_only_fonts_without_opening_them() {
    let dir = temp_dir();
    let machine = dir.path().join("machine");
    let env = font_machine(&machine);
    let paths = write_files(
        &user_font_dir(&machine),
        &[("a.ttf", "local font"), ("b.otf", "cloud font")],
    );
    let source = FakeSourceFs::new([paths[1].clone()]);
    let exporter = fonts_exporter(&source);

    // `detect` lists the folder only and opens nothing.
    assert!(matches!(
        exporter.probe(&env),
        Availability::Available { .. }
    ));
    assert!(source.opened().is_empty());

    let target = dir.path().join("backup");
    let result = run_export(&exporter, &env, &target, &CancellationToken::new())
        .await
        .unwrap();
    let names: Vec<_> = result.files.iter().map(|f| f.rel_path.clone()).collect();
    assert_eq!(names, [Path::new(fonts::USER_DIR).join("a.ttf")]);
    assert_eq!(cloud_only_paths(&result), [text(&paths[1])]);
    assert_eq!(result.warnings.len(), 1);
    assert_eq!(source.opens_of(&paths[0]), 1);
    assert_eq!(source.opens_of(&paths[1]), 0);
    assert_eq!(files_under(&target), names);

    // Only cloud-only fonts: a result without files, with the warnings.
    let source = FakeSourceFs::new(paths.clone());
    let exporter = fonts_exporter(&source);
    let target = dir.path().join("backup-2");
    let result = run_export(&exporter, &env, &target, &CancellationToken::new())
        .await
        .unwrap();
    assert!(result.files.is_empty());
    assert_eq!(
        cloud_only_paths(&result),
        [text(&paths[0]), text(&paths[1])]
    );
    assert!(source.opened().is_empty());
    assert!(!target.exists() || files_under(&target).is_empty());
}

#[tokio::test]
async fn hosts_in_the_cloud_is_not_opened() {
    let dir = temp_dir();
    let machine = dir.path().join("machine");
    let env = Environment::fake(&machine);
    let hosts = write_files(
        &hosts_dir(&machine),
        &[("hosts", "127.0.0.1 dev.local\r\n")],
    )
    .remove(0);

    // A local file is read by `detect` (once) and copied by `run`.
    let local = FakeSourceFs::new([]);
    let exporter = HostsExporter::with_source(local.shared());
    assert!(matches!(
        exporter.probe(&env),
        Availability::Available {
            estimated_bytes: Some(_),
            ..
        }
    ));
    assert_eq!(local.opens_of(&hosts), 1);

    let cloud = FakeSourceFs::new([hosts.clone()]);
    let exporter = HostsExporter::with_source(cloud.shared());
    // Whether it has entries is unknown: available without an estimate.
    assert_eq!(
        exporter.probe(&env),
        Availability::Available {
            estimated_bytes: None,
            details: Default::default(),
        }
    );
    let target = dir.path().join("backup");
    let result = run_export(&exporter, &env, &target, &CancellationToken::new())
        .await
        .unwrap();
    assert!(result.files.is_empty());
    assert_eq!(cloud_only_paths(&result), [text(&hosts)]);
    assert_eq!(result.warnings.len(), 1);
    assert!(cloud.opened().is_empty());
    assert!(!target.join(hosts::FILE).exists());
}

/// A versioned module with a cloud-only manifest and data file, and a plain module with
/// a cloud-only manifest; returns the cloud-only paths.
fn cloud_modules(root: &Path) -> Vec<PathBuf> {
    let paths = write_files(
        &modules_dir(root),
        &[
            ("Pester/1.2/Pester.psd1", "@{ ModuleVersion = '1.2.3' }"),
            ("Pester/1.2/Pester.psm1", "function Invoke-Pester {}"),
            ("Pester/1.2/data.bin", "cloud data"),
            ("tools/tools.psd1", "@{ ModuleVersion = '0.3' }"),
            ("tools/tools.psm1", "function Get-X {}"),
        ],
    );
    vec![paths[0].clone(), paths[2].clone(), paths[3].clone()]
}

#[tokio::test]
async fn powershell_modules_are_copied_without_cloud_only_files() {
    let dir = temp_dir();
    let machine = dir.path().join("machine");
    let env = Environment::fake(&machine);
    let cloud = cloud_modules(&machine);
    let source = FakeSourceFs::new(cloud.clone());
    let exporter = PowershellModulesExporter::new().with_source(source.shared());
    let target = dir.path().join("backup");

    let result = run_export(&exporter, &env, &target, &CancellationToken::new())
        .await
        .unwrap();

    let dest = Path::new(powershell_modules::MODULES_DIR).join("PowerShell");
    let names: Vec<_> = result.files.iter().map(|f| f.rel_path.clone()).collect();
    assert_eq!(
        names,
        [
            PathBuf::from(powershell_modules::MODULES_JSON),
            dest.join("Pester").join("1.2").join("Pester.psm1"),
            dest.join("tools").join("tools.psm1"),
        ]
    );
    // Each cloud-only file is warned once, even the manifest read for the version and
    // listed for the copy; none of them is opened.
    let mut warned = cloud_only_paths(&result);
    warned.sort();
    let mut expected: Vec<_> = cloud.iter().map(|p| text(p)).collect();
    expected.sort();
    assert_eq!(warned, expected);
    assert_eq!(result.warnings.len(), cloud.len());
    for path in &cloud {
        assert_eq!(source.opens_of(path), 0, "{}", path.display());
    }

    // The versions of cloud-only manifests: the version folder, else unknown; the
    // modules count as copied.
    let json: serde_json::Value = serde_json::from_slice(
        &std::fs::read(target.join(powershell_modules::MODULES_JSON)).unwrap(),
    )
    .unwrap();
    let modules = json["modules"].as_array().unwrap();
    assert_eq!(modules.len(), 2);
    assert_eq!(modules[0]["name"], "Pester");
    assert_eq!(modules[0]["version"], "1.2");
    assert_eq!(modules[0]["copied"], true);
    assert_eq!(modules[1]["name"], "tools");
    assert_eq!(modules[1]["version"], serde_json::Value::Null);
    assert_eq!(modules[1]["copied"], true);
    let mut written = files_under(&target);
    written.sort();
    let mut names = names;
    names.sort();
    assert_eq!(written, names);
}

/// The real check (`sk_core::fs::cloud_state`) with the OFFLINE attribute, set by
/// `attrib +O` on files this test created in its own temporary folder.
#[cfg(windows)]
mod win {
    use std::os::windows::fs::MetadataExt;
    use std::process::Command;

    use super::*;

    const FILE_ATTRIBUTE_OFFLINE: u32 = 0x1000;

    /// Marks the test file `path` as offline (cloud-only).
    fn set_offline(path: &Path) {
        let status = Command::new("attrib").arg("+O").arg(path).status().unwrap();
        assert!(status.success(), "attrib +O {}", path.display());
        assert!(is_offline(path), "{}", path.display());
    }

    fn is_offline(path: &Path) -> bool {
        let attrs = std::fs::symlink_metadata(path).unwrap().file_attributes();
        attrs & FILE_ATTRIBUTE_OFFLINE != 0
    }

    #[tokio::test]
    async fn offline_files_are_not_opened_by_any_copying_exporter() {
        let dir = temp_dir();
        let machine = dir.path().join("machine");
        let env = font_machine(&machine);
        let fonts = write_files(
            &user_font_dir(&machine),
            &[("a.ttf", "local font"), ("b.otf", "cloud font")],
        );
        let hosts = write_files(&hosts_dir(&machine), &[("hosts", "10.0.0.1 nas\r\n")]);
        let modules = cloud_modules(&machine);
        let offline: Vec<PathBuf> = [&fonts[1], &hosts[0]]
            .into_iter()
            .chain(&modules)
            .cloned()
            .collect();
        for path in &offline {
            set_offline(path);
        }
        // The fake marks nothing itself: the real attribute check decides.
        let source = FakeSourceFs::new([]);
        let cancel = CancellationToken::new();

        let target = dir.path().join("fonts");
        let result = run_export(&fonts_exporter(&source), &env, &target, &cancel)
            .await
            .unwrap();
        assert_eq!(result.files.len(), 1);
        assert_eq!(cloud_only_paths(&result), [text(&fonts[1])]);

        let exporter = HostsExporter::with_source(source.shared());
        assert!(matches!(
            exporter.detect(&env),
            Availability::Available {
                estimated_bytes: None,
                ..
            }
        ));
        let target = dir.path().join("hosts");
        let result = run_export(&exporter, &env, &target, &cancel).await.unwrap();
        assert!(result.files.is_empty());
        assert_eq!(cloud_only_paths(&result), [text(&hosts[0])]);

        let exporter = PowershellModulesExporter::new().with_source(source.shared());
        let target = dir.path().join("modules");
        let result = run_export(&exporter, &env, &target, &cancel).await.unwrap();
        assert_eq!(result.files.len(), 3);
        assert_eq!(cloud_only_paths(&result).len(), modules.len());

        // Not opened, still offline and unchanged; the local files were copied.
        for path in &offline {
            assert_eq!(source.opens_of(path), 0, "{}", path.display());
            assert!(is_offline(path), "{}", path.display());
        }
        assert_eq!(source.opens_of(&fonts[0]), 1);
        assert_eq!(std::fs::read(&fonts[1]).unwrap(), b"cloud font");
    }
}
