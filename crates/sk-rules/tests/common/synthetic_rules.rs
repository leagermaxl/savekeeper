//! In-memory generator of a large synthetic rule base and a matching fake
//! profile (SPEC-04 §6, NFR-04-01).
//!
//! Shared by `tests/rules_perf.rs` and `benches/rules.rs` via `#[path]`.
//! The rules mimic the built-in base (SPEC-04 §4.7): path targets with
//! include/exclude, `glob_root`, registry targets, optional targets, `claims`
//! (with and without `*`), claims-only rules and the conditions `exists`,
//! `not_exists`, `any_of`, `installed` (regex), `registry_exists`, `os` and
//! `process_running`. Every rule kind needs only `exists`/`read_dir`/registry
//! probes, as NFR-04-01 assumes.
//!
//! Two of every three applications are "installed": their folders, registry
//! keys and `installed_programs` entries are in the profile, so the number of
//! findings is known in advance ([`SyntheticRules::expected_findings`]).

// Each user of the module needs only some of the items.
#![allow(dead_code)]

use std::fmt::Write as _;
use std::path::PathBuf;

use sk_core::env::{Environment, InstalledProgram, KnownFolder, ProgramSource};
use sk_core::model::RegHive;
use sk_core::registry::MemRegistry;
use sk_scan::MemFs;

/// Number of rule kinds; rule `i` has kind `i % KINDS`.
const KINDS: usize = 10;

/// Findings one installed application gives, by rule kind.
const FINDINGS_PER_KIND: [usize; KINDS] = [1, 2, 3, 1, 1, 0, 1, 1, 2, 2];

/// Programs in `installed_programs` that match no rule.
const NOISE_PROGRAMS: usize = 200;

/// Generated rule files and what running them on [`profile`] must give.
pub struct SyntheticRules {
    /// `(file name, YAML text)` of each rule file.
    pub files: Vec<(String, String)>,
    /// Total number of rules.
    pub rules: usize,
    /// Number of findings the rules give on [`profile`] with the same count.
    pub expected_findings: usize,
}

impl SyntheticRules {
    /// Total size of the YAML texts, bytes.
    pub fn bytes(&self) -> usize {
        self.files.iter().map(|(_, text)| text.len()).sum()
    }
}

/// Whether application `i` is present in the fake profile.
pub fn installed(i: usize) -> bool {
    !i.is_multiple_of(3)
}

/// Display name of application `i` in `installed_programs`.
fn program_name(i: usize) -> String {
    format!("Synthetic App {i:03}")
}

/// `count` rules split into files of `per_file` rules each.
pub fn synthetic_rules(count: usize, per_file: usize) -> SyntheticRules {
    let per_file = per_file.max(1);
    let mut files = Vec::new();
    let mut expected_findings = 0;
    let mut start = 0;
    while start < count {
        let end = (start + per_file).min(count);
        let mut text = String::from("schema_version: 1\nrules:\n");
        for i in start..end {
            push_rule(&mut text, i);
            if installed(i) {
                expected_findings += FINDINGS_PER_KIND[i % KINDS];
            }
        }
        files.push((format!("synthetic-{:02}.yaml", files.len()), text));
        start = end;
    }
    SyntheticRules {
        files,
        rules: count,
        expected_findings,
    }
}

