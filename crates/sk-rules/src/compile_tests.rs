use super::*;

/// The example from SPEC-04 §4.2 without comments.
const SPEC_EXAMPLE: &str = r#"
schema_version: 1
rules:
  - id: vscode.user-settings
    app: { id: vscode, name: Visual Studio Code, kind: dev_tool, winget: Microsoft.VisualStudioCode }
    category: app_config
    title_key: rules.vscode.user_settings
    confidence: 0.95
    tags: [ide]
    conditions:
      - exists: "{APPDATA}\\Code\\User"
    targets:
      - path: "{APPDATA}\\Code\\User"
        include: ["settings.json", "keybindings.json", "snippets/**", "profiles/**", "tasks.json"]
        exclude: ["workspaceStorage/**", "History/**", "globalStorage/**/*.vsix"]
      - registry: { hive: hkcu, key: "Software\\Classes\\vscode" }
        optional: true
    claims:
      - "{APPDATA}\\Code\\Cache"
"#;

/// A file with one valid rule whose body is replaced by `body` (indented by 4).
fn one_rule(body: &str) -> String {
    format!("schema_version: 1\nrules:\n  - id: app.rule\n{body}")
}

/// A valid rule body plus `extra` lines.
fn valid_rule(extra: &str) -> String {
    one_rule(&format!(
        "    app: {{ id: app, name: App, kind: application }}\n    category: app_config\n    title_key: rules.app\n{extra}"
    ))
}

fn compile_ok(text: &str) -> CompiledFile {
    match compile_yaml(text) {
        Ok(file) => file,
        Err(errors) => panic!("expected success, got {errors:?}"),
    }
}

/// Reasons of all errors; panics if the file compiles.
fn reasons(text: &str) -> Vec<String> {
    match compile_yaml(text) {
        Ok(file) => panic!("expected errors, compiled {file:?}"),
        Err(errors) => errors.iter().map(ToString::to_string).collect(),
    }
}

fn assert_error(text: &str, needle: &str) {
    let reasons = reasons(text);
    assert!(
        reasons.iter().any(|r| r.contains(needle)),
        "no error containing {needle:?} in {reasons:?}"
    );
}

#[test]
fn spec_example_compiles() {
    let file = compile_ok(SPEC_EXAMPLE);
    assert!(file.warnings.is_empty());
    let [rule] = file.rules.as_slice() else {
        panic!("one rule expected");
    };
    assert_eq!(rule.id(), "vscode.user-settings");
    assert_eq!(rule.targets.len(), 2);

    let fs = &rule.targets[0];
    assert_eq!(
        fs.root,
        TargetRoot::Path {
            template: PathTemplate::parse("{APPDATA}\\Code\\User").unwrap(),
            glob_root: false,
        }
    );
    assert_eq!(fs.category, Category::AppConfig);
    assert_eq!(fs.sensitivity, Sensitivity::None);
    assert_eq!(fs.tags, vec!["ide".to_owned()]);
    assert!(!fs.optional);
    assert!(fs.include.is_match("settings.json"));
    assert!(fs.include.is_match("Snippets/rust.json")); // case-insensitive
    assert!(!fs.include.is_match("other.json"));
    assert!(fs.exclude.is_match("globalStorage/a/b/ext.vsix"));
    assert!(!fs.exclude.is_match("globalStorage/state.json"));

    let reg = &rule.targets[1];
    let TargetRoot::Registry(registry) = &reg.root else {
        panic!("registry target expected");
    };
    assert_eq!(registry.hive, RegHive::Hkcu);
    assert!(registry.recursive);
    assert!(reg.optional);
}

#[test]
fn globs_are_normalized_to_slash() {
    let file = compile_ok(&valid_rule(
        "    targets:\n      - path: \"{HOME}\\\\x\"\n        include: [\"a\\\\*.txt\"]\n        exclude: [\"b\\\\**\"]\n",
    ));
    let target = &file.rules[0].targets[0];
    assert_eq!(target.include_globs, vec!["a/*.txt".to_owned()]);
    assert_eq!(target.exclude_globs, vec!["b/**".to_owned()]);
    assert!(target.include.is_match("a/1.txt"));
    assert!(!target.include.is_match("a/sub/1.txt")); // `*` does not cross `/`
}

#[test]
fn empty_include_compiles_to_empty_set() {
    let file = compile_ok(&valid_rule("    targets: [ { path: \"{HOME}\\\\x\" } ]\n"));
    let target = &file.rules[0].targets[0];
    assert!(target.include_globs.is_empty());
    assert!(target.include.is_empty());
}

