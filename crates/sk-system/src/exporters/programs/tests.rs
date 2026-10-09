use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::json;
use sk_core::env::{Environment, InstalledProgram, ProgramSource};
use sk_core::model::{AppKind, Category, EvidenceSource, FindingId, Sensitivity, Target};
use sk_core::CancellationToken;

use super::render::{self, Lang, Page, Row};
use super::winget_ids::WingetIndex;
use super::*;
use crate::exporters::test_util::{files_under, run_export, temp_dir};

fn program(name: &str, source: ProgramSource) -> InstalledProgram {
    InstalledProgram {
        name: name.to_owned(),
        publisher: None,
        version: None,
        install_location: None,
        install_date: None,
        estimated_size_kb: None,
        source,
        uninstall_key: name.to_owned(),
    }
}

fn vscode() -> InstalledProgram {
    InstalledProgram {
        publisher: Some("Microsoft Corporation".to_owned()),
        version: Some("1.94.2".to_owned()),
        install_location: Some(PathBuf::from(r"C:\Program Files\Microsoft VS Code")),
        install_date: Some("20241009".to_owned()),
        estimated_size_kb: Some(380_000),
        ..program("Microsoft Visual Studio Code", ProgramSource::Hklm)
    }
}

/// Programs of a test machine: one with every field, one needing CSV quoting, one MSIX.
fn programs() -> Vec<InstalledProgram> {
    vec![
        vscode(),
        InstalledProgram {
            publisher: Some("Igor \"7z\" Pavlov, Ltd".to_owned()),
            version: Some("23.01".to_owned()),
            estimated_size_kb: Some(512),
            ..program("7-Zip 23.01 (x64)", ProgramSource::Hklm32)
        },
        program("Windows Terminal", ProgramSource::Msix),
    ]
}

fn winget_json() -> serde_json::Value {
    json!({
        "$schema": "https://aka.ms/winget-packages.schema.2.0.json",
        "CreationDate": "2024-10-09T10:00:00.000-00:00",
        "Sources": [
            {
                "Packages": [
                    { "PackageIdentifier": "Microsoft.VisualStudioCode", "Version": "1.94.2" },
                    { "PackageIdentifier": "7zip.7zip", "Version": "23.01" },
                    { "PackageIdentifier": "Git.Git" },
                    { "PackageIdentifier": "Python.Python.3.12" },
                    { "PackageIdentifier": "Microsoft.Edge" },
                    { "PackageIdentifier": "Microsoft.EdgeWebView2Runtime" }
                ],
                "SourceDetails": { "Name": "winget", "Type": "Microsoft.PreIndexed.Package" }
            },
            {
                "Packages": [ { "PackageIdentifier": "9N0DX20HK701" } ],
                "SourceDetails": { "Name": "msstore", "Type": "Microsoft.Rest" }
            }
        ],
        "WinGetVersion": "1.9.25200"
    })
}

fn index() -> WingetIndex {
    WingetIndex::from_json(&winget_json()).unwrap()
}

#[test]
fn winget_ids_match_program_names() {
    let index = index();
    let cases = [
        (
            "Microsoft Visual Studio Code",
            Some("Microsoft.VisualStudioCode"),
        ),
        ("7-Zip 23.01 (x64)", Some("7zip.7zip")),
        ("Git", Some("Git.Git")),
        ("GitHub Desktop", None),
        ("Python 3.12.1 (64-bit)", Some("Python.Python.3.12")),
        ("Python 3.11.4 (64-bit)", None),
        ("Microsoft Edge", Some("Microsoft.Edge")),
        // The longest key wins over the shorter `edge`.
        (
            "Microsoft Edge WebView2 Runtime",
            Some("Microsoft.EdgeWebView2Runtime"),
        ),
        ("Windows Terminal", None),
        ("", None),
    ];
    for (name, expected) in cases {
        assert_eq!(index.find(name), expected, "{name}");
    }
}