/// Appends the YAML of rule `i`. Paths are single-quoted, so `\` is literal.
fn push_rule(out: &mut String, i: usize) {
    let app = format!("synth{i:03}");
    let vendor = format!("SynthVendor{i:03}");
    let name = program_name(i);
    let _ = write!(
        out,
        "  - id: {app}.{suffix}\n    app: {{ id: {app}, name: {name}, kind: application, winget: Synth.App{i:03} }}\n",
        suffix = if i % KINDS == 5 { "none" } else { "config" },
    );
    let _ = write!(
        out,
        "    title_key: rules.{app}.config\n    tags: [synthetic]\n    priority: {}\n",
        100 + i % 3
    );
    let body = match i % KINDS {
        // Folder with include/exclude, condition on its existence, a claim inside.
        0 => format!(
            r#"    category: app_config
    conditions:
      - exists: '{{APPDATA}}\{vendor}\App'
    targets:
      - path: '{{APPDATA}}\{vendor}\App'
        include: ["settings.json", "profiles/**", "*.ini", "plugins/config/**"]
        exclude: ["logs/**", "cache/**", "crashes/**"]
    claims:
      - '{{APPDATA}}\{vendor}\App\cache'
"#
        ),
        // Folder plus an optional registry key, labels, a running process.
        1 => format!(
            r#"    category: app_data
    sensitivity: high
    conditions:
      - process_running: {app}.exe
    targets:
      - path: '{{LOCALAPPDATA}}\{vendor}\User Data'
        include: ["*/Bookmarks", "*/Preferences", "*/Extensions/**", "*/Login Data*", "Local State"]
        label_key: rules.{app}.label_data
      - registry: {{ hive: hkcu, key: 'Software\Classes\{app}', recursive: true }}
        optional: true
        label_key: rules.{app}.label_protocol
    claims:
      - '{{LOCALAPPDATA}}\{vendor}\User Data\*\Cache'
      - '{{LOCALAPPDATA}}\{vendor}\User Data\*\Code Cache'
"#
        ),
        // Version folders found by glob_root; the service folder does not match.
        2 => format!(
            r#"    category: dev_environment
    targets:
      - path: '{{APPDATA}}\{vendor}\*20*'
        glob_root: true
        include: ["options/**", "keymaps/**", "codestyles/**", "*.key"]
    claims:
      - '{{LOCALAPPDATA}}\{vendor}'
"#
        ),
        // Registry branch, HKCU or HKLM, behind registry_exists.
        3 => {
            let hive = if i.is_multiple_of(2) { "hkcu" } else { "hklm" };
            format!(
                r#"    category: app_config
    conditions:
      - registry_exists: {{ hive: {hive}, key: 'Software\{vendor}\App' }}
    targets:
      - registry: {{ hive: {hive}, key: 'Software\{vendor}\App' }}
"#
            )
        }
        // any_of of a folder and an installed program; claims with `*`.
        4 => format!(
            r#"    category: app_config
    conditions:
      - any_of:
          - exists: '{{LOCALAPPDATA}}\{vendor}\Missing'
          - installed: {{ display_name_regex: '(?i)^synthetic app {i:03}\b' }}
    targets:
      - path: '{{LOCALAPPDATA}}\{vendor}\Settings'
        exclude: ["**/Logs/**", "Updates/**"]
    claims:
      - '{{LOCALAPPDATA}}\{vendor}\*\Temp'
"#
        ),
        // Claims only, behind the existence of the claimed folders.
        5 => format!(
            r#"    category: reinstallable
    conditions:
      - any_of:
          - exists: '{{APPDATA}}\{vendor}'
          - exists: '{{LOCALAPPDATA}}\{vendor}Cache'
    claims:
      - '{{APPDATA}}\{vendor}'
      - '{{LOCALAPPDATA}}\{vendor}Cache'
"#
        ),
        // A single credentials file.
        6 => format!(
            r#"    category: credentials
    sensitivity: high
    conditions:
      - exists: '{{HOME}}\.{app}'
    targets:
      - path: '{{HOME}}\.{app}\credentials.toml'
"#
        ),
        // Documents folder behind the OS build and a missing legacy folder.
        7 => format!(
            r#"    category: user_files
    conditions:
      - os: {{ min_build: 19041 }}
      - not_exists: '{{DOCUMENTS}}\{vendor} Legacy'
    targets:
      - path: '{{DOCUMENTS}}\{vendor}'
        exclude: ["Screenshots/**", "Logs/**"]
"#
        ),
        // One finding per profile folder.
        8 => format!(
            r#"    category: app_data
    targets:
      - path: '{{APPDATA}}\{vendor}\Profiles\*'
        glob_root: true
        include: ["prefs.js", "places.sqlite", "extensions/**"]
"#
        ),
        // Explicit root under Program Files, by installed program, plus an
        // optional file.
        _ => format!(
            r#"    category: app_config
    conditions:
      - installed: {{ display_name_regex: '(?i)^synthetic app {i:03}$' }}
    targets:
      - path: '{{PROGRAMFILES_X86}}\{vendor}\Profiles'
      - path: '{{APPDATA}}\{vendor}\extra.cfg'
        optional: true
        label_key: rules.{app}.label_extra
"#
        ),
    };
    out.push_str(&body);
}

