use std::fs;

use sk_core::model::{Category, Sensitivity};

use super::*;

/// One enabled rule with this id and priority.
fn rule_yaml(id: &str, priority: i32, title: &str) -> String {
    format!(
        "  - id: {id}\n    app: {{ id: app, name: App, kind: application }}\n    category: app_config\n    title: {title}\n    priority: {priority}\n    targets: [ {{ path: \"{{APPDATA}}\\\\App\" }} ]\n"
    )
}

fn file_yaml(rules: &[String]) -> String {
    format!("schema_version: 1\nrules:\n{}", rules.concat())
}

fn builtin_set(files: &[(&str, &str)]) -> RuleSet {
    let mut merger = Merger::default();
    if let Err(err) = merger.add_builtin(files) {
        panic!("builtin rules rejected: {err}");
    }
    merger.finish()
}

/// Synthetic built-in files merged with the user files of `dir`.
fn merged(builtin: &[(&str, &str)], dir: &Path) -> (RuleSet, Vec<ScanIssue>) {
    let mut merger = Merger::default();
    if let Err(err) = merger.add_builtin(builtin) {
        panic!("builtin rules rejected: {err}");
    }
    let mut issues = Vec::new();
    merger.add_user_dir(dir, &mut issues);
    (merger.finish(), issues)
}

fn ids(set: &RuleSet) -> Vec<&str> {
    set.rules.iter().map(CompiledRule::id).collect()
}

fn title(set: &RuleSet, id: &str) -> Option<String> {
    set.get(id).and_then(|rule| rule.rule.title.clone())
}

/// FR-04-07: every embedded `rules/*.yaml` compiles without errors or
/// automatic fixes, ids are unique across files, `credentials` ⇒ `high`.
#[test]
fn builtin_rules_valid() {
    let files = match builtin_files() {
        Ok(files) => files,
        Err(err) => panic!("cannot read built-in rules: {err}"),
    };
    for (name, text) in &files {
        let diagnostics = diagnostic::validate_str(Path::new(name), text);
        assert!(
            diagnostics.is_empty(),
            "built-in rule file {name} has problems: {diagnostics:#?}"
        );
    }
    let set = match RuleSet::builtin() {
        Ok(set) => set,
        Err(err) => panic!("built-in rules rejected: {err}"),
    };
    for rule in &set.rules {
        for target in &rule.targets {
            if target.category == Category::Credentials {
                assert_eq!(target.sensitivity, Sensitivity::High, "{}", rule.id());
            }
        }
        assert!(!set.source(rule.id()).is_some_and(RuleSource::is_user));
    }
    let (loaded, issues) = RuleSet::load(true, None);
    assert_eq!(issues, vec![]);
    assert_eq!(loaded.len(), set.len());
}

#[test]
fn empty_load() {
    let (set, issues) = RuleSet::load(false, None);
    assert!(set.is_empty());
    assert_eq!(set.len(), 0);
    assert!(set.get("app.one").is_none());
    assert_eq!(issues, vec![]);
}

#[test]
fn missing_user_dir_is_not_an_issue() {
    let tmp = tempfile::tempdir().unwrap();
    let (set, issues) = RuleSet::load(false, Some(&tmp.path().join("rules.d")));
    assert!(set.is_empty());
    assert_eq!(issues, vec![]);
}

#[test]
fn rules_ordered_by_priority_desc_then_id() {
    let a = file_yaml(&[rule_yaml("app.b", 100, "B"), rule_yaml("app.low", 5, "L")]);
    let b = file_yaml(&[
        rule_yaml("app.a", 100, "A"),
        rule_yaml("app.high", 200, "H"),
    ]);
    let set = builtin_set(&[("a.yaml", &a), ("b.yaml", &b)]);
    assert_eq!(ids(&set), ["app.high", "app.a", "app.b", "app.low"]);
    for id in ["app.high", "app.a", "app.b", "app.low"] {
        assert_eq!(set.get(id).map(CompiledRule::id), Some(id));
    }
    assert_eq!(
        set.source("app.a"),
        Some(&RuleSource::Builtin {
            file: "b.yaml".into()
        })
    );
}

#[test]
fn duplicate_id_across_builtin_files_is_an_error() {
    let a = file_yaml(&[rule_yaml("app.one", 100, "A")]);
    let b = file_yaml(&[rule_yaml("app.one", 100, "B")]);
    let mut merger = Merger::default();
    let err = merger.add_builtin(&[("a.yaml", &a), ("b.yaml", &b)]);
    assert!(matches!(err, Err(RuleError::DuplicateId(id)) if id == "app.one"));
    assert!(merger.finish().is_empty(), "nothing is added on error");
}

#[test]
fn invalid_builtin_file_is_an_error() {
    let mut merger = Merger::default();
    let err = merger.add_builtin(&[("a.yaml", "schema_version: 2\nrules: []\n")]);
    assert!(matches!(err, Err(RuleError::Invalid { .. })), "{err:?}");
}