#[test]
fn non_winget_json_gives_no_index() {
    assert!(WingetIndex::from_json(&json!({ "packages": [] })).is_none());
    assert!(WingetIndex::from_json(&json!([1, 2])).is_none());
    let empty = WingetIndex::from_json(&json!({ "Sources": [] })).unwrap();
    assert_eq!(empty.find("Git"), None);
}

#[test]
fn ui_language_selects_page_language() {
    assert_eq!(Lang::for_ui_language("ru-RU"), Lang::Ru);
    assert_eq!(Lang::for_ui_language("RU"), Lang::Ru);
    assert_eq!(Lang::for_ui_language("en-US"), Lang::En);
    assert_eq!(Lang::for_ui_language("uk-UA"), Lang::En);
    assert_eq!(Lang::for_ui_language(""), Lang::En);
}

fn rows<'a>(programs: &'a [InstalledProgram], index: Option<&'a WingetIndex>) -> Vec<Row<'a>> {
    programs
        .iter()
        .map(|program| Row {
            program,
            winget_id: index.and_then(|i| i.find(&program.name)),
        })
        .collect()
}

#[test]
fn csv_has_header_quoting_and_winget_column() {
    let programs = programs();
    let index = index();
    let csv = render::csv(&rows(&programs, Some(&index)), true);
    let expected = "\u{feff}name,publisher,version,install_date,install_location,estimated_size_kb,source,in_winget,winget_id\r\n\
        Microsoft Visual Studio Code,Microsoft Corporation,1.94.2,20241009,C:\\Program Files\\Microsoft VS Code,380000,hklm,yes,Microsoft.VisualStudioCode\r\n\
        7-Zip 23.01 (x64),\"Igor \"\"7z\"\" Pavlov, Ltd\",23.01,,,512,hklm32,yes,7zip.7zip\r\n\
        Windows Terminal,,,,,,msix,no,\r\n";
    assert_eq!(csv, expected);

    let csv = render::csv(&rows(&programs, None), false);
    assert!(
        csv.ends_with("Windows Terminal,,,,,,msix,,\r\n"),
        "unknown winget column is empty: {csv}"
    );
    let line_breaks = render::csv(
        &rows(&[program("Two\r\nlines", ProgramSource::Hkcu)], None),
        false,
    );
    assert!(line_breaks.contains("\"Two\r\nlines\",,,,,,hkcu,,\r\n"));
}

#[test]
fn html_lists_programs_with_search_and_escapes_text() {
    let mut programs = programs();
    programs.push(program(
        "<script>alert('x')</script> & Co",
        ProgramSource::Hkcu,
    ));
    let index = index();
    let page = Page {
        lang: Lang::En,
        machine_name: "HOME-PC",
        winget_known: true,
    };
    let html = render::html(&rows(&programs, Some(&index)), &page);

    assert!(html.starts_with("<!DOCTYPE html>\n<html lang=\"en\">"));
    assert!(html.contains("<title>Installed programs — HOME-PC</title>"));
    assert!(html.contains("<span id=\"shown\">4</span> / 4 · in winget.json: 2</p>"));
    assert!(html.contains("<input id=\"q\" type=\"search\""));
    assert!(html.contains(
        "<tr><td>Microsoft Visual Studio Code</td><td>Microsoft Corporation</td><td>1.94.2</td>\
         <td>2024-10-09</td><td class=\"num\">371.1 MB</td><td>C:\\Program Files\\Microsoft VS Code</td>\
         <td>HKLM</td><td>Microsoft.VisualStudioCode</td></tr>"
    ));
    assert!(html.contains("<td class=\"num\">512 KB</td>"));
    assert!(html.contains("<td>Igor &quot;7z&quot; Pavlov, Ltd</td>"));
    assert!(html.contains("<td>&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt; &amp; Co</td>"));
    assert!(!html.contains("<script>alert"));
    assert!(html.contains("<td>MSIX</td><td>—</td></tr>"));
    assert!(!html.contains("class=\"note\""));
    assert!(html.contains("<script>(function(){"));
}

