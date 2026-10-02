//! `savekeeper-cli` end to end (SPEC-01 §4.9, §6; SPEC-12 §4.1).
//!
//! The binary is copied into a temporary folder for every test, so its
//! portable data folder (config, logs, reports) is created there and not next
//! to the build output.

// Helpers outside `#[test]` functions are not covered by `allow-unwrap-in-tests`.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use assert_cmd::Command;
use predicates::prelude::*;
use predicates::str::contains;
use sk_core::model::{FolderSummary, Marker, ScanReport};
use tempfile::TempDir;

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

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(&self.exe);
        cmd.args(args).current_dir(self.path()).env_remove("SK_LOG");
        cmd
    }
}

#[test]
fn scan_out_writes_a_valid_report() {
    let cli = Cli::new();
    cli.cmd(&["scan", "--out", "r.json", "--no-games"])
        .assert()
        .code(0)
        .stdout("");

    let text = std::fs::read_to_string(cli.path().join("r.json")).unwrap();
    let report = ScanReport::from_json(&text).unwrap();
    assert_eq!(report.schema_version, ScanReport::SCHEMA_VERSION);
    assert!(report.findings.is_empty());
    assert!(report.issues.is_empty());
    assert!(!report.options.collectors.games);
    assert!(report.options.collectors.system);

    // Portable mode: config, log and saved report next to the program.
    assert!(cli.path().join("savekeeper.config.json").is_file());
    let scans = cli.path().join("savekeeper-data").join("scans");
    let saved = scans.join(format!("{}.json", report.scan_id));
    assert!(saved.is_file(), "{} missing", saved.display());
    let logs = std::fs::read_dir(cli.path().join("savekeeper-data").join("logs")).unwrap();
    assert!(logs.count() > 0);
}

#[test]
fn scan_prints_the_report_without_out() {
    let cli = Cli::new();
    let output = cli.cmd(&["scan", "--pretty"]).assert().code(0);
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    assert!(stdout.starts_with("{\n"));
    let report = ScanReport::from_json(&stdout).unwrap();
    assert!(report.findings.is_empty());
}

#[test]
fn config_path_is_next_to_the_program() {
    let cli = Cli::new();
    let output = cli.cmd(&["config", "path"]).assert().code(0);
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    let path = PathBuf::from(stdout.trim());
    assert_eq!(path.file_name().unwrap(), "savekeeper.config.json");
    assert_eq!(
        path.parent().unwrap().canonicalize().unwrap(),
        cli.path().canonicalize().unwrap()
    );
}

#[test]
fn config_show_prints_and_creates_the_config() {
    let cli = Cli::new();
    let output = cli.cmd(&["config", "show"]).assert().code(0);
    let json: serde_json::Value = serde_json::from_slice(&output.get_output().stdout).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["llm"]["mode"], "off");
    assert!(cli.path().join("savekeeper.config.json").is_file());
}

#[test]
fn config_warnings_go_to_stderr() {
    let cli = Cli::new();
    std::fs::write(
        cli.path().join("savekeeper.config.json"),
        r#"{ "schema_version": 1, "llm": { "locl": 1 } }"#,
    )
    .unwrap();
    cli.cmd(&["config", "show"])
        .assert()
        .code(0)
        .stderr(contains("llm.locl"));
}

#[test]
fn env_prints_the_environment() {
    let cli = Cli::new();
    let output = cli.cmd(&["env"]).assert().code(0);
    let json: serde_json::Value = serde_json::from_slice(&output.get_output().stdout).unwrap();
    assert!(json["known_folders"]["HOME"].is_string());
    assert!(json["os"]["arch"].is_string());
}