#[test]
fn disabled_builtin_rule_is_not_active() {
    let text = file_yaml(&[
        rule_yaml("app.one", 100, "A"),
        "  - { id: app.two, disabled: true }\n".to_owned(),
    ]);
    let set = builtin_set(&[("a.yaml", &text)]);
    assert_eq!(ids(&set), ["app.one"]);
}

#[test]
fn user_rule_replaces_builtin() {
    let builtin = file_yaml(&[
        rule_yaml("app.one", 100, "Builtin"),
        rule_yaml("app.two", 100, "T"),
    ]);
    let tmp = tempfile::tempdir().unwrap();
    let user = file_yaml(&[rule_yaml("app.one", 100, "Mine")]);
    fs::write(tmp.path().join("mine.yaml"), user).unwrap();

    let (set, issues) = merged(&[("a.yaml", &builtin)], tmp.path());
    assert_eq!(issues, vec![]);
    assert_eq!(ids(&set), ["app.one", "app.two"]);
    assert_eq!(title(&set, "app.one").as_deref(), Some("Mine"));
    assert_eq!(
        set.source("app.one"),
        Some(&RuleSource::User {
            file: tmp.path().join("mine.yaml")
        })
    );
    assert!(set.source("app.one").is_some_and(RuleSource::is_user));
    assert!(!set.source("app.two").is_some_and(RuleSource::is_user));
}

#[test]
fn user_rule_disables_builtin() {
    let builtin = file_yaml(&[
        rule_yaml("app.one", 100, "A"),
        rule_yaml("app.two", 100, "T"),
    ]);
    let tmp = tempfile::tempdir().unwrap();
    let user = "schema_version: 1\nrules: [ { id: app.one, disabled: true }, { id: app.unknown, disabled: true } ]\n";
    fs::write(tmp.path().join("off.yml"), user).unwrap();

    let (set, issues) = merged(&[("a.yaml", &builtin)], tmp.path());
    assert_eq!(issues, vec![]);
    assert_eq!(ids(&set), ["app.two"]);
    assert!(set.get("app.one").is_none());
    assert!(set.source("app.one").is_none());
}

#[test]
fn duplicate_user_id_later_file_wins_with_warning() {
    let tmp = tempfile::tempdir().unwrap();
    let first = file_yaml(&[rule_yaml("app.one", 100, "First")]);
    let second = file_yaml(&[rule_yaml("app.one", 100, "Second")]);
    // Alphabetical, case-insensitive: "a.yaml" < "B.yaml".
    fs::write(tmp.path().join("B.yaml"), second).unwrap();
    fs::write(tmp.path().join("a.yaml"), first).unwrap();

    let (set, issues) = RuleSet::load(false, Some(tmp.path()));
    assert_eq!(title(&set, "app.one").as_deref(), Some("Second"));
    let [issue] = issues.try_into().unwrap();
    assert_eq!(issue.severity, IssueSeverity::Warning);
    assert_eq!(issue.source, "rules");
    assert_eq!(issue.message_key, ISSUE_DUPLICATE_ID);
    assert_eq!(issue.message_args["rule_id"], "app.one");
    assert_eq!(issue.message_args["file"], "B.yaml");
    assert_eq!(issue.message_args["previous_file"], "a.yaml");
}

#[test]
fn invalid_user_file_is_skipped_with_warning() {
    let tmp = tempfile::tempdir().unwrap();
    // Line 5: unknown field in the first rule; the whole file is skipped.
    let bad = format!(
        "schema_version: 1\nrules:\n  - id: app.bad\n    category: app_config\n    titel: X\n{}",
        rule_yaml("app.also-skipped", 100, "S")
    );
    fs::write(tmp.path().join("bad.yaml"), bad).unwrap();
    let good = file_yaml(&[rule_yaml("app.good", 100, "G")]);
    fs::write(tmp.path().join("good.yaml"), good).unwrap();

    let (set, issues) = RuleSet::load(false, Some(tmp.path()));
    assert_eq!(ids(&set), ["app.good"]);
    let [issue] = issues.try_into().unwrap();
    assert_eq!(issue.severity, IssueSeverity::Warning);
    assert_eq!(issue.source, "rules");
    assert_eq!(issue.path, None);
    assert_eq!(issue.message_key, ISSUE_INVALID_FILE);
    assert_eq!(issue.message_args["file"], "bad.yaml");
    assert_eq!(issue.message_args["line"], "5");
    assert_eq!(issue.message_args["rule_id"], "app.bad");
    assert!(issue.message_args["error"].contains("titel"));
}

#[test]
fn validation_error_in_user_file_points_at_rule() {
    let tmp = tempfile::tempdir().unwrap();
    let text = file_yaml(&[
        rule_yaml("app.one", 100, "A"),
        rule_yaml("App.Two", 100, "B"),
    ]);
    fs::write(tmp.path().join("ids.yaml"), text).unwrap();

    let (set, issues) = RuleSet::load(false, Some(tmp.path()));
    assert!(set.is_empty());
    let [issue] = issues.try_into().unwrap();
    assert_eq!(issue.message_key, ISSUE_INVALID_FILE);
    assert_eq!(issue.message_args["line"], "9");
    assert_eq!(issue.message_args["rule_id"], "App.Two");
}

