use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::json;
use sk_core::env::{DriveInfo, ProgramSource};
use sk_core::model::Sensitivity;
use sk_core::CancellationToken;

use super::hints::{self, wildcard_match, LicenseKind};
use super::render::{escape, interpolate};
use super::*;
use crate::exporters::disks_info::{self, BitLocker};
use crate::exporters::test_util::{assert_system_settings_plan, files_under, run_export, temp_dir};
use crate::exporters::windows_info::{self, Activation as WmiActivation};

fn program(name: &str, publisher: Option<&str>) -> InstalledProgram {
    InstalledProgram {
        name: name.to_owned(),
        publisher: publisher.map(str::to_owned),
        version: None,
        install_location: None,
        install_date: None,
        estimated_size_kb: None,
        source: ProgramSource::Hklm,
        uninstall_key: name.to_owned(),
    }
}

/// Programs of a test machine: licensed ones, a free edition, an authenticator.
fn programs() -> Vec<InstalledProgram> {
    vec![
        program("Adobe Photoshop 2024", Some("Adobe Inc.")),
        program("Adobe Acrobat Reader DC", Some("Adobe Inc.")),
        program("PyCharm 2024.2.1", Some("JetBrains s.r.o.")),
        program(
            "IntelliJ IDEA Community Edition 2024.2",
            Some("JetBrains s.r.o."),
        ),
        program("WinRAR 7.01 (64-bit)", Some("win.rar GmbH")),
        program("Twilio Authy Desktop", Some("Twilio")),
        program("7-Zip 24.08 (x64)", Some("Igor Pavlov")),
        program("Ableton Live 12 Suite", Some("Ableton")),
        program("Microsoft 365 Apps for business - en-us", Some("Microsoft")),
        program("MATLAB R2024a", Some("MathWorks")),
    ]
}

fn drive(env: &Environment, letter: char, kind: &str) -> DriveInfo {
    let mut drive = env.drives[0].clone();
    drive.letter = letter;
    drive.kind = serde_json::from_value(json!(kind)).unwrap();
    drive
}

/// A backup `system` folder with the files of `windows-info`, `disks-info`, `drivers`
/// and `winget`, written by the real renderers of those exporters.
fn write_inputs(system: &Path, env: &Environment) {
    let windows = windows_info::render(
        env,
        Some("Professional"),
        &WmiActivation {
            license_status: Some(1),
            channel: Some("OEM:DM".to_owned()),
        },
        Some(true),
    );
    let dir = system.join(windows_info::ID);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(windows_info::FILE), windows).unwrap();

    let drives = [
        drive(env, 'C', "fixed"),
        drive(env, 'D', "fixed"),
        drive(env, 'E', "removable"),
        drive(env, 'Z', "network"),
    ];
    let statuses = BTreeMap::from([('C', BitLocker::On), ('D', BitLocker::Off)]);
    let dir = system.join(disks_info::ID);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(disks_info::FILE),
        disks_info::render(&drives, &statuses),
    )
    .unwrap();

    let dir = system.join(drivers::ID);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("oem1.inf"), b"[Version]").unwrap();
    let dir = system.join(winget::ID);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(winget::EXPORT_FILE), b"{}").unwrap();
}

/// Facts with every item of the checklist.
fn all_facts(lang: Lang) -> Facts {
    let env = Environment::fake(Path::new("/fake"));
    let mut facts = collect_facts(
        &env,
        Path::new("/no/such/backup/system/checklist"),
        &json!({}),
    );
    facts.lang = lang;
    facts.machine_name = "HOME_PC".to_owned();
    facts.credentials = true;
    facts.browsers = true;
    facts.authenticators = vec!["Twilio Authy Desktop".to_owned()];
    facts.bitlocker = BitLockerFacts::Checked {
        encrypted: vec!['C'],
        unknown: vec!['D', 'E'],
    };
    facts.licenses = hints::match_programs(&hints::builtin(), &programs());
    facts.windows = Some(WindowsFacts {
        activation: Activation::Licensed,
        oem_key_in_firmware: true,
    });
    facts.unpushed_repos = 2;
    facts.cloud_only_files = 15;
    facts
}

#[test]
fn plan_is_system_settings_without_sensitivity() {
    assert_system_settings_plan(&ChecklistExporter, TITLE, Sensitivity::None);
}