#[test]
fn html_in_russian_without_winget() {
    let programs = vec![InstalledProgram {
        estimated_size_kb: Some(3 * 1024 * 1024 / 2),
        ..program("Яндекс Браузер", ProgramSource::Hkcu)
    }];
    let page = Page {
        lang: Lang::Ru,
        machine_name: "ДОМ",
        winget_known: false,
    };
    let html = render::html(&rows(&programs, None), &page);
    assert!(html.contains("<html lang=\"ru\">"));
    assert!(html.contains("<h1>Установленные программы</h1>"));
    assert!(html.contains("ДОМ · Показано: <span id=\"shown\">1</span> / 1</p>"));
    assert!(html.contains("<p class=\"note\">winget.json не создан"));
    assert!(html.contains("<td class=\"num\">1,5 ГБ</td>"));
    assert!(html.contains("<td>HKCU</td><td></td></tr>"));
}

#[test]
fn probe_is_available_with_program_count() {
    let mut env = Environment::fake(Path::new("/fake"));
    env.installed_programs = programs();
    assert_eq!(
        ProgramsExporter::probe(&env),
        Availability::Available {
            estimated_bytes: Some(ESTIMATE_BASE_BYTES + 3 * ESTIMATE_BYTES_PER_PROGRAM),
            details: BTreeMap::from([("programs".to_owned(), "3".to_owned())]),
        }
    );
}

#[cfg(not(windows))]
#[test]
fn detect_off_windows_is_not_windows() {
    let env = Environment::fake(Path::new("/fake"));
    assert_eq!(ProgramsExporter.detect(&env), unavailable(NOT_WINDOWS));
}

#[test]
fn plan_is_system_settings_finding() {
    let env = Environment::fake(Path::new("/fake"));
    let finding = ProgramsExporter
        .plan(&env, &ProgramsExporter::probe(&env))
        .unwrap();
    let target = Target::SystemExport {
        exporter_id: "programs".to_owned(),
        params: json!({}),
    };
    assert_eq!(finding.id, FindingId::for_target(&target));
    assert_eq!(finding.target, target);
    assert_eq!(finding.category, Category::SystemSettings);
    assert_eq!(finding.sensitivity, Sensitivity::None);
    assert!(!finding.requires_elevation);
    assert_eq!(finding.title, TITLE);
    assert_eq!(finding.app.as_ref().map(|a| a.kind), Some(AppKind::System));
    assert_eq!(
        finding.evidence[0].source,
        EvidenceSource::System {
            exporter_id: "programs".to_owned()
        }
    );
    assert_eq!(finding.evidence[0].message_args["programs"], "0");
    assert!(ProgramsExporter
        .plan(&env, &unavailable(NOT_WINDOWS))
        .is_none());
}

/// `<dir>/system/{winget,programs}` of a backup, with `winget.json` if given.
fn backup(dir: &Path, winget: Option<&[u8]>) -> PathBuf {
    let system = dir.join("backup").join("system");
    if let Some(content) = winget {
        std::fs::create_dir_all(system.join("winget")).unwrap();
        std::fs::write(system.join("winget").join("winget.json"), content).unwrap();
    }
    system.join(ID)
}

fn env_with_programs() -> Environment {
    let mut env = Environment::fake(Path::new("/fake"));
    // Not sorted: the export sorts by name.
    let mut programs = programs();
    programs.reverse();
    env.installed_programs = programs;
    env
}