#[test]
fn target_overrides_and_tags() {
    let file = compile_ok(&valid_rule(
        "    tags: [a, b]\n    targets:\n      - path: \"{HOME}\\\\x\"\n        category: app_data\n        sensitivity: low\n        tags: [b, c]\n        label_key: rules.app.x\n",
    ));
    let target = &file.rules[0].targets[0];
    assert_eq!(target.category, Category::AppData);
    assert_eq!(target.sensitivity, Sensitivity::Low);
    assert_eq!(target.tags, vec!["a", "b", "c"]);
    assert_eq!(target.label_key.as_deref(), Some("rules.app.x"));
}

#[test]
fn schema_version_must_be_1() {
    let text = "schema_version: 2\nrules: []\n";
    assert_error(text, "unsupported schema_version 2");
}

#[test]
fn disabled_rule_needs_only_id() {
    let file =
        compile_ok("schema_version: 1\nrules: [ { id: discord.settings, disabled: true } ]\n");
    let rule = &file.rules[0];
    assert!(rule.rule.disabled);
    assert!(rule.targets.is_empty());
}

#[test]
fn disabled_rule_id_is_still_checked() {
    assert_error(
        "schema_version: 1\nrules: [ { id: Discord, disabled: true } ]\n",
        "must match",
    );
}

#[test]
fn enabled_rule_requires_fields() {
    let reasons = reasons(&one_rule("    priority: 1\n"));
    for needle in [
        "`app`",
        "`category`",
        "`title_key` or `title`",
        "`targets` or `claims`",
    ] {
        assert!(
            reasons.iter().any(|r| r.contains(needle)),
            "{needle} missing in {reasons:?}"
        );
    }
}

#[test]
fn title_or_claims_alone_are_enough() {
    compile_ok(&one_rule(
        "    app: { id: app, name: App, kind: application }\n    category: cache\n    title: App\n    claims: [ \"{APPDATA}\\\\App\" ]\n",
    ));
}

#[test]
fn rule_id_syntax() {
    assert!(is_valid_rule_id("vscode.user-settings_2"));
    assert!(!is_valid_rule_id(""));
    assert!(!is_valid_rule_id("VSCode"));
    assert!(!is_valid_rule_id("a b"));
    assert!(!is_valid_rule_id("a/b"));
    let text =
        valid_rule("    targets: [ { path: \"{HOME}\\\\x\" } ]\n").replace("app.rule", "app.Rule");
    assert_error(&text, "must match");
}

#[test]
fn duplicate_ids_in_one_file() {
    let rule = "  - { id: a.b, app: { id: a, name: A, kind: game }, category: game_save, title: A, claims: [\"{HOME}\\\\a\"] }\n";
    let text = format!("schema_version: 1\nrules:\n{rule}{rule}");
    let Err(errors) = compile_yaml(&text) else {
        panic!("expected an error");
    };
    assert!(matches!(errors.as_slice(), [RuleError::DuplicateId(id)] if id == "a.b"));
}

#[test]
fn unknown_token_is_an_error() {
    let Err(errors) = compile_yaml(&valid_rule("    targets: [ { path: \"{NOPE}\\\\x\" } ]\n"))
    else {
        panic!("expected an error");
    };
    assert!(matches!(errors.as_slice(), [RuleError::Yaml(_)]));
}

#[test]
fn invalid_glob_is_an_error() {
    assert_error(
        &valid_rule("    targets: [ { path: \"{HOME}\\\\x\", include: [\"a[\"] } ]\n"),
        "targets[0].include: invalid glob `a[`",
    );
    assert_error(
        &valid_rule("    targets: [ { path: \"{HOME}\\\\x\", exclude: [\"{a,b\"] } ]\n"),
        "targets[0].exclude",
    );
}

#[test]
fn confidence_range() {
    let target = "    targets: [ { path: \"{HOME}\\\\x\" } ]\n";
    compile_ok(&valid_rule(&format!("    confidence: 0\n{target}")));
    compile_ok(&valid_rule(&format!("    confidence: 1\n{target}")));
    assert_error(
        &valid_rule(&format!("    confidence: 1.5\n{target}")),
        "outside 0..=1",
    );
    assert_error(
        &valid_rule(&format!("    confidence: -0.1\n{target}")),
        "outside 0..=1",
    );
}

#[test]
fn credentials_raise_sensitivity_with_warning() {
    let text = one_rule(
        "    app: { id: ssh, name: SSH, kind: dev_tool }\n    category: credentials\n    title: SSH\n    sensitivity: low\n    targets: [ { path: \"{HOME}\\\\.ssh\" } ]\n",
    );
    let file = compile_ok(&text);
    let rule = &file.rules[0];
    assert_eq!(rule.rule.sensitivity, Sensitivity::High);
    assert_eq!(rule.targets[0].sensitivity, Sensitivity::High);
    assert_eq!(file.warnings.len(), 1);
    assert_eq!(file.warnings[0].rule_id, "app.rule");
}