/// A fake profile under `root` with the files, registry keys and programs of
/// the installed applications among the first `count`.
pub fn profile(root: &std::path::Path, count: usize) -> (MemFs, Environment, MemRegistry) {
    let mut env = Environment::fake(root);
    let mut fs = MemFs::new();
    let mut registry = MemRegistry::new();
    let folder = |f: KnownFolder| -> PathBuf {
        env.known_folder(f)
            .map(std::path::Path::to_path_buf)
            .unwrap_or_default()
    };
    let (appdata, local, home, docs, pf86) = (
        folder(KnownFolder::AppData),
        folder(KnownFolder::LocalAppData),
        folder(KnownFolder::Home),
        folder(KnownFolder::Documents),
        folder(KnownFolder::ProgramFilesX86),
    );
    let file = |fs: &mut MemFs, base: &PathBuf, rel: &str, size: u64| {
        let path = rel.split('/').fold(base.clone(), |p, c| p.join(c));
        fs.add_file(&path.to_string_lossy(), size, "-3d", None);
    };

    let mut programs = Vec::new();
    for i in (0..count).filter(|&i| installed(i)) {
        let vendor = format!("SynthVendor{i:03}");
        let app = format!("synth{i:03}");
        match i % KINDS {
            0 => {
                for rel in [
                    "settings.json",
                    "app.ini",
                    "profiles/main.json",
                    "logs/today.log",
                ] {
                    file(&mut fs, &appdata, &format!("{vendor}/App/{rel}"), 2048);
                }
            }
            1 => {
                for rel in [
                    "Local State",
                    "Default/Bookmarks",
                    "Default/Preferences",
                    "Default/Cache/data_0",
                    "Profile 1/Bookmarks",
                ] {
                    file(&mut fs, &local, &format!("{vendor}/User Data/{rel}"), 4096);
                }
                registry.add_key(RegHive::Hkcu, &format!("Software\\Classes\\{app}"));
            }
            2 => {
                for version in ["App2023.3", "App2024.2", "App2025.1"] {
                    file(
                        &mut fs,
                        &appdata,
                        &format!("{vendor}/{version}/options/ui.xml"),
                        512,
                    );
                }
                file(
                    &mut fs,
                    &appdata,
                    &format!("{vendor}/consentOptions/accepted"),
                    16,
                );
            }
            3 => {
                let hive = if i.is_multiple_of(2) {
                    RegHive::Hkcu
                } else {
                    RegHive::Hklm
                };
                registry.add_key(hive, &format!("Software\\{vendor}\\App\\Settings"));
            }
            4 => {
                file(
                    &mut fs,
                    &local,
                    &format!("{vendor}/Settings/config.json"),
                    1024,
                );
                programs.push(i);
            }
            5 => file(&mut fs, &appdata, &format!("{vendor}/bin/app.dll"), 1 << 20),
            6 => file(&mut fs, &home, &format!(".{app}/credentials.toml"), 256),
            7 => file(&mut fs, &docs, &format!("{vendor}/notes.txt"), 300),
            8 => {
                for profile in ["abc123.default", "def456.work"] {
                    file(
                        &mut fs,
                        &appdata,
                        &format!("{vendor}/Profiles/{profile}/prefs.js"),
                        900,
                    );
                }
            }
            _ => {
                file(
                    &mut fs,
                    &pf86,
                    &format!("{vendor}/Profiles/default.cfg"),
                    700,
                );
                file(&mut fs, &appdata, &format!("{vendor}/extra.cfg"), 100);
                programs.push(i);
            }
        }
    }

    env.installed_programs = programs
        .into_iter()
        .map(program_name)
        .chain((0..NOISE_PROGRAMS).map(|n| format!("Unrelated Program {n:03}")))
        .enumerate()
        .map(|(n, name)| InstalledProgram {
            name,
            publisher: Some("Synthetic".to_owned()),
            version: Some("1.0".to_owned()),
            install_location: None,
            install_date: None,
            estimated_size_kb: None,
            source: ProgramSource::Hkcu,
            uninstall_key: format!("{{synthetic-{n:04}}}"),
        })
        .collect();
    env.running_processes = (0..count)
        .filter(|i| i % 7 == 1)
        .map(|i| format!("synth{i:03}.exe"))
        .collect();
    (fs, env, registry)
}