#[tokio::test]
async fn run_writes_csv_and_html_with_winget_column() {
    let dir = temp_dir();
    let mut winget = b"\xEF\xBB\xBF".to_vec();
    winget.extend(serde_json::to_vec(&winget_json()).unwrap());
    let target = backup(dir.path(), Some(&winget));
    let env = env_with_programs();

    let result = run_export(&ProgramsExporter, &env, &target, &CancellationToken::new())
        .await
        .unwrap();

    let names: Vec<_> = result.files.iter().map(|f| f.rel_path.clone()).collect();
    assert_eq!(names, [PathBuf::from(CSV_FILE), PathBuf::from(HTML_FILE)]);
    for file in &result.files {
        let content = std::fs::read(target.join(&file.rel_path)).unwrap();
        assert_eq!(file.bytes, content.len() as u64);
        assert_eq!(file.blake3, blake3::hash(&content).to_hex().to_string());
    }
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_eq!(
        result.restore_hint,
        RestoreHint::Manual {
            key: RESTORE_KEY.to_owned()
        }
    );

    let csv = std::fs::read_to_string(target.join(CSV_FILE)).unwrap();
    let lines: Vec<&str> = csv.lines().skip(1).collect();
    assert!(lines[0].starts_with("7-Zip 23.01 (x64),"), "{csv}");
    assert!(lines[0].ends_with(",yes,7zip.7zip"), "{csv}");
    assert!(
        lines[1].starts_with("Microsoft Visual Studio Code,"),
        "{csv}"
    );
    assert!(lines[2].ends_with(",msix,no,"), "{csv}");

    // P1: only the target folder is written; winget.json was there before.
    assert_eq!(
        files_under(&dir.path().join("backup")),
        [
            PathBuf::from("system").join("programs").join(CSV_FILE),
            PathBuf::from("system").join("programs").join(HTML_FILE),
            PathBuf::from("system").join("winget").join("winget.json"),
        ]
    );
}

#[tokio::test]
async fn run_without_winget_json_leaves_column_empty() {
    let dir = temp_dir();
    let target = backup(dir.path(), None);
    let env = env_with_programs();

    let result = run_export(&ProgramsExporter, &env, &target, &CancellationToken::new())
        .await
        .unwrap();
    assert!(result.warnings.is_empty());
    let csv = std::fs::read_to_string(target.join(CSV_FILE)).unwrap();
    assert!(
        csv.lines().skip(1).all(|line| line.ends_with(",,")),
        "{csv}"
    );
    let html = std::fs::read_to_string(target.join(HTML_FILE)).unwrap();
    assert!(html.contains("class=\"note\""));
}

#[tokio::test]
async fn run_with_broken_winget_json_warns() {
    for content in [&b"{ not json"[..], &br#"{"packages": []}"#[..]] {
        let dir = temp_dir();
        let target = backup(dir.path(), Some(content));
        let env = env_with_programs();

        let result = run_export(&ProgramsExporter, &env, &target, &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(result.files.len(), 2);
        assert_eq!(result.warnings.len(), 1);
        assert_eq!(result.warnings[0].message_key, ISSUE_WINGET_UNREADABLE);
        assert!(result.warnings[0].message_args.contains_key("error"));
    }
}

#[tokio::test]
async fn run_with_no_programs_writes_empty_list() {
    let dir = temp_dir();
    let target = backup(dir.path(), None);
    let env = Environment::fake(Path::new("/fake"));

    let result = run_export(&ProgramsExporter, &env, &target, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.files.len(), 2);
    let csv = std::fs::read_to_string(target.join(CSV_FILE)).unwrap();
    assert_eq!(csv.lines().count(), 1, "header only: {csv}");
}

#[tokio::test]
async fn cancelled_run_writes_nothing() {
    let dir = temp_dir();
    let target = backup(dir.path(), None);
    let cancel = CancellationToken::new();
    cancel.cancel();

    let result = run_export(&ProgramsExporter, &env_with_programs(), &target, &cancel).await;
    assert!(matches!(result, Err(ExportError::Cancelled)), "{result:?}");
    assert!(!target.exists());
}
