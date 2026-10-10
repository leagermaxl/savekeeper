use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use sk_core::model::Sensitivity;
use sk_core::registry::MemRegistry;
use sk_core::CancellationToken;

use super::*;
use crate::cmd::{CmdRunner, FakeCmdRunner};
use crate::exporters::test_util::{
    assert_system_settings_plan, files_under, reg_file_content, reg_runner, run_export, temp_dir,
};
use crate::reg_export::REG_EXE;

/// A registry with `HKCU\...\Fonts` holding `values` font registrations (no key for 0).
fn registry(values: usize) -> MemRegistry {
    let mut registry = MemRegistry::new();
    if values > 0 {
        registry.add_key(RegHive::Hkcu, FONTS_KEY);
    }
    for i in 0..values {
        let name = format!("Font {i} (TrueType)");
        let file = format!(r"C:\Users\user\AppData\Local\Microsoft\Windows\Fonts\font{i}.ttf");
        registry.set_string(RegHive::Hkcu, FONTS_KEY, &name, &file);
    }
    registry
}

/// The exporter whose `reg export` and `detect` see `registry`.
fn exporter(runner: FakeCmdRunner, registry: MemRegistry) -> (FontsExporter, Arc<FakeCmdRunner>) {
    let registry: Arc<dyn RegistryReader> = Arc::new(registry);
    let runner = Arc::new(runner);
    let reg = RegExport::new(
        Arc::clone(&runner) as Arc<dyn CmdRunner>,
        Arc::clone(&registry),
    );
    (FontsExporter::with(reg, registry), runner)
}

/// Writes the files `names` (content: the name) into `dir`.
fn write_files(dir: &Path, names: &[&str]) {
    std::fs::create_dir_all(dir).unwrap();
    for name in names {
        std::fs::write(dir.join(name), name.as_bytes()).unwrap();
    }
}

fn user_dir(root: &Path) -> PathBuf {
    root.join("Users")
        .join("user")
        .join("AppData")
        .join("Local")
        .join("Microsoft")
        .join("Windows")
        .join("Fonts")
}

fn system_dir(root: &Path) -> PathBuf {
    root.join("Windows").join("Fonts")
}

#[test]
fn plan_is_system_settings_without_sensitivity() {
    assert_system_settings_plan(&FontsExporter::new(), TITLE, Sensitivity::None);
}

#[cfg(not(windows))]
#[test]
fn detect_off_windows_is_not_windows() {
    let env = Environment::fake(Path::new("/fake"));
    assert_eq!(FontsExporter::new().detect(&env), unavailable(NOT_WINDOWS));
}

#[test]
fn glob_patterns() {
    assert!(glob_match("arial.ttf", "arial.ttf"));
    assert!(!glob_match("arial.ttf", "arialbd.ttf"));
    assert!(glob_match("calibri*.ttf", "calibri.ttf"));
    assert!(glob_match("calibri*.ttf", "calibriz.ttf"));
    assert!(!glob_match("calibri*.ttf", "calibri.otf"));
    assert!(glob_match("8514*.fon", "8514oem.fon"));
    assert!(glob_match("*", ""));
    assert!(glob_match("a*b*c", "axxbyyc"));
    assert!(glob_match("a*b*c", "abcbc"));
    assert!(!glob_match("a*b*c", "axxbyy"));
    assert!(glob_match("шрифт*.ttf", "шрифт-1.ttf"));
}

#[test]
fn standard_list_is_lowercase_font_names() {
    let patterns: Vec<_> = standard_patterns().collect();
    assert!(patterns.len() > 100, "{}", patterns.len());
    for pattern in patterns {
        assert_eq!(pattern, pattern.to_lowercase(), "{pattern}");
        assert!(is_font(Path::new(pattern)), "{pattern}");
    }
}