/// A folder tree for `debug summarize` in its own temporary folder:
/// `proj` with 3 counted files (10 bytes) in 2 folders, a `.git` folder and
/// an excluded `node_modules`.
fn summary_tree() -> TempDir {
    let tree = tempfile::tempdir().unwrap();
    let proj = tree.path().join("proj");
    for (path, content) in [
        ("a.txt", "hello"),
        ("sub/b.json", "{}"),
        (".git/HEAD", "ref"),
        ("node_modules/x.js", "module.exports = 1;"),
    ] {
        let path = proj.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    tree
}

fn parse_summary(stdout: &[u8]) -> FolderSummary {
    serde_json::from_slice(stdout).unwrap()
}

#[test]
fn debug_summarize_prints_the_folder_summary() {
    let cli = Cli::new();
    let tree = summary_tree();
    let proj = tree.path().join("proj");
    let output = cli
        .cmd(&["debug", "summarize", proj.to_str().unwrap()])
        .assert()
        .code(0);
    let stdout = &output.get_output().stdout;
    // Compact: one line.
    assert!(stdout.starts_with(b"{\"path\":"));
    assert_eq!(stdout.iter().filter(|&&b| b == b'\n').count(), 1);
    let summary = parse_summary(stdout);
    assert_eq!(summary.file_count, 3);
    assert_eq!(summary.dir_count, 2);
    assert_eq!(summary.total_bytes, 10);
    assert!(summary.markers.contains(&Marker::GitRepo));
    assert!(!summary.truncated);
    assert!(summary
        .sample_names
        .iter()
        .all(|n| !n.contains("node_modules")));
    assert!(summary
        .top_children
        .iter()
        .all(|c| c.name != "node_modules"));

    // The source is not changed; no log is written.
    assert!(proj.join("node_modules").join("x.js").is_file());
    assert!(!cli.path().join("savekeeper-data").join("logs").exists());
}

#[test]
fn debug_summarize_pretty() {
    let cli = Cli::new();
    let tree = summary_tree();
    let proj = tree.path().join("proj");
    let output = cli
        .cmd(&["debug", "summarize", "--pretty", proj.to_str().unwrap()])
        .assert()
        .code(0);
    let stdout = &output.get_output().stdout;
    assert!(stdout.starts_with(b"{\n  \"path\":"));
    assert_eq!(parse_summary(stdout).file_count, 3);
}

#[test]
fn debug_summarize_resolves_a_relative_path_from_the_current_folder() {
    let cli = Cli::new();
    let tree = summary_tree();
    let mut cmd = cli.cmd(&["debug", "summarize", "proj"]);
    let output = cmd.current_dir(tree.path()).assert().code(0);
    let summary = parse_summary(&output.get_output().stdout);
    assert_eq!(summary.file_count, 3);
    assert!(summary.markers.contains(&Marker::GitRepo));
}

#[test]
fn debug_summarize_fails_on_a_missing_path_or_a_file() {
    let cli = Cli::new();
    let tree = summary_tree();
    let proj = tree.path().join("proj");
    let missing = tree.path().join("missing");
    cli.cmd(&["debug", "summarize", missing.to_str().unwrap()])
        .assert()
        .code(1)
        .stdout("")
        .stderr(contains("cannot summarize").and(contains("not found")));
    let file = proj.join("a.txt");
    cli.cmd(&["debug", "summarize", file.to_str().unwrap()])
        .assert()
        .code(1)
        .stdout("")
        .stderr(contains("cannot summarize").and(contains("is a file")));
}

#[test]
fn debug_summarize_applies_the_user_exclude_globs() {
    let cli = Cli::new();
    let tree = summary_tree();
    let proj = tree.path().join("proj");
    let config = cli.path().join("savekeeper.config.json");

    std::fs::write(
        &config,
        r#"{ "schema_version": 1, "scan": { "exclude_globs": ["*.json"] } }"#,
    )
    .unwrap();
    let output = cli
        .cmd(&["debug", "summarize", proj.to_str().unwrap()])
        .assert()
        .code(0);
    assert_eq!(parse_summary(&output.get_output().stdout).file_count, 2);

    // An invalid glob: a warning, and only the built-in exclusions.
    std::fs::write(
        &config,
        r#"{ "schema_version": 1, "scan": { "exclude_globs": ["[", "*.json"] } }"#,
    )
    .unwrap();
    let output = cli
        .cmd(&["debug", "summarize", proj.to_str().unwrap()])
        .assert()
        .code(0)
        .stderr(contains("invalid scan.exclude_globs"));
    assert_eq!(parse_summary(&output.get_output().stdout).file_count, 3);
}

/// Serializes the tests that run `backup`: it takes the system-wide
/// single-instance lock, which one of them holds on purpose.
static BACKUP_LOCK: Mutex<()> = Mutex::new(());

fn backup_lock() -> MutexGuard<'static, ()> {
    BACKUP_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

#[test]
fn unimplemented_commands_fail_with_a_message() {
    let _backup = backup_lock();
    let cli = Cli::new();
    for (args, name) in [
        (
            &["backup", "--report", "r.json", "--to", "b"][..],
            "`backup`",
        ),
        (&["rules", "validate", "a.yaml"], "`rules validate`"),
        (&["manifest", "update"], "`manifest update`"),
    ] {
        cli.cmd(args)
            .assert()
            .code(1)
            .stderr(contains(name).and(contains("not implemented yet")));
    }
}

/// SPEC-01 §5, T-01-08: a second instance running `backup` exits with 1.
#[cfg(windows)]
#[test]
fn backup_fails_while_another_instance_runs() {
    let _backup = backup_lock();
    let cli = Cli::new();
    let first = sk_core::win::single_instance::acquire().unwrap();
    cli.cmd(&["backup", "--report", "r.json", "--to", "b"])
        .assert()
        .code(1)
        .stderr(contains("another SaveKeeper instance is running"));
    drop(first);
    // Without the other instance it gets past the lock.
    cli.cmd(&["backup", "--report", "r.json", "--to", "b"])
        .assert()
        .code(1)
        .stderr(contains("not implemented yet"));
}

#[test]
fn usage_errors_exit_with_1() {
    let cli = Cli::new();
    cli.cmd(&["frobnicate"]).assert().code(1);
    cli.cmd(&["scan", "--llm", "remote"]).assert().code(1);
    cli.cmd(&[]).assert().code(1);
}

#[test]
fn help_and_version_succeed() {
    let cli = Cli::new();
    cli.cmd(&["--help"])
        .assert()
        .code(0)
        .stdout(contains("scan").and(contains("config")));
    cli.cmd(&["--version"])
        .assert()
        .code(0)
        .stdout(contains(env!("CARGO_PKG_VERSION")));
}