#[test]
fn only_yaml_files_directly_in_user_dir_are_loaded() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("notes.txt"), "not yaml").unwrap();
    fs::write(tmp.path().join("README.md"), "# rules").unwrap();
    fs::create_dir(tmp.path().join("sub")).unwrap();
    let nested = file_yaml(&[rule_yaml("app.nested", 100, "N")]);
    fs::write(tmp.path().join("sub").join("nested.yaml"), nested).unwrap();
    let upper = file_yaml(&[rule_yaml("app.upper", 100, "U")]);
    fs::write(tmp.path().join("UPPER.YAML"), upper).unwrap();
    let yml = file_yaml(&[rule_yaml("app.yml", 100, "Y")]);
    fs::write(tmp.path().join("short.yml"), yml).unwrap();

    let (set, issues) = RuleSet::load(false, Some(tmp.path()));
    assert_eq!(issues, vec![]);
    assert_eq!(ids(&set), ["app.upper", "app.yml"]);
}

#[test]
fn compile_warnings_do_not_reject_user_file() {
    let tmp = tempfile::tempdir().unwrap();
    let text = "schema_version: 1\nrules:\n  - id: ssh.keys\n    app: { id: ssh, name: SSH, kind: dev_tool }\n    category: credentials\n    title: SSH\n    targets: [ { path: \"{HOME}\\\\.ssh\" } ]\n";
    fs::write(tmp.path().join("ssh.yaml"), text).unwrap();

    let (set, issues) = RuleSet::load(false, Some(tmp.path()));
    assert_eq!(issues, vec![]);
    let rule = set.get("ssh.keys").unwrap();
    assert_eq!(rule.rule.sensitivity, Sensitivity::High);
}

#[test]
fn validate_file_delegates_to_diagnostic() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("missing.yaml");
    let [d] = RuleSet::validate_file(&missing).try_into().unwrap();
    assert_eq!(d.file, missing);
    assert_eq!(d.severity, DiagnosticSeverity::Error);

    let valid = tmp.path().join("valid.yaml");
    fs::write(&valid, file_yaml(&[rule_yaml("app.one", 100, "A")])).unwrap();
    assert_eq!(RuleSet::validate_file(&valid), vec![]);
}

#[test]
fn parallel_compile_keeps_file_order() {
    // Every 7th rule has an invalid confidence, every 5th repeats an id.
    let rules: Vec<String> = (0..60)
        .map(|i| {
            let id = if i % 5 == 4 {
                format!("app.r{}", i - 1)
            } else {
                format!("app.r{i}")
            };
            let yaml = rule_yaml(&id, 100, "T");
            if i % 7 == 0 {
                format!("{yaml}    confidence: 2.0\n")
            } else {
                yaml
            }
        })
        .collect();
    let errors = match crate::compile::compile_yaml(&file_yaml(&rules)) {
        Ok(_) => panic!("file must be rejected"),
        Err(errors) => errors,
    };
    // Per rule in file order: the duplicate id first, then the rule's own error.
    let expected: Vec<String> = (0..60)
        .flat_map(|i| {
            let mut e = Vec::new();
            if i % 5 == 4 {
                e.push(format!("duplicate rule id app.r{}", i - 1));
            }
            if i % 7 == 0 {
                let id = if i % 5 == 4 { i - 1 } else { i };
                e.push(format!(
                    "invalid rule app.r{id}: confidence 2 is outside 0..=1"
                ));
            }
            e
        })
        .collect();
    let errors: Vec<String> = errors.iter().map(ToString::to_string).collect();
    assert_eq!(errors.len(), expected.len());
    for (error, expected) in errors.iter().zip(&expected) {
        assert!(error.contains(expected.as_str()), "{error} vs {expected}");
    }

    let ok_rules: Vec<String> = (0..60)
        .map(|i| rule_yaml(&format!("app.r{i:02}"), 100, "T"))
        .collect();
    let compiled = crate::compile::compile_yaml(&file_yaml(&ok_rules)).unwrap();
    let ids: Vec<&str> = compiled.rules.iter().map(CompiledRule::id).collect();
    let expected: Vec<String> = (0..60).map(|i| format!("app.r{i:02}")).collect();
    assert_eq!(ids, expected);
}

#[test]
fn condition_regexes_are_kept_for_the_evaluator() {
    let text = r"schema_version: 1
rules:
  - id: app.one
    app: { id: app, name: App, kind: application }
    category: app_config
    title: T
    conditions:
      - any_of:
          - installed: { display_name_regex: '(?i)^app\b' }
          - file_contains: { path: '{APPDATA}\App\a.txt', pattern: 'x+' }
    targets: [ { path: '{APPDATA}\App' } ]
";
    let set = builtin_set(&[("a.yaml", text)]);
    let patterns = |list: Vec<&String>| list.into_iter().cloned().collect::<Vec<_>>();
    let text_patterns = patterns(set.regexes().text.iter().map(|(p, _)| p).collect());
    let bytes_patterns = patterns(set.regexes().bytes.iter().map(|(p, _)| p).collect());
    assert_eq!(text_patterns, [r"(?i)^app\b"]);
    assert_eq!(bytes_patterns, ["x+"]);
}
