use std::path::PathBuf;

use sk_core::env::{InstalledProgram, ProgramSource};
use sk_core::model::IssueSeverity;
use sk_scan::MemFs;

use super::*;
use crate::registry::MemRegistry;
use crate::schema::RuleFile;

fn root() -> PathBuf {
    PathBuf::from(if cfg!(windows) { r"C:\fake" } else { "/fake" })
}

fn env() -> Environment {
    Environment::fake(&root())
}

/// Absolute path of `rel` (`/`-separated) under `{APPDATA}` of the fake env.
fn appdata(rel: &str) -> String {
    let base = root()
        .join("Users")
        .join("user")
        .join("AppData")
        .join("Roaming");
    rel.split('/')
        .fold(base, |p, c| p.join(c))
        .to_string_lossy()
        .into_owned()
}

/// A rule with only an id and the given YAML flow list of conditions.
fn rule(conditions: &str) -> Rule {
    let yaml =
        format!("schema_version: 1\nrules:\n  - id: app.test\n    conditions: {conditions}\n");
    match RuleFile::from_yaml(&yaml) {
        Ok(mut file) if file.rules.len() == 1 => file.rules.remove(0),
        Ok(_) => panic!("expected one rule"),
        Err(err) => panic!("invalid test rule: {err}\n{yaml}"),
    }
}

fn program(name: &str) -> InstalledProgram {
    InstalledProgram {
        name: name.to_owned(),
        publisher: None,
        version: None,
        install_location: None,
        install_date: None,
        estimated_size_kb: None,
        source: ProgramSource::Hkcu,
        uninstall_key: name.to_owned(),
    }
}

/// Test inputs; `eval` borrows them.
struct Setup {
    env: Environment,
    fs: MemFs,
    registry: MemRegistry,
    resolve: ResolveContext,
}

impl Setup {
    fn new() -> Self {
        Self {
            env: env(),
            fs: MemFs::new(),
            registry: MemRegistry::new(),
            resolve: ResolveContext::default(),
        }
    }

    fn eval(&self) -> ConditionEvaluator<'_> {
        ConditionEvaluator::new(&self.env, &self.fs, &self.registry, &self.resolve)
    }

    fn matched(&self, conditions: &str) -> bool {
        self.eval().evaluate(&rule(conditions)).matched
    }
}

#[test]
fn evaluator_is_send_and_sync() {
    fn assert_sync<T: Send + Sync>() {}
    assert_sync::<ConditionEvaluator<'_>>();
}

#[test]
fn empty_conditions_match() {
    let setup = Setup::new();
    assert_eq!(
        setup.eval().evaluate(&rule("[]")),
        ConditionOutcome {
            matched: true,
            app_running: false
        }
    );
}