#[test]
fn windows_fonts_are_standard_others_are_not() {
    for name in [
        "arial.ttf",
        "SegoeUI.ttf",
        "seguiemj.ttf",
        "consolaz.ttf",
        "Sitka.ttc",
        "msyh.ttc",
        "vga866.fon",
        "8514oem.fon",
        "CascadiaMono.ttf",
        "times.ttf",
    ] {
        assert!(is_standard(name), "{name}");
    }
    for name in [
        "unispace bd.ttf",
        "Roboto-Regular.ttf",
        "GARA.TTF",
        "JetBrainsMono-Bold.ttf",
        "terminus.fon",
    ] {
        assert!(!is_standard(name), "{name}");
    }
    let names = ["b.ttf", "arial.ttf", "A.otf"].map(OsStr::new);
    assert_eq!(nonstandard(names), ["A.otf", "b.ttf"]);
}

#[test]
fn nothing_installed_is_unavailable() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    let (exporter, _) = exporter(reg_runner(), registry(0));
    assert_eq!(exporter.probe(&env), unavailable(NONE));
    // Files that are not fonts do not count.
    write_files(&user_dir(dir.path()), &["desktop.ini", "notes.txt"]);
    assert_eq!(exporter.probe(&env), unavailable(NONE));
}

#[test]
fn user_font_files_are_available_with_their_size() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    write_files(&user_dir(dir.path()), &["a.ttf", "B.OTF", "desktop.ini"]);
    let (exporter, _) = exporter(reg_runner(), registry(0));
    assert_eq!(
        exporter.probe(&env),
        Availability::Available {
            estimated_bytes: Some(("a.ttf".len() + "B.OTF".len()) as u64),
            details: BTreeMap::from([
                ("user_fonts".to_owned(), "2".to_owned()),
                ("registered_fonts".to_owned(), "0".to_owned()),
            ]),
        }
    );
}

#[test]
fn registrations_alone_are_available() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    let (exporter, _) = exporter(reg_runner(), registry(3));
    let Availability::Available { details, .. } = exporter.probe(&env) else {
        panic!("not available");
    };
    assert_eq!(details["user_fonts"], "0");
    assert_eq!(details["registered_fonts"], "3");
}

#[tokio::test]
async fn run_copies_user_fonts_exports_registry_and_lists_nonstandard_fonts() {
    let dir = temp_dir();
    let machine = dir.path().join("machine");
    let env = Environment::fake(&machine);
    write_files(&user_dir(&machine), &["Fira.otf", "a.ttf", "desktop.ini"]);
    write_files(
        &system_dir(&machine),
        &[
            "arial.ttf",
            "vgaoem.fon",
            "unispace bd.ttf",
            "Gara.ttf",
            "desktop.ini",
        ],
    );
    let target = dir.path().join("backup").join("system").join(ID);
    let (exporter, runner) = exporter(reg_runner(), registry(2));

    let result = run_export(&exporter, &env, &target, &CancellationToken::new())
        .await
        .unwrap();

    let names: Vec<_> = result.files.iter().map(|f| f.rel_path.clone()).collect();
    assert_eq!(
        names,
        [
            Path::new(USER_DIR).join("Fira.otf"),
            Path::new(USER_DIR).join("a.ttf"),
            PathBuf::from(REG_FILE),
            PathBuf::from(NONSTANDARD_FILE),
        ]
    );
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
    assert_eq!(
        std::fs::read(target.join(USER_DIR).join("a.ttf")).unwrap(),
        b"a.ttf"
    );
    assert_eq!(
        std::fs::read_to_string(target.join(NONSTANDARD_FILE)).unwrap(),
        "Gara.ttf\r\nunispace bd.ttf\r\n"
    );
    assert_eq!(
        std::fs::read(target.join(REG_FILE)).unwrap(),
        reg_file_content(r"HKEY_CURRENT_USER\Software\Microsoft\Windows NT\CurrentVersion\Fonts")
    );
    let out = target.join(REG_FILE).to_string_lossy().into_owned();
    let branch = format!(r"HKCU\{FONTS_KEY}");
    assert_eq!(
        runner.calls(),
        [(
            REG_EXE.to_owned(),
            ["export", branch.as_str(), &out, "/y"]
                .map(str::to_owned)
                .to_vec()
        )]
    );
    // P1: sources unchanged, nothing written outside the target folder.
    assert_eq!(
        std::fs::read(user_dir(&machine).join("Fira.otf")).unwrap(),
        b"Fira.otf"
    );
    let mut expected: Vec<_> = names
        .iter()
        .map(|f| PathBuf::from("system").join(ID).join(f))
        .collect();
    expected.sort();
    assert_eq!(files_under(&dir.path().join("backup")), expected);
}