#[test]
fn detect_is_always_available_on_windows() {
    let env = Environment::fake(Path::new("/fake"));
    let avail = ChecklistExporter.detect(&env);
    if cfg!(windows) {
        assert!(matches!(avail, Availability::Available { .. }), "{avail:?}");
    } else {
        assert_eq!(avail, unavailable(NOT_WINDOWS));
    }
}

#[test]
fn builtin_hints_parse_and_are_consistent() {
    let list = hints::builtin();
    assert!(list.len() >= 30, "only {} programs", list.len());
    let mut names = std::collections::BTreeSet::new();
    for hint in &list {
        assert!(
            names.insert(hint.name.to_lowercase()),
            "duplicate {}",
            hint.name
        );
        assert!(
            !hint.names.is_empty() || !hint.publisher.is_empty(),
            "{} matches nothing",
            hint.name
        );
        let patterns = hint
            .names
            .iter()
            .chain(&hint.publisher)
            .chain(&hint.exclude);
        for pattern in patterns {
            assert!(
                !pattern.trim().is_empty() && pattern != "*",
                "{}",
                hint.name
            );
        }
        assert_eq!(
            hint.kind == LicenseKind::LicenseFile,
            hint.file.is_some(),
            "{}: `file` goes with `license_file` only",
            hint.name
        );
    }
}

#[test]
fn unknown_fields_and_kinds_are_rejected() {
    assert!(hints::parse("programs:\n  - name: A\n    match: [A]\n    kind: crack\n").is_err());
    assert!(hints::parse("programs:\n  - name: A\n    key: X\n    kind: key\n").is_err());
    assert!(hints::parse("programs:\n  - name: A\n    match: [A]\n    kind: key\n").is_ok());
}

#[test]
fn wildcard_patterns() {
    assert!(wildcard_match("Adobe Photoshop*", "adobe photoshop 2024"));
    assert!(wildcard_match(
        "*Community*",
        "IntelliJ IDEA Community Edition"
    ));
    assert!(wildcard_match("ESET *", "ESET Security"));
    assert!(!wildcard_match("ESET *", "ESETSecurity"));
    assert!(wildcard_match("a*b*c", "aXXbYYc"));
    assert!(!wildcard_match("a*b*c", "aXXbYY"));
    assert!(wildcard_match(
        "1С:Предприятие*",
        "1С:ПРЕДПРИЯТИЕ 8 (x86-64)"
    ));
    assert!(!wildcard_match("WinRAR*", "My WinRAR"));
    assert!(wildcard_match("**", ""));
}

#[test]
fn programs_match_hints_by_name_publisher_and_exclusions() {
    let found = hints::match_programs(&hints::builtin(), &programs());
    let by_name: BTreeMap<&str, &[String]> = found
        .iter()
        .map(|m| (m.hint.name.as_str(), m.programs.as_slice()))
        .collect();
    assert_eq!(by_name["Adobe Creative Cloud"], ["Adobe Photoshop 2024"]);
    // Acrobat Reader is free; IntelliJ IDEA Community is excluded.
    assert!(!by_name.contains_key("Adobe Acrobat"));
    assert_eq!(by_name["JetBrains IDEs"], ["PyCharm 2024.2.1"]);
    assert_eq!(by_name["WinRAR"], ["WinRAR 7.01 (64-bit)"]);
    assert_eq!(by_name["Microsoft 365"].len(), 1);
    assert!(by_name.contains_key("Ableton Live") && by_name.contains_key("MATLAB"));
    assert_eq!(found.len(), 6, "{:?}", by_name.keys());
    // Sorted by entry name.
    let names: Vec<_> = found.iter().map(|m| m.hint.name.to_lowercase()).collect();
    assert!(names.windows(2).all(|w| w[0] <= w[1]), "{names:?}");
}

/// The free Reader of the unified 64-bit installer is "Adobe Acrobat (64-bit)", like a
/// paid Acrobat: only names of paid editions match.
#[test]
fn only_paid_acrobat_editions_match() {
    let list = hints::builtin();
    let acrobat = list.iter().find(|h| h.name == "Adobe Acrobat").unwrap();
    for name in [
        "Adobe Acrobat (64-bit)",
        "Adobe Acrobat Reader DC",
        "Adobe Acrobat Reader DC - Russian",
        "Adobe Acrobat Reader (64-bit)",
    ] {
        assert!(!acrobat.matches(&program(name, Some("Adobe"))), "{name}");
    }
    for name in [
        "Adobe Acrobat DC",
        "Adobe Acrobat Pro DC",
        "Adobe Acrobat Standard 2020",
        "Adobe Acrobat 2017",
    ] {
        assert!(acrobat.matches(&program(name, Some("Adobe"))), "{name}");
    }
}