#[test]
fn credentials_target_override_raises_sensitivity() {
    let file = compile_ok(&valid_rule(
        "    targets:\n      - path: \"{HOME}\\\\x\"\n      - path: \"{HOME}\\\\keys\"\n        category: credentials\n",
    ));
    let targets = &file.rules[0].targets;
    assert_eq!(targets[0].sensitivity, Sensitivity::None);
    assert_eq!(targets[1].sensitivity, Sensitivity::High);
    assert_eq!(file.warnings.len(), 1);
    assert!(file.warnings[0].message.starts_with("targets[1]"));
}

#[test]
fn credentials_with_high_has_no_warning() {
    let file = compile_ok(&one_rule(
        "    app: { id: ssh, name: SSH, kind: dev_tool }\n    category: credentials\n    title: SSH\n    sensitivity: high\n    targets: [ { path: \"{HOME}\\\\.ssh\" } ]\n",
    ));
    assert!(file.warnings.is_empty());
}

#[test]
fn hklm_requires_system_or_app_config() {
    let hklm = "    targets: [ { registry: { hive: hklm, key: \"Software\\\\X\" } } ]\n";
    compile_ok(&valid_rule(hklm)); // app_config
    compile_ok(&valid_rule(hklm).replace("app_config", "system_settings"));
    assert_error(
        &valid_rule(hklm).replace("app_config", "app_data"),
        "`hive: hklm`",
    );
    // A target override counts.
    assert_error(
        &valid_rule("    targets: [ { registry: { hive: hklm, key: X }, category: game_save } ]\n"),
        "`hive: hklm`",
    );
    // HKCU is fine anywhere.
    compile_ok(
        &valid_rule("    targets: [ { registry: { hive: hkcu, key: X } } ]\n")
            .replace("app_config", "app_data"),
    );
}

#[test]
fn star_in_path_requires_glob_root() {
    let path = "{APPDATA}\\\\JetBrains\\\\*";
    assert_error(
        &valid_rule(&format!("    targets: [ {{ path: \"{path}\" }} ]\n")),
        "requires `glob_root: true`",
    );
    let file = compile_ok(&valid_rule(&format!(
        "    targets: [ {{ path: \"{path}\", glob_root: true }} ]\n"
    )));
    assert!(matches!(
        file.rules[0].targets[0].root,
        TargetRoot::Path {
            glob_root: true,
            ..
        }
    ));
}

#[test]
fn glob_root_depth_is_limited() {
    compile_ok(&valid_rule(
        "    targets: [ { path: \"{APPDATA}\\\\Adobe\\\\Adobe Photoshop *\\\\Adobe Photoshop * Settings\", glob_root: true } ]\n",
    ));
    assert_error(
        &valid_rule("    targets: [ { path: \"{APPDATA}\\\\*\\\\*\\\\*\", glob_root: true } ]\n"),
        "at most 2",
    );
}

#[test]
fn claims_allow_two_wildcard_segments() {
    let claims = |c: &str| valid_rule(&format!("    claims: [ \"{c}\" ]\n"));
    compile_ok(&claims("{APPDATA}\\\\Chrome\\\\*\\\\Cache"));
    compile_ok(&claims("{DRIVE:*}\\\\*\\\\x*\\\\Cache"));
    assert_error(
        &claims("{APPDATA}\\\\*\\\\*\\\\*"),
        "claims[0]: 3 `*` segments",
    );
}

#[test]
fn all_drives_token_is_not_a_wildcard() {
    compile_ok(&valid_rule(
        "    targets: [ { path: \"{DRIVE:*}\\\\RetroArch\" } ]\n",
    ));
}

#[test]
fn target_needs_exactly_one_root() {
    let both = "    targets: [ { path: \"{HOME}\\\\x\", registry: { hive: hkcu, key: X } } ]\n";
    assert_error(&valid_rule(both), "exactly one of");
    assert_error(
        &valid_rule("    targets: [ { optional: true } ]\n"),
        "exactly one of",
    );
    let path_and_json = "    targets: [ { path: \"{HOME}\\\\x\", from_json: { file: \"{HOME}\\\\a.json\", select: /a } } ]\n";
    assert_error(&valid_rule(path_and_json), "exactly one of");
}

#[test]
fn from_json_select_is_checked() {
    let target = |select: &str| {
        valid_rule(&format!(
            "    targets: [ {{ from_json: {{ file: \"{{APPDATA}}\\\\o.json\", select: \"{select}\" }} }} ]\n"
        ))
    };
    let file = compile_ok(&target("/vaults/*/path"));
    assert!(matches!(
        file.rules[0].targets[0].root,
        TargetRoot::FromJson(_)
    ));
    compile_ok(&target("/*/*/*"));
    assert_error(&target("vaults/*"), "JSON Pointer");
    assert_error(&target("/*/*/*/*"), "at most 3");
}

