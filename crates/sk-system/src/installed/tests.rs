use std::path::{Path, PathBuf};

use serde_json::Value;
use sk_core::env::{Environment, InstalledProgram, ProgramSource};
use sk_core::model::RegHive;
use sk_core::registry::{MemRegistry, SystemRegistry};

use super::{enrich_with, installed_programs, is_system_package, location, MSIX_PACKAGES};

/// Registry dump of SPEC-06 §6 «фикстура JSON-дампа ключей».
const FIXTURE: &str = include_str!("../../../../fixtures/samples/system/uninstall-keys.json");

/// Loads the JSON dump into a `MemRegistry`: strings are `REG_SZ`, numbers `REG_DWORD`,
/// `"denied": true` keys cannot be read.
fn fixture_registry() -> MemRegistry {
    let dump: Value = serde_json::from_str(FIXTURE).expect("fixture is valid JSON");
    let mut registry = MemRegistry::new();
    for entry in dump["keys"].as_array().expect("keys array") {
        let hive = match entry["hive"].as_str().expect("hive") {
            "hklm" => RegHive::Hklm,
            "hkcu" => RegHive::Hkcu,
            other => panic!("unknown hive {other}"),
        };
        let key = entry["key"].as_str().expect("key");
        if entry["denied"].as_bool() == Some(true) {
            registry.add_denied_key(hive, key);
            continue;
        }
        registry.add_key(hive, key);
        let values = entry["values"].as_object().expect("values object");
        for (name, value) in values {
            match value {
                Value::String(s) => registry.set_string(hive, key, name, s),
                Value::Number(n) => {
                    let dword = u32::try_from(n.as_u64().expect("u64")).expect("u32");
                    registry.set_dword(hive, key, name, dword)
                }
                other => panic!("unsupported value {other}"),
            };
        }
    }
    registry
}

fn summary(programs: &[InstalledProgram]) -> Vec<(&str, Option<&str>, ProgramSource)> {
    programs
        .iter()
        .map(|p| (p.name.as_str(), p.publisher.as_deref(), p.source))
        .collect()
}

fn find<'a>(programs: &'a [InstalledProgram], name: &str) -> &'a InstalledProgram {
    programs
        .iter()
        .find(|p| p.name == name)
        .unwrap_or_else(|| panic!("{name} not found"))
}

#[test]
fn fixture_is_filtered_deduplicated_and_sorted() {
    let programs = installed_programs(&fixture_registry());
    assert_eq!(
        summary(&programs),
        vec![
            (
                "7-Zip 24.08 (x64)",
                Some("Igor Pavlov"),
                ProgramSource::Hklm
            ),
            ("Discord", Some("Discord Inc."), ProgramSource::Hkcu),
            (
                "Git",
                Some("The Git Development Community"),
                ProgramSource::Hklm
            ),
            ("Microsoft.UI.Xaml.2.8", None, ProgramSource::Msix),
            ("Notepad++ (32-bit x86)", None, ProgramSource::Hkcu),
            (
                "Notepad++ (32-bit x86)",
                Some("Notepad++ Team"),
                ProgramSource::Hklm32
            ),
            ("SpotifyAB.SpotifyMusic", None, ProgramSource::Msix),
            ("Steam", Some("Valve Corporation"), ProgramSource::Hklm32),
            ("VLC media player", Some("VideoLAN"), ProgramSource::Hklm),
        ]
    );
}