#[test]
fn interpolation_is_one_pass_and_keeps_unknown_args() {
    assert_eq!(
        interpolate("a {{x}} b {{ y }}", &[("x", "1"), ("y", "2")]),
        "a 1 b 2"
    );
    assert_eq!(
        interpolate("{{x}}", &[("x", "{{y}}"), ("y", "no")]),
        "{{y}}"
    );
    assert_eq!(
        interpolate("{{missing}} {{x", &[("x", "1")]),
        "{{missing}} {{x"
    );
}

#[test]
fn markdown_characters_are_escaped() {
    assert_eq!(
        escape("A*B_[c]`d` <e> #1 |"),
        r"A\*B\_\[c\]\`d\` \<e\> \#1 \|"
    );
    assert_eq!(escape(r"C:\Tools"), r"C:\\Tools");
}

#[test]
fn checklist_en_with_all_facts() {
    insta::assert_snapshot!(
        "checklist_en_all_facts",
        render::render(&all_facts(Lang::En))
    );
}

#[test]
fn checklist_ru_with_all_facts() {
    insta::assert_snapshot!(
        "checklist_ru_all_facts",
        render::render(&all_facts(Lang::Ru))
    );
}

/// Every text is found and every argument is filled, in both languages.
#[test]
fn every_text_is_resolved() {
    for lang in [Lang::En, Lang::Ru] {
        let mut facts = all_facts(lang);
        for (kind, file) in [
            (LicenseKind::SignOut, None),
            (LicenseKind::Account, None),
            (LicenseKind::Key, None),
            (LicenseKind::Deactivate, None),
            (LicenseKind::LicenseFile, Some("x.key".to_owned())),
        ] {
            let mut extra = facts.licenses[0].clone();
            extra.hint.kind = kind;
            extra.hint.file = file;
            facts.licenses.push(extra);
        }
        let mut texts = render::render(&facts);
        for activation in [
            Activation::Volume,
            Activation::NotActivated,
            Activation::Unknown,
        ] {
            facts.windows = Some(WindowsFacts {
                activation,
                oem_key_in_firmware: false,
            });
            texts.push_str(&render::render(&facts));
        }
        facts.bitlocker = BitLockerFacts::NotChecked;
        facts.drivers_exported = false;
        facts.winget_exported = false;
        texts.push_str(&render::render(&facts));
        assert!(!texts.contains("{{"), "{texts}");
        assert!(!texts.contains("system.checklist"), "{texts}");
    }
}

#[test]
fn facts_come_from_params_programs_and_sibling_files() {
    let dir = temp_dir();
    let system = dir.path().join("system");
    let mut env = Environment::fake(dir.path());
    env.installed_programs = programs();
    write_inputs(&system, &env);
    let params = json!({
        "credentials": true,
        "browsers": true,
        "unpushed_repos": 3,
        "cloud_only_files": 7,
    });
    let facts = collect_facts(&env, &system.join(ID), &params);
    assert_eq!(facts.lang, Lang::En);
    assert!(facts.credentials && facts.browsers);
    assert_eq!((facts.unpushed_repos, facts.cloud_only_files), (3, 7));
    assert_eq!(facts.authenticators, ["Twilio Authy Desktop"]);
    // C: on, D: off, E: unknown (removable), Z: network with an unknown status: skipped.
    assert_eq!(
        facts.bitlocker,
        BitLockerFacts::Checked {
            encrypted: vec!['C'],
            unknown: vec!['E'],
        }
    );
    assert_eq!(
        facts.windows,
        Some(WindowsFacts {
            activation: Activation::Licensed,
            oem_key_in_firmware: true,
        })
    );
    assert!(facts.drivers_exported && facts.winget_exported);
    assert_eq!(facts.licenses.len(), 6);
}

