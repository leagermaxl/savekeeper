//! `savekeeper-cli rules validate` end to end (SPEC-04 FR-04-08, SPEC-01 §4.9).
//!
//! Rule files are written into temporary folders; the binary is copied into
//! its own temporary folder so that its portable data folder (`rules.d`) is
//! there and not next to the build output.

// Helpers outside `#[test]` functions are not covered by `allow-unwrap-in-tests`.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use predicates::prelude::*;
use predicates::str::contains;
use tempfile::TempDir;

const VALID: &str = r#"schema_version: 1
rules:
  - id: app.one
    app: { id: app, name: App, kind: application }
    category: app_config
    title_key: rules.app.one
    targets: [ { path: "{APPDATA}\\App" } ]
"#;

/// Line 6 is a misspelled field of rule `app.two`.
const UNKNOWN_FIELD: &str = "schema_version: 1
rules:
  - { id: app.one, disabled: true }
  - id: app.two
    category: app_config
    titel: X
";

/// Valid, with an automatic fix: `credentials` is raised to sensitivity `high`.
const CREDENTIALS: &str = r#"schema_version: 1
rules:
  - id: keys.ssh
    app: { id: keys, name: Keys, kind: dev_tool }
    category: credentials
    title_key: rules.keys.ssh
    targets: [ { path: "{HOME}\\.ssh" } ]
"#;

/// A copy of the CLI in its own folder.
struct Cli {
    dir: TempDir,
    exe: PathBuf,
}

impl Cli {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir
            .path()
            .join(format!("savekeeper-cli{}", std::env::consts::EXE_SUFFIX));
        std::fs::copy(env!("CARGO_BIN_EXE_savekeeper-cli"), &exe).unwrap();
        Self { dir, exe }
    }

    fn validate(&self, paths: &[&Path]) -> Command {
        let mut cmd = Command::new(&self.exe);
        cmd.args(["rules", "validate"])
            .args(paths)
            .current_dir(self.dir.path())
            .env_remove("SK_LOG");
        cmd
    }

    /// The user rule folder of this copy (portable mode).
    fn rules_d(&self) -> PathBuf {
        self.dir.path().join("savekeeper-data").join("rules.d")
    }
}

/// Writes `files` into a new temporary folder.
fn rule_dir(files: &[(&str, &str)]) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (name, text) in files {
        std::fs::write(dir.path().join(name), text).unwrap();
    }
    dir
}

fn stdout(output: &assert_cmd::assert::Assert) -> String {
    String::from_utf8(output.get_output().stdout.clone()).unwrap()
}

#[test]
fn valid_file_prints_nothing_and_succeeds() {
    let cli = Cli::new();
    let dir = rule_dir(&[("ok.yaml", VALID)]);
    cli.validate(&[&dir.path().join("ok.yaml")])
        .assert()
        .code(0)
        .stdout("")
        .stderr(contains("1 rule file checked, 0 errors, 0 warnings"));
}

#[test]
fn errors_have_file_line_rule_and_exit_with_1() {
    let cli = Cli::new();
    let dir = rule_dir(&[("bad.yaml", UNKNOWN_FIELD)]);
    let file = dir.path().join("bad.yaml");
    let output = cli.validate(&[&file]).assert().code(1).stderr(
        contains("1 rule file checked, 1 error, 0 warnings")
            .and(contains("error: 1 rule file is invalid")),
    );
    let stdout = stdout(&output);
    let prefix = format!("{}:6: error: rule `app.two`: ", file.display());
    assert!(stdout.starts_with(&prefix), "{stdout}");
    assert!(stdout.contains("titel"), "{stdout}");
    assert_eq!(stdout.lines().count(), 1, "{stdout}");
}

#[test]
fn warnings_only_exit_with_3() {
    let cli = Cli::new();
    let dir = rule_dir(&[("keys.yaml", CREDENTIALS)]);
    let file = dir.path().join("keys.yaml");
    let output = cli
        .validate(&[&file])
        .assert()
        .code(3)
        .stderr(contains("1 rule file checked, 0 errors, 1 warning"));
    let stdout = stdout(&output);
    let prefix = format!("{}:3: warning: rule `keys.ssh`: ", file.display());
    assert!(stdout.starts_with(&prefix), "{stdout}");
    assert!(stdout.contains("high"), "{stdout}");
}

#[test]
fn folders_and_files_are_checked_together() {
    let cli = Cli::new();
    let dir = rule_dir(&[
        ("b.yml", CREDENTIALS),
        ("a.YAML", UNKNOWN_FIELD),
        ("notes.txt", "not a rule file"),
    ]);
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    std::fs::write(dir.path().join("sub").join("c.yaml"), UNKNOWN_FIELD).unwrap();
    let extra = rule_dir(&[("ok.yaml", VALID)]);
    let missing = extra.path().join("missing.yaml");

    let output = cli
        .validate(&[dir.path(), &extra.path().join("ok.yaml"), &missing])
        .assert()
        .code(1)
        .stderr(
            contains("4 rule files checked, 2 errors, 1 warning")
                .and(contains("2 rule files are invalid")),
        );
    let stdout = stdout(&output);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 3, "{stdout}");
    // Folder files in load order, then the other arguments; no subfolders.
    let a = dir.path().join("a.YAML");
    assert!(lines[0].starts_with(&format!("{}:6: error", a.display())));
    let b = dir.path().join("b.yml");
    assert!(lines[1].starts_with(&format!("{}:3: warning", b.display())));
    assert!(lines[2].starts_with(&format!("{}: error: cannot read file", missing.display())));
    assert!(!stdout.contains("c.yaml") && !stdout.contains("notes.txt"));

    // The rule files are not changed (P1).
    assert_eq!(std::fs::read_to_string(&b).unwrap(), CREDENTIALS);
}

#[test]
fn without_paths_the_user_rule_folder_is_checked() {
    let cli = Cli::new();
    // No `rules.d` yet: nothing to check.
    cli.validate(&[])
        .assert()
        .code(0)
        .stdout("")
        .stderr(contains("0 rule files checked, 0 errors, 0 warnings"));

    std::fs::create_dir_all(cli.rules_d()).unwrap();
    std::fs::write(cli.rules_d().join("mine.yaml"), VALID).unwrap();
    cli.validate(&[])
        .assert()
        .code(0)
        .stderr(contains("1 rule file checked"));

    std::fs::write(cli.rules_d().join("broken.yaml"), "schema_version: [").unwrap();
    let output = cli.validate(&[]).assert().code(1);
    let stdout = stdout(&output);
    assert!(stdout.contains("broken.yaml:"), "{stdout}");
    assert!(stdout.contains(": error: "), "{stdout}");
    assert!(!stdout.contains("mine.yaml"), "{stdout}");
}