#[test]
fn uninstall_entry_fields_are_read() {
    let programs = installed_programs(&fixture_registry());
    assert_eq!(
        find(&programs, "7-Zip 24.08 (x64)"),
        &InstalledProgram {
            name: "7-Zip 24.08 (x64)".into(),
            publisher: Some("Igor Pavlov".into()),
            version: Some("24.08".into()),
            install_location: Some(PathBuf::from(r"C:\Program Files\7-Zip\")),
            install_date: Some("20240812".into()),
            estimated_size_kb: Some(5911),
            source: ProgramSource::Hklm,
            uninstall_key: "7-Zip".into(),
        }
    );
    assert_eq!(
        find(&programs, "Discord").estimated_size_kb,
        Some(103_424),
        "EstimatedSize is a DWORD in KiB"
    );
}

#[test]
fn values_are_trimmed_and_blank_ones_are_none() {
    let programs = installed_programs(&fixture_registry());
    let vlc = find(&programs, "VLC media player");
    assert_eq!(vlc.install_location, None, "blank InstallLocation");
    assert_eq!(vlc.install_date, None);
    assert_eq!(vlc.estimated_size_kb, None);
    assert_eq!(
        find(&programs, "Git").install_location,
        Some(PathBuf::from(r"C:\Program Files\Git\")),
        "quotes around InstallLocation are removed"
    );
}

#[test]
fn msix_package_name_version_and_location() {
    let programs = installed_programs(&fixture_registry());
    assert_eq!(
        find(&programs, "SpotifyAB.SpotifyMusic"),
        &InstalledProgram {
            name: "SpotifyAB.SpotifyMusic".into(),
            publisher: None,
            version: Some("1.303.264.0".into()),
            install_location: Some(PathBuf::from(
                r"C:\Program Files\WindowsApps\SpotifyAB.SpotifyMusic_1.303.264.0_x64__zpdnekdrzrea0"
            )),
            install_date: None,
            estimated_size_kb: None,
            source: ProgramSource::Msix,
            uninstall_key: "SpotifyAB.SpotifyMusic_1.303.264.0_x64__zpdnekdrzrea0".into(),
        }
    );
    let xaml = find(&programs, "Microsoft.UI.Xaml.2.8");
    assert_eq!(
        xaml.uninstall_key, "Microsoft.UI.Xaml.2.8_8.2501.31001.0_x64__8wekyb3d8bbwe",
        "x64 and x86 packages are one program; the first key wins"
    );
}

#[test]
fn first_source_wins_on_duplicates() {
    let mut registry = MemRegistry::new();
    let hklm = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\App";
    let hkcu = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\App";
    registry
        .set_string(RegHive::Hkcu, hkcu, "DisplayName", "app")
        .set_string(RegHive::Hkcu, hkcu, "DisplayVersion", "2.0")
        .set_string(RegHive::Hklm, hklm, "DisplayName", "App")
        .set_string(RegHive::Hklm, hklm, "DisplayVersion", "1.0")
        .set_string(
            RegHive::Hkcu,
            &format!(r"{MSIX_PACKAGES}\APP_3.0.0.0_x64__abc"),
            "PackageRootFolder",
            r"C:\x",
        );
    let programs = installed_programs(&registry);
    assert_eq!(programs.len(), 1);
    assert_eq!(programs[0].source, ProgramSource::Hklm);
    assert_eq!(programs[0].version.as_deref(), Some("1.0"));
}

#[test]
fn enrich_replaces_previous_programs_and_is_repeatable() {
    let registry = fixture_registry();
    let mut env = Environment::fake(Path::new("root"));
    env.installed_programs.push(InstalledProgram {
        name: "Stale".into(),
        publisher: None,
        version: None,
        install_location: None,
        install_date: None,
        estimated_size_kb: None,
        source: ProgramSource::Hkcu,
        uninstall_key: "Stale".into(),
    });
    enrich_with(&mut env, &registry);
    let first = env.installed_programs.clone();
    assert!(first.iter().all(|p| p.name != "Stale"));
    assert_eq!(first.len(), 9);
    enrich_with(&mut env, &registry);
    assert_eq!(env.installed_programs, first);
}

#[test]
fn empty_registry_gives_no_programs() {
    assert!(installed_programs(&MemRegistry::new()).is_empty());
}

#[cfg(not(windows))]
#[test]
fn system_registry_outside_windows_gives_no_programs() {
    let mut env = Environment::fake(Path::new("root"));
    super::enrich(&mut env);
    assert!(env.installed_programs.is_empty());
    assert!(installed_programs(&SystemRegistry).is_empty());
}

#[test]
fn system_packages_are_recognised() {
    for name in [
        "Microsoft.Windows.Photos",
        "microsoft.windows.ShellExperienceHost",
        "Microsoft.NET.Native.Runtime.2.2",
        "Microsoft.NET.CoreRuntime.1.1",
        "Microsoft.VCLibs.140.00",
        "Microsoft.VCLibs.140.00.UWPDesktop",
    ] {
        assert!(is_system_package(name), "{name}");
    }
    for name in [
        "Microsoft.WindowsCalculator",
        "Microsoft.WindowsTerminal",
        "SpotifyAB.SpotifyMusic",
        "Microsoft.NETFoo",
    ] {
        assert!(!is_system_package(name), "{name}");
    }
}

#[test]
fn location_strips_quotes_and_blanks() {
    assert_eq!(location(r#""C:\A B""#), Some(PathBuf::from(r"C:\A B")));
    assert_eq!(location(r"C:\A"), Some(PathBuf::from(r"C:\A")));
    assert_eq!(location(r#""""#), None);
    assert_eq!(location(" "), None);
}

/// Manual check against «Apps & features» (SPEC-06 T-06-03): prints what `enrich`
/// finds on this machine. Run with `cargo test -p sk-system -- --ignored --nocapture
/// installed_programs_of_this_machine`.
#[cfg(windows)]
#[test]
#[ignore = "manual: depends on the programs installed on this machine"]
fn installed_programs_of_this_machine() {
    let programs = installed_programs(&SystemRegistry);
    for p in &programs {
        println!(
            "{:?}\t{}\t{}\t{}",
            p.source,
            p.name,
            p.publisher.as_deref().unwrap_or("-"),
            p.version.as_deref().unwrap_or("-")
        );
    }
    println!("total: {}", programs.len());
    assert!(!programs.is_empty());
}
