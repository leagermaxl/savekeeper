use super::*;

fn file() -> PathBuf {
    PathBuf::from("rules.d/test.yaml")
}

fn check(text: &str) -> Vec<RuleDiagnostic> {
    validate_str(&file(), text)
}

const VALID: &str = r#"schema_version: 1
rules:
  - id: app.one
    app: { id: app, name: App, kind: application }
    category: app_config
    title_key: rules.app.one
    targets: [ { path: "{APPDATA}\\App" } ]
"#;

#[test]
fn valid_file_has_no_diagnostics() {
    assert_eq!(check(VALID), vec![]);
}

#[test]
fn yaml_error_has_line_and_rule() {
    // Line 6: unknown field inside the second rule (starts at line 4).
    let text = "schema_version: 1\nrules:\n  - { id: app.one, disabled: true }\n  - id: app.two\n    category: app_config\n    titel: X\n";
    let [d] = check(text).try_into().unwrap();
    assert_eq!(d.file, file());
    assert_eq!(d.line, Some(6));
    assert_eq!(d.rule_id.as_deref(), Some("app.two"));
    assert!(d.message.contains("titel"), "{}", d.message);
}

#[test]
fn template_error_has_line() {
    let text = VALID.replace("{APPDATA}", "{NOPE}");
    let [d] = check(&text).try_into().unwrap();
    assert_eq!(d.line, Some(7));
    assert_eq!(d.rule_id.as_deref(), Some("app.one"));
    assert!(d.message.contains("NOPE"), "{}", d.message);
}

#[test]
fn broken_yaml_has_line_without_rule() {
    let text = "schema_version: 1\nrules:\n  - id: [unclosed\n";
    let [d] = check(text).try_into().unwrap();
    assert!(d.line.is_some());
    assert_eq!(d.rule_id, None);
}

#[test]
fn validation_errors_point_at_the_rule() {
    let text = format!(
        "{VALID}  - id: app.two\n    title: Two\n    claims: [ \"{{HOME}}\\\\x\" ]\n  - id: app.one\n    disabled: true\n"
    );
    let diagnostics = check(&text);
    // app.two (line 8) lacks app and category; app.one repeats (line 11).
    let two: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.rule_id.as_deref() == Some("app.two"))
        .collect();
    assert_eq!(two.len(), 2, "{diagnostics:?}");
    assert!(two.iter().all(|d| d.line == Some(8)));
    assert!(two.iter().any(|d| d.message == "`app` is required"));

    let dup = diagnostics
        .iter()
        .find(|d| d.message.contains("duplicate"))
        .unwrap();
    assert_eq!(dup.rule_id.as_deref(), Some("app.one"));
    assert_eq!(dup.line, Some(11));
}

#[test]
fn schema_version_error_points_at_its_line() {
    let text = VALID.replace("schema_version: 1", "schema_version: 3");
    let [d] = check(&text).try_into().unwrap();
    assert_eq!(d.line, Some(1));
    assert_eq!(d.rule_id, None);
    assert_eq!(d.severity, DiagnosticSeverity::Error);
    assert!(d.message.contains("schema_version 3"));
}

#[test]
fn warnings_are_reported_after_errors() {
    let text = format!(
        "{}  - id: ssh.keys\n    app: {{ id: ssh, name: SSH, kind: dev_tool }}\n    category: credentials\n    title: SSH\n    targets: [ {{ path: \"{{HOME}}\\\\.ssh\" }} ]\n",
        VALID.replace("rules.app.one", "\"\"")
    );
    let diagnostics = check(&text);
    assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
    assert_eq!(diagnostics[0].rule_id.as_deref(), Some("app.one"));
    assert_eq!(diagnostics[0].line, Some(3));
    assert_eq!(diagnostics[1].rule_id.as_deref(), Some("ssh.keys"));
    assert_eq!(diagnostics[1].line, Some(8));
    assert_eq!(diagnostics[0].severity, DiagnosticSeverity::Error);
    assert_eq!(diagnostics[1].severity, DiagnosticSeverity::Warning);
    assert!(
        diagnostics[1].message.contains("high"),
        "{}",
        diagnostics[1].message
    );
}

#[test]
fn rule_with_empty_id_points_at_its_own_line() {
    // The second rule (line 8) has `id: ""` and no `app`.
    let text = format!("{VALID}  - id: \"\"\n    category: app_config\n    title: Empty\n");
    let diagnostics = check(&text);
    assert!(diagnostics.len() >= 2, "{diagnostics:?}");
    assert!(
        diagnostics.iter().all(|d| d.line == Some(8)
            && d.rule_id.is_none()
            && d.severity == DiagnosticSeverity::Error),
        "{diagnostics:?}"
    );
    assert!(diagnostics.iter().any(|d| d.message == "`app` is required"));
}

#[test]
fn unreadable_file_is_a_diagnostic() {
    let path = Path::new("definitely/missing/rules.yaml");
    let [d] = validate_file(path).try_into().unwrap();
    assert_eq!(d.file, path);
    assert_eq!(d.line, None);
    assert!(d.message.starts_with("cannot read file"));
}