#[test]
fn exists_and_not_exists() {
    let mut setup = Setup::new();
    setup.fs.add_dir(&appdata("Code/User"));
    assert!(setup.matched(r#"[ { exists: "{APPDATA}\\Code\\User" } ]"#));
    assert!(setup.matched(r#"[ { exists: "{APPDATA}\\code\\USER" } ]"#));
    assert!(!setup.matched(r#"[ { exists: "{APPDATA}\\Cursor" } ]"#));
    assert!(setup.matched(r#"[ { not_exists: "{APPDATA}\\Cursor" } ]"#));
    assert!(!setup.matched(r#"[ { not_exists: "{APPDATA}\\Code" } ]"#));
}

#[test]
fn all_conditions_must_hold() {
    let mut setup = Setup::new();
    setup.fs.add_dir(&appdata("A"));
    assert!(setup.matched(r#"[ { exists: "{APPDATA}\\A" }, { not_exists: "{APPDATA}\\B" } ]"#));
    assert!(!setup.matched(r#"[ { exists: "{APPDATA}\\A" }, { exists: "{APPDATA}\\B" } ]"#));
}

#[test]
fn exists_with_multi_valued_token_needs_any_path() {
    let mut setup = Setup::new();
    setup.resolve.steam_user_ids = vec!["111".to_owned(), "222".to_owned()];
    let conditions = r#"[ { exists: "{APPDATA}\\Steam\\{STEAM_USERID}\\config" } ]"#;
    assert!(!setup.matched(conditions));
    setup.fs.add_dir(&appdata("Steam/222/config"));
    assert!(setup.matched(conditions));
}

#[test]
fn token_without_value_does_not_exist() {
    // No `{STEAM}` in the fake environment: nothing to check, no issue.
    let setup = Setup::new();
    let eval = setup.eval();
    assert!(
        !eval
            .evaluate(&rule(r#"[ { exists: "{STEAM}\\userdata" } ]"#))
            .matched
    );
    assert!(
        eval.evaluate(&rule(r#"[ { not_exists: "{STEAM}\\userdata" } ]"#))
            .matched
    );
    assert!(eval.take_issues().is_empty());
}

#[test]
fn exists_results_are_cached_per_scan() {
    let mut setup = Setup::new();
    setup.fs.add_dir(&appdata("Code/User"));
    let eval = setup.eval();
    let first = rule(r#"[ { exists: "{APPDATA}\\Code\\User" } ]"#);
    let second =
        rule(r#"[ { not_exists: "{APPDATA}\\Code\\User" }, { exists: "{APPDATA}\\Code\\User" } ]"#);
    assert!(eval.evaluate(&first).matched);
    assert!(!eval.evaluate(&second).matched);
    assert!(eval.evaluate(&first).matched);
    assert_eq!(setup.fs.calls().exists, 1);

    // A new evaluator (a new scan) asks again.
    assert!(setup.eval().evaluate(&first).matched);
    assert_eq!(setup.fs.calls().exists, 2);
}

#[test]
fn installed_by_display_name_regex() {
    let mut setup = Setup::new();
    setup.env.installed_programs = vec![program("OBS Studio"), program("7-Zip 24.08 (x64)")];
    assert!(setup.matched(r#"[ { installed: { display_name_regex: "(?i)^obs studio" } } ]"#));
    assert!(setup.matched(r#"[ { installed: { display_name_regex: "7-Zip" } } ]"#));
    assert!(!setup.matched(r#"[ { installed: { display_name_regex: "^obs studio" } } ]"#));
    assert!(!setup.matched(r#"[ { installed: { display_name_regex: "(?i)^streamlabs" } } ]"#));
}

#[test]
fn installed_by_winget_never_matches_without_winget_ids() {
    let mut setup = Setup::new();
    setup.env.installed_programs = vec![program("OBSProject.OBSStudio")];
    assert!(!setup.matched(r#"[ { installed: { winget: "OBSProject.OBSStudio" } } ]"#));
    assert!(!setup.matched("[ { installed: {} } ]"));
    // The criteria are alternatives: the regex still decides.
    assert!(setup.matched(
        r#"[ { installed: { winget: "OBSProject.OBSStudio", display_name_regex: "(?i)obs" } } ]"#
    ));
}

#[test]
fn invalid_regex_is_false_and_reported_once() {
    let mut setup = Setup::new();
    setup.env.installed_programs = vec![program("OBS Studio")];
    setup
        .fs
        .add_file(&appdata("App/config.json"), 2, "-1d", Some(b"{}"));
    let eval = setup.eval();
    let bad = rule(r#"[ { installed: { display_name_regex: "(obs" } } ]"#);
    assert!(!eval.evaluate(&bad).matched);
    assert!(!eval.evaluate(&bad).matched);
    let bad_contains =
        rule(r#"[ { file_contains: { path: "{APPDATA}\\App\\config.json", pattern: "[" } } ]"#);
    assert!(!eval.evaluate(&bad_contains).matched);
    // The same invalid pattern in the other kind of condition: not reported again.
    let same_pattern =
        rule(r#"[ { file_contains: { path: "{APPDATA}\\App\\config.json", pattern: "(obs" } } ]"#);
    assert!(!eval.evaluate(&same_pattern).matched);

    let issues = eval.take_issues();
    assert_eq!(issues.len(), 2, "{issues:?}");
    assert!(issues.iter().all(|i| i.severity == IssueSeverity::Warning
        && i.message_key == ISSUE_INVALID_REGEX
        && i.source == "rules"));
    assert_eq!(
        issues[0].message_args.get("pattern").map(String::as_str),
        Some("(obs")
    );
    assert_eq!(
        issues[0].message_args.get("rule_id").map(String::as_str),
        Some("app.test")
    );
    assert_eq!(
        issues[1].message_args.get("pattern").map(String::as_str),
        Some("[")
    );
    assert!(eval.take_issues().is_empty());
}

#[test]
fn registry_exists() {
    let mut setup = Setup::new();
    setup
        .registry
        .add_key(RegHive::Hkcu, "Software\\SimonTatham\\PuTTY\\Sessions");
    assert!(setup.matched(
        r#"[ { registry_exists: { hive: hkcu, key: "Software\\SimonTatham\\PuTTY" } } ]"#
    ));
    assert!(setup.matched(
        r#"[ { registry_exists: { hive: hkcu, key: "\\software\\simontatham\\putty\\" } } ]"#
    ));
    assert!(!setup.matched(
        r#"[ { registry_exists: { hive: hklm, key: "Software\\SimonTatham\\PuTTY" } } ]"#
    ));
}

#[test]
fn unreadable_registry_key_is_false_with_one_info_issue() {
    let mut setup = Setup::new();
    setup
        .registry
        .add_denied_key(RegHive::Hklm, "SOFTWARE\\Vendor\\Secret");
    let eval = setup.eval();
    let condition =
        rule(r#"[ { registry_exists: { hive: hklm, key: "SOFTWARE\\Vendor\\Secret" } } ]"#);
    assert!(!eval.evaluate(&condition).matched);
    assert!(!eval.evaluate(&condition).matched);
    let issues = eval.take_issues();
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert_eq!(issues[0].severity, IssueSeverity::Info);
    assert_eq!(issues[0].message_key, ISSUE_REGISTRY_ACCESS_DENIED);
    assert_eq!(
        issues[0].message_args.get("hive").map(String::as_str),
        Some("HKLM")
    );
    assert_eq!(
        issues[0].message_args.get("key").map(String::as_str),
        Some("SOFTWARE\\Vendor\\Secret")
    );
}

#[test]
fn file_contains_matches_regex_in_content() {
    let mut setup = Setup::new();
    let content = br#"{ "telemetry": false, "product": "VSCodium" }"#;
    setup.fs.add_file(
        &appdata("App/product.json"),
        content.len() as u64,
        "-1d",
        Some(content),
    );
    let at = r#"path: "{APPDATA}\\App\\product.json""#;
    assert!(setup.matched(&format!(
        r#"[ {{ file_contains: {{ {at}, pattern: "\"telemetry\"" }} }} ]"#
    )));
    assert!(setup.matched(&format!(
        r#"[ {{ file_contains: {{ {at}, pattern: "(?i)vscod(e|ium)" }} }} ]"#
    )));
    assert!(!setup.matched(&format!(
        r#"[ {{ file_contains: {{ {at}, pattern: "Cursor" }} }} ]"#
    )));
    assert!(!setup.matched(
        r#"[ { file_contains: { path: "{APPDATA}\\App\\missing.json", pattern: "x" } } ]"#
    ));
}

#[test]
fn file_contains_respects_max_bytes() {
    let mut setup = Setup::new();
    let mut big = vec![b' '; 70_000];
    big.extend_from_slice(b"needle");
    setup
        .fs
        .add_file(&appdata("App/big.txt"), big.len() as u64, "-1d", Some(&big));
    setup
        .fs
        .add_file(&appdata("App/small.txt"), 6, "-1d", Some(b"needle"));
    // Default max_bytes is 65536: the big file is not read.
    assert!(!setup.matched(
        r#"[ { file_contains: { path: "{APPDATA}\\App\\big.txt", pattern: "needle" } } ]"#
    ));
    assert!(setup.matched(
        r#"[ { file_contains: { path: "{APPDATA}\\App\\big.txt", pattern: "needle", max_bytes: 100000 } } ]"#
    ));
    assert!(!setup.matched(
        r#"[ { file_contains: { path: "{APPDATA}\\App\\small.txt", pattern: "needle", max_bytes: 5 } } ]"#
    ));
}

#[test]
fn file_contains_does_not_read_cloud_only_files() {
    let mut setup = Setup::new();
    setup
        .fs
        .add_file(&appdata("App/cfg.txt"), 6, "-1d", Some(b"needle"));
    setup.fs.set_cloud_only(&appdata("App/cfg.txt"));
    assert!(!setup.matched(
        r#"[ { file_contains: { path: "{APPDATA}\\App\\cfg.txt", pattern: "needle" } } ]"#
    ));
}

#[test]
fn os_min_build() {
    let mut setup = Setup::new();
    // The fake environment is build 26100.2033.
    assert!(setup.matched("[ { os: { min_build: 22000 } } ]"));
    assert!(setup.matched("[ { os: { min_build: 26100 } } ]"));
    assert!(!setup.matched("[ { os: { min_build: 26101 } } ]"));
    setup.env.os.build = "19045".to_owned();
    assert!(!setup.matched("[ { os: { min_build: 22000 } } ]"));
    setup.env.os.build = "unknown".to_owned();
    assert!(!setup.matched("[ { os: { min_build: 1 } } ]"));
}

#[test]
fn any_of_is_or() {
    let mut setup = Setup::new();
    setup.fs.add_dir(&appdata("B"));
    assert!(setup
        .matched(r#"[ { any_of: [ { exists: "{APPDATA}\\A" }, { exists: "{APPDATA}\\B" } ] } ]"#));
    assert!(!setup
        .matched(r#"[ { any_of: [ { exists: "{APPDATA}\\A" }, { exists: "{APPDATA}\\C" } ] } ]"#));
    assert!(!setup.matched("[ { any_of: [] } ]"));
    // Nested inside AND.
    assert!(!setup
        .matched(r#"[ { exists: "{APPDATA}\\A" }, { any_of: [ { exists: "{APPDATA}\\B" } ] } ]"#));
}

#[test]
fn process_running_does_not_affect_matching() {
    let mut setup = Setup::new();
    setup.fs.add_dir(&appdata("obs-studio"));
    let exists = r#"{ exists: "{APPDATA}\\obs-studio" }"#;

    // Not running: still matches, no tag.
    let outcome = setup.eval().evaluate(&rule(&format!(
        r#"[ {exists}, {{ process_running: "obs64.exe" }} ]"#
    )));
    assert_eq!(
        outcome,
        ConditionOutcome {
            matched: true,
            app_running: false
        }
    );

    // Running (case-insensitive): matches with the tag.
    setup.env.running_processes = vec!["explorer.exe".to_owned(), "obs64.exe".to_owned()];
    let outcome = setup.eval().evaluate(&rule(&format!(
        r#"[ {exists}, {{ process_running: "OBS64.EXE" }} ]"#
    )));
    assert_eq!(
        outcome,
        ConditionOutcome {
            matched: true,
            app_running: true
        }
    );

    // Running but the rule does not match: no tag.
    let outcome = setup.eval().evaluate(&rule(
        r#"[ { exists: "{APPDATA}\\missing" }, { process_running: "obs64.exe" } ]"#,
    ));
    assert_eq!(outcome, ConditionOutcome::default());
}

#[test]
fn process_running_alone_or_in_any_of_is_neutral() {
    let mut setup = Setup::new();
    setup.env.running_processes = vec!["obs64.exe".to_owned()];
    let alone = setup
        .eval()
        .evaluate(&rule(r#"[ { process_running: "obs64.exe" } ]"#));
    assert!(alone.matched && alone.app_running);
    assert!(setup.matched(r#"[ { any_of: [ { process_running: "other.exe" } ] } ]"#));
    // In an OR with a deciding condition, only that condition counts.
    assert!(!setup.matched(
        r#"[ { any_of: [ { process_running: "obs64.exe" }, { exists: "{APPDATA}\\missing" } ] } ]"#
    ));
    let nested = setup.eval().evaluate(&rule(
        r#"[ { any_of: [ { process_running: "obs64.exe" }, { not_exists: "{APPDATA}\\missing" } ] } ]"#,
    ));
    assert!(nested.matched && nested.app_running);
}

#[test]
fn parse_build_takes_the_build_number() {
    assert_eq!(parse_build("26100.2033"), Some(26100));
    assert_eq!(parse_build("19045"), Some(19045));
    assert_eq!(parse_build(""), None);
    assert_eq!(parse_build("x.1"), None);
}