#[tokio::test]
async fn missing_registry_key_and_standard_system_fonts_give_only_user_fonts() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    write_files(&user_dir(dir.path()), &["a.ttf"]);
    write_files(&system_dir(dir.path()), &["arial.ttf", "segoeui.ttf"]);
    let (exporter, runner) = exporter(reg_runner(), registry(0));

    let result = run_export(
        &exporter,
        &env,
        &dir.path().join("out"),
        &CancellationToken::new(),
    )
    .await
    .unwrap();

    let names: Vec<_> = result.files.iter().map(|f| f.rel_path.clone()).collect();
    assert_eq!(names, [Path::new(USER_DIR).join("a.ttf")]);
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert!(runner.calls().is_empty());
}

#[tokio::test]
async fn unreadable_system_folder_and_failed_reg_export_are_warnings() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    write_files(&user_dir(dir.path()), &["a.ttf"]);
    // No `{WINDIR}\Fonts`: the folder cannot be listed.
    let runner = FakeCmdRunner::new().output(REG_EXE, 1, "ERROR: access denied");
    let (exporter, _) = exporter(runner, registry(1));

    let result = run_export(
        &exporter,
        &env,
        &dir.path().join("out"),
        &CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(result.files.len(), 1);
    let keys: Vec<_> = result
        .warnings
        .iter()
        .map(|w| w.message_key.as_str())
        .collect();
    assert_eq!(keys, [ISSUE_EXPORT_FAILED, ISSUE_FOLDER_READ_FAILED]);
    assert_eq!(
        result.warnings[1].message_args["path"],
        system_dir(dir.path()).to_string_lossy()
    );
}

#[tokio::test]
async fn nothing_to_export_is_not_available() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    write_files(&system_dir(dir.path()), &["arial.ttf"]);
    let (exporter, _) = exporter(reg_runner(), registry(0));
    let target = dir.path().join("out");
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
    write_files(&user_dir(dir.path()), &["a.ttf"]);
    let (exporter, runner) = exporter(reg_runner(), registry(1));
    let cancel = CancellationToken::new();
    cancel.cancel();
    let target = dir.path().join("out");
    let result = run_export(&exporter, &env, &target, &cancel).await;
    assert!(matches!(result, Err(ExportError::Cancelled)), "{result:?}");
    assert!(runner.calls().is_empty());
    assert!(!target.exists());
}

/// `detect` on this machine: fast and without errors (no files are written).
#[cfg(windows)]
#[test]
fn real_fonts_detect() {
    let env = Environment::detect().unwrap();
    let started = std::time::Instant::now();
    let avail = FontsExporter::new().detect(&env);
    println!("fonts: {avail:?} in {:?}", started.elapsed());
}

/// The real export copies the user's fonts (may be large): manual run only, into
/// `target/tmp`.
#[cfg(windows)]
#[tokio::test]
#[ignore = "copies the fonts of this machine's user"]
async fn real_fonts_export() {
    let env = Environment::detect().unwrap();
    let dir = temp_dir();
    let result = run_export(
        &FontsExporter::new(),
        &env,
        &dir.path().join(ID),
        &CancellationToken::new(),
    )
    .await;
    println!("{result:#?}");
}