#[test]
fn missing_inputs_and_params_are_not_checked() {
    let dir = temp_dir();
    let mut env = Environment::fake(dir.path());
    env.os.ui_language = "ru-RU".to_owned();
    let params = json!({ "credentials": "yes", "unpushed_repos": -1 });
    let facts = collect_facts(&env, &dir.path().join("system").join(ID), &params);
    assert_eq!(facts.lang, Lang::Ru);
    assert!(!facts.credentials && !facts.browsers);
    assert_eq!((facts.unpushed_repos, facts.cloud_only_files), (0, 0));
    assert_eq!(facts.bitlocker, BitLockerFacts::NotChecked);
    assert_eq!(facts.windows, None);
    assert!(!facts.drivers_exported && !facts.winget_exported);
    assert!(facts.licenses.is_empty() && facts.authenticators.is_empty());
}

#[test]
fn activation_of_windows_json() {
    let facts = |status: &str, channel: &str| {
        windows_facts(&json!({ "activation": { "status": status, "channel": channel } }))
    };
    assert_eq!(facts("licensed", "Retail").activation, Activation::Licensed);
    assert_eq!(
        facts("licensed", "Volume:GVLK").activation,
        Activation::Volume
    );
    assert_eq!(
        facts("notification", "Retail").activation,
        Activation::NotActivated
    );
    assert_eq!(
        facts("unlicensed", "Retail").activation,
        Activation::NotActivated
    );
    assert_eq!(facts("unknown", "Retail").activation, Activation::Unknown);
    let empty = windows_facts(&json!({}));
    assert_eq!(empty.activation, Activation::Unknown);
    assert!(!empty.oem_key_in_firmware);
}

#[test]
fn invalid_sibling_json_is_not_checked() {
    let dir = temp_dir();
    let system = dir.path().join("system");
    for (id, file) in [
        (windows_info::ID, windows_info::FILE),
        (disks_info::ID, disks_info::FILE),
    ] {
        std::fs::create_dir_all(system.join(id)).unwrap();
        std::fs::write(system.join(id).join(file), b"{ not json").unwrap();
    }
    let env = Environment::fake(dir.path());
    let facts = collect_facts(&env, &system.join(ID), &json!({}));
    assert_eq!(facts.bitlocker, BitLockerFacts::NotChecked);
    assert_eq!(facts.windows, None);
}

/// `run` writes only `checklist.md` into its folder and leaves the other exporters'
/// files as they were (principle P1, FR-06-02).
#[tokio::test]
async fn run_writes_only_the_checklist() {
    let dir = temp_dir();
    let system = dir.path().join("system");
    let mut env = Environment::fake(dir.path());
    env.installed_programs = programs();
    write_inputs(&system, &env);
    let before = files_under(dir.path());
    let target = system.join(ID);

    let result = run_export(&ChecklistExporter, &env, &target, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.restore_hint, RestoreHint::None);
    assert!(result.warnings.is_empty());
    assert_eq!(result.files.len(), 1);
    assert_eq!(result.files[0].rel_path, PathBuf::from(FILE));
    let text = std::fs::read_to_string(target.join(FILE)).unwrap();
    assert_eq!(result.files[0].bytes, text.len() as u64);
    assert_eq!(
        result.files[0].blake3,
        blake3::hash(text.as_bytes()).to_hex().to_string()
    );
    assert!(
        text.starts_with("# Checklist before reinstalling Windows\n"),
        "{text}"
    );
    assert!(
        text.contains("**WinRAR** (WinRAR 7.01 (64-bit)):"),
        "{text}"
    );
    assert!(text.contains("encryption on: C:."), "{text}");
    // Drivers and winget.json were exported: no items for them.
    assert!(!text.contains("## Drivers and programs"), "{text}");

    let mut after = files_under(dir.path());
    after.retain(|p| p != &Path::new("system").join(ID).join(FILE));
    assert_eq!(after, before);
}

#[tokio::test]
async fn cancelled_run_writes_nothing() {
    let dir = temp_dir();
    let env = Environment::fake(dir.path());
    let cancel = CancellationToken::new();
    cancel.cancel();
    let target = dir.path().join(ID);
    let result = run_export(&ChecklistExporter, &env, &target, &cancel).await;
    assert!(matches!(result, Err(ExportError::Cancelled)), "{result:?}");
    assert!(!target.exists());
}