#[test]
fn all_errors_are_reported() {
    let text = format!(
        "{}  - id: Bad\n    disabled: true\n",
        one_rule("    confidence: 2\n")
    );
    let reasons = reasons(&text);
    assert!(reasons.len() >= 5, "{reasons:?}");
    assert!(reasons.iter().any(|r| r.contains("invalid rule Bad")));
    assert!(reasons.iter().any(|r| r.contains("outside 0..=1")));
}

/// A valid rule with the given YAML flow list of conditions.
fn with_conditions(conditions: &str) -> String {
    valid_rule(&format!(
        "    conditions: {conditions}\n    targets: [ {{ path: \"{{HOME}}\\\\x\" }} ]\n"
    ))
}

#[test]
fn installed_regex_is_compiled() {
    let file = compile_ok(&with_conditions(
        r#"[ { installed: { display_name_regex: "(?i)^obs studio" } } ]"#,
    ));
    assert!(file.warnings.is_empty());
    assert_error(
        &with_conditions(r#"[ { installed: { display_name_regex: "(obs" } } ]"#),
        "conditions[0].installed.display_name_regex: invalid regex `(obs`",
    );
    // Nested in `any_of`, also when `winget` is set.
    assert_error(
        &with_conditions(
            r#"[ { exists: "{HOME}\\x" }, { any_of: [ { exists: "{HOME}\\y" }, { installed: { display_name_regex: "[", winget: A.B } } ] } ]"#,
        ),
        "conditions[1].any_of[1].installed.display_name_regex",
    );
}

#[test]
fn installed_without_criteria_is_an_error() {
    assert_error(
        &with_conditions("[ { installed: {} } ]"),
        "conditions[0].installed: `display_name_regex` or `winget` is required",
    );
    assert_error(
        &with_conditions("[ { any_of: [ { any_of: [ { installed: {} } ] } ] } ]"),
        "conditions[0].any_of[0].any_of[0].installed",
    );
}

#[test]
fn installed_with_only_winget_warns() {
    let file = compile_ok(&with_conditions(
        r#"[ { any_of: [ { installed: { winget: OBSProject.OBSStudio } } ] } ]"#,
    ));
    assert_eq!(file.rules.len(), 1);
    let [warning] = file.warnings.as_slice() else {
        panic!("one warning expected: {:?}", file.warnings);
    };
    assert_eq!(warning.rule_id, "app.rule");
    assert!(
        warning
            .message
            .starts_with("conditions[0].any_of[0].installed: `winget`"),
        "{}",
        warning.message
    );
    // With a regex the rule can match: no warning.
    let file = compile_ok(&with_conditions(
        r#"[ { installed: { winget: OBSProject.OBSStudio, display_name_regex: "(?i)obs" } } ]"#,
    ));
    assert!(file.warnings.is_empty());
}

#[test]
fn file_contains_pattern_is_compiled() {
    compile_ok(&with_conditions(
        r#"[ { file_contains: { path: "{HOME}\\a.json", pattern: "\"telemetry\"" } } ]"#,
    ));
    assert_error(
        &with_conditions(r#"[ { file_contains: { path: "{HOME}\\a.json", pattern: "a{2" } } ]"#),
        "conditions[0].file_contains.pattern: invalid regex `a{2`",
    );
    assert_error(
        &with_conditions(
            r#"[ { any_of: [ { file_contains: { path: "{HOME}\\a.json", pattern: "(" } } ] } ]"#,
        ),
        "conditions[0].any_of[0].file_contains.pattern",
    );
}

#[test]
fn file_contains_max_bytes_is_at_most_1_mib() {
    let condition = |max: u64| {
        with_conditions(&format!(
            r#"[ {{ file_contains: {{ path: "{{HOME}}\\a.json", pattern: x, max_bytes: {max} }} }} ]"#
        ))
    };
    compile_ok(&condition(MAX_FILE_CONTAINS_BYTES));
    assert_error(
        &condition(MAX_FILE_CONTAINS_BYTES + 1),
        "conditions[0].file_contains.max_bytes: 1048577 exceeds 1048576",
    );
}

#[test]
fn condition_errors_reject_the_file_with_every_error() {
    let reasons = reasons(&with_conditions(
        r#"[ { installed: {} }, { file_contains: { path: "{HOME}\\a", pattern: "(", max_bytes: 2000000 } } ]"#,
    ));
    assert_eq!(reasons.len(), 3, "{reasons:?}");
}

#[test]
fn disabled_rule_conditions_are_not_checked() {
    compile_ok(
        "schema_version: 1\nrules: [ { id: app.rule, disabled: true, conditions: [ { installed: {} } ] } ]\n",
    );
}

#[test]
fn compile_takes_parsed_file() {
    let file = RuleFile::from_yaml(SPEC_EXAMPLE).unwrap();
    assert_eq!(compile(file).unwrap().rules.len(), 1);
}
