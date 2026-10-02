use super::*;

/// The example from SPEC-04 §4.2 (comments translated).
const SPEC_EXAMPLE: &str = r#"
schema_version: 1
rules:
  - id: vscode.user-settings                # unique, [a-z0-9._-]+, prefix = app.id
    app:
      id: vscode
      name: Visual Studio Code
      kind: dev_tool                        # game | application | system | dev_tool
      winget: Microsoft.VisualStudioCode    # -> AppRef.source_ids.winget (optional)
    category: app_config                    # SPEC-02 §2.3, snake_case
    title_key: rules.vscode.user_settings   # i18n key; or title: "VS Code settings"
    message_key: evidence.rule_match        # evidence.rule_match by default
    confidence: 0.95                        # 0..1, 0.9 by default
    sensitivity: none                       # none | low | high
    tags: [ide]
    priority: 100                           # higher wins on conflicts (100 by default)
    conditions:                             # all must hold (AND); any_of is OR
      - exists: "{APPDATA}\\Code\\User"
    targets:
      - path: "{APPDATA}\\Code\\User"       # FileSet for a directory, File for a file
        include: ["settings.json", "keybindings.json", "snippets/**", "profiles/**", "tasks.json"]
        exclude: ["workspaceStorage/**", "History/**", "globalStorage/**/*.vsix"]
      - registry: { hive: hkcu, key: "Software\\Classes\\vscode", recursive: true }
        optional: true                      # absence does not block findings from other targets
    claims:                                 # explained but not saved
      - "{APPDATA}\\Code\\Cache"
      - "{APPDATA}\\Code\\CachedData"
      - "{APPDATA}\\Code\\User\\workspaceStorage"
    notes_key: rules.vscode.notes           # UI hint
"#;

/// The `from_json` example from SPEC-04 §4.2.1, wrapped into a file.
const FROM_JSON_EXAMPLE: &str = r#"
schema_version: 1
rules:
  - id: obsidian.vaults
    app: { id: obsidian, name: Obsidian, kind: application }
    category: user_files
    title_key: rules.obsidian.vault
    conditions: [ { exists: "{APPDATA}\\obsidian\\obsidian.json" } ]
    targets:
      - from_json:
          file: "{APPDATA}\\obsidian\\obsidian.json"
          format: json
          select: "/vaults/*/path"
          max_matches: 50
        include: []
        exclude: [".trash/**"]
        tags: [notes]
"#;

fn tpl(s: &str) -> PathTemplate {
    PathTemplate::parse(s).unwrap()
}

fn parse(text: &str) -> Result<RuleFile, RuleError> {
    RuleFile::from_yaml(text)
}

/// Wraps a single rule body (indented by 4) into a file.
fn one_rule(body: &str) -> String {
    format!("schema_version: 1\nrules:\n  - id: x.y\n{body}")
}

fn err_text(text: &str) -> String {
    match parse(text) {
        Ok(file) => panic!("expected an error, parsed {file:?}"),
        Err(err) => err.to_string(),
    }
}

#[test]
fn spec_example_parses() {
    let file = parse(SPEC_EXAMPLE).unwrap();
    assert_eq!(file.schema_version, SCHEMA_VERSION);
    assert_eq!(file.rules.len(), 1);
    let rule = &file.rules[0];

    assert_eq!(rule.id, "vscode.user-settings");
    assert!(!rule.disabled);
    assert_eq!(
        rule.app,
        Some(RuleApp {
            id: "vscode".into(),
            name: "Visual Studio Code".into(),
            kind: AppKind::DevTool,
            winget: Some("Microsoft.VisualStudioCode".into()),
        })
    );
    assert_eq!(rule.category, Some(Category::AppConfig));
    assert_eq!(
        rule.title_key.as_deref(),
        Some("rules.vscode.user_settings")
    );
    assert_eq!(rule.title, None);
    assert_eq!(rule.message_key(), "evidence.rule_match");
    assert_eq!(rule.confidence, 0.95);
    assert_eq!(rule.sensitivity, Sensitivity::None);
    assert_eq!(rule.tags, ["ide"]);
    assert_eq!(rule.priority, 100);
    assert_eq!(
        rule.conditions,
        [Condition::Exists(tpl(r"{APPDATA}\Code\User"))]
    );

    assert_eq!(rule.targets.len(), 2);
    let fs = &rule.targets[0];
    assert_eq!(fs.path, Some(tpl(r"{APPDATA}\Code\User")));
    assert_eq!(fs.registry, None);
    assert_eq!(fs.from_json, None);
    assert_eq!(
        fs.include,
        [
            "settings.json",
            "keybindings.json",
            "snippets/**",
            "profiles/**",
            "tasks.json"
        ]
    );
    assert_eq!(
        fs.exclude,
        [
            "workspaceStorage/**",
            "History/**",
            "globalStorage/**/*.vsix"
        ]
    );
    assert!(!fs.optional);
    assert!(!fs.glob_root);

    let reg = &rule.targets[1];
    assert_eq!(reg.path, None);
    assert_eq!(
        reg.registry,
        Some(RegistryTarget {
            hive: RegHive::Hkcu,
            key: r"Software\Classes\vscode".into(),
            recursive: true,
        })
    );
    assert!(reg.optional);

    assert_eq!(
        rule.claims,
        [
            tpl(r"{APPDATA}\Code\Cache"),
            tpl(r"{APPDATA}\Code\CachedData"),
            tpl(r"{APPDATA}\Code\User\workspaceStorage"),
        ]
    );
    assert_eq!(rule.notes_key.as_deref(), Some("rules.vscode.notes"));
}

#[test]
fn from_json_example_parses() {
    let file = parse(FROM_JSON_EXAMPLE).unwrap();
    let rule = &file.rules[0];
    assert_eq!(rule.category, Some(Category::UserFiles));
    let target = &rule.targets[0];
    assert_eq!(target.path, None);
    assert_eq!(
        target.from_json,
        Some(FromJson {
            file: tpl(r"{APPDATA}\obsidian\obsidian.json"),
            format: JsonFormat::Json,
            select: "/vaults/*/path".into(),
            max_matches: 50,
        })
    );
    assert!(target.include.is_empty());
    assert_eq!(target.exclude, [".trash/**"]);
    assert_eq!(target.tags, ["notes"]);
}

#[test]
fn defaults_apply() {
    let text = one_rule(
        r#"    app: { id: x, name: X, kind: game }
    category: game_save
    targets:
      - path: "{SAVED_GAMES}\\X"
      - registry: { hive: hklm, key: "Software\\X" }
      - from_json: { file: "{APPDATA}\\x.json", select: "/a" }
"#,
    );
    let file = parse(&text).unwrap();
    let rule = &file.rules[0];
    assert!(!rule.disabled);
    assert_eq!(rule.title_key, None);
    assert_eq!(rule.message_key, None);
    assert_eq!(rule.message_key(), DEFAULT_MESSAGE_KEY);
    assert_eq!(rule.confidence, DEFAULT_CONFIDENCE);
    assert_eq!(rule.sensitivity, Sensitivity::None);
    assert_eq!(rule.priority, DEFAULT_PRIORITY);
    assert!(rule.tags.is_empty());
    assert!(rule.conditions.is_empty());
    assert!(rule.claims.is_empty());
    assert_eq!(rule.notes_key, None);
    assert_eq!(rule.app.as_ref().unwrap().winget, None);

    let path = &rule.targets[0];
    assert!(path.include.is_empty() && path.exclude.is_empty() && path.tags.is_empty());
    assert_eq!((path.category, path.sensitivity), (None, None));
    assert!(!path.optional && !path.glob_root);
    assert_eq!(path.label_key, None);

    let reg = rule.targets[1].registry.as_ref().unwrap();
    assert_eq!(reg.hive, RegHive::Hklm);
    assert!(reg.recursive);

    let json = rule.targets[2].from_json.as_ref().unwrap();
    assert_eq!(json.format, JsonFormat::Json);
    assert_eq!(json.max_matches, DEFAULT_MAX_MATCHES);
}

#[test]
fn all_target_fields_parse() {
    let text = one_rule(
        r#"    title: "X settings"
    message_key: evidence.custom
    sensitivity: high
    priority: -5
    targets:
      - path: "{APPDATA}\\X\\Profiles\\*"
        glob_root: true
        category: credentials
        sensitivity: low
        label_key: rules.x.profile
        optional: true
      - from_json: { file: "{APPDATA}\\x.json", format: jsonc, select: "/a/*", max_matches: 7 }
"#,
    );
    let file = parse(&text).unwrap();
    let rule = &file.rules[0];
    assert_eq!(rule.title.as_deref(), Some("X settings"));
    assert_eq!(rule.message_key(), "evidence.custom");
    assert_eq!(rule.sensitivity, Sensitivity::High);
    assert_eq!(rule.priority, -5);
    let t = &rule.targets[0];
    assert_eq!(t.path, Some(tpl(r"{APPDATA}\X\Profiles\*")));
    assert!(t.glob_root && t.optional);
    assert_eq!(t.category, Some(Category::Credentials));
    assert_eq!(t.sensitivity, Some(Sensitivity::Low));
    assert_eq!(t.label_key.as_deref(), Some("rules.x.profile"));
    let json = rule.targets[1].from_json.as_ref().unwrap();
    assert_eq!(json.format, JsonFormat::Jsonc);
    assert_eq!(json.max_matches, 7);
}

#[test]
fn all_conditions_parse() {
    let text = one_rule(
        r#"    conditions:
      - exists: "{LOCALAPPDATA}\\Obsidian"
      - not_exists: "{LOCALAPPDATA}\\Obsidian\\portable"
      - installed: { display_name_regex: "(?i)^obs studio" }
      - installed: { winget: "OBSProject.OBSStudio" }
      - registry_exists: { hive: hkcu, key: "Software\\SimonTatham\\PuTTY" }
      - file_contains: { path: "{APPDATA}\\X\\a.json", pattern: "\"telemetry\"", max_bytes: 65536 }
      - file_contains: { path: "{APPDATA}\\X\\b.json", pattern: "y" }
      - os: { min_build: 22000 }
      - any_of: [ { exists: "{APPDATA}\\A" }, { installed: { winget: "A.B" } } ]
      - process_running: "obs64.exe"
"#,
    );
    let file = parse(&text).unwrap();
    let expected = vec![
        Condition::Exists(tpl(r"{LOCALAPPDATA}\Obsidian")),
        Condition::NotExists(tpl(r"{LOCALAPPDATA}\Obsidian\portable")),
        Condition::Installed(InstalledCondition {
            display_name_regex: Some("(?i)^obs studio".into()),
            winget: None,
        }),
        Condition::Installed(InstalledCondition {
            display_name_regex: None,
            winget: Some("OBSProject.OBSStudio".into()),
        }),
        Condition::RegistryExists(RegistryKey {
            hive: RegHive::Hkcu,
            key: r"Software\SimonTatham\PuTTY".into(),
        }),
        Condition::FileContains(FileContainsCondition {
            path: tpl(r"{APPDATA}\X\a.json"),
            pattern: "\"telemetry\"".into(),
            max_bytes: 65536,
        }),
        Condition::FileContains(FileContainsCondition {
            path: tpl(r"{APPDATA}\X\b.json"),
            pattern: "y".into(),
            max_bytes: DEFAULT_FILE_CONTAINS_MAX_BYTES,
        }),
        Condition::Os(OsCondition { min_build: 22000 }),
        Condition::AnyOf(vec![
            Condition::Exists(tpl(r"{APPDATA}\A")),
            Condition::Installed(InstalledCondition {
                display_name_regex: None,
                winget: Some("A.B".into()),
            }),
        ]),
        Condition::ProcessRunning("obs64.exe".into()),
    ];
    assert_eq!(file.rules[0].conditions, expected);
}

#[test]
fn disable_only_rule_parses() {
    // SPEC-04 §4.6: all other fields are optional with `disabled`.
    let file =
        parse("schema_version: 1\nrules: [{ id: discord.settings, disabled: true }]").unwrap();
    let rule = &file.rules[0];
    assert_eq!(rule.id, "discord.settings");
    assert!(rule.disabled);
    assert_eq!(rule.app, None);
    assert_eq!(rule.category, None);
    assert!(rule.targets.is_empty());
}

#[test]
fn several_rules_in_one_file() {
    let text = "schema_version: 1\nrules:\n  - { id: a.one, disabled: true }\n  - { id: b.two, disabled: true }\n";
    let ids: Vec<_> = parse(text)
        .unwrap()
        .rules
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(ids, ["a.one", "b.two"]);
}

#[test]
fn unknown_fields_rejected_everywhere() {
    let cases = [
        // top level
        "schema_version: 1\nrules: []\nextra: 1\n".to_string(),
        // rule (typo)
        one_rule("    categry: app_config\n"),
        // app
        one_rule("    app: { id: x, name: X, kind: game, vendor: Y }\n"),
        // target
        one_rule("    targets: [ { path: \"{HOME}\\\\x\", inclde: [a] } ]\n"),
        // registry target
        one_rule("    targets: [ { registry: { hive: hkcu, key: K, deep: true } } ]\n"),
        // from_json
        one_rule("    targets: [ { from_json: { file: \"{HOME}\\\\x.json\", select: /a, limit: 3 } } ]\n"),
        // condition payloads
        one_rule("    conditions: [ { installed: { name: X } } ]\n"),
        one_rule("    conditions: [ { registry_exists: { hive: hkcu, key: K, recursive: true } } ]\n"),
        one_rule("    conditions: [ { os: { min_build: 1, max_build: 2 } } ]\n"),
        one_rule("    conditions: [ { file_contains: { path: \"{HOME}\\\\x\", pattern: p, regex: true } } ]\n"),
    ];
    for text in &cases {
        let msg = err_text(text);
        assert!(msg.contains("unknown field"), "{text}\n-> {msg}");
    }
}

#[test]
fn unknown_condition_rejected() {
    let msg = err_text(&one_rule(
        "    conditions: [ { exist: \"{HOME}\\\\x\" } ]\n",
    ));
    assert!(msg.contains("unknown variant"), "{msg}");
}

#[test]
fn bad_enum_values_rejected() {
    for body in [
        "    category: settings\n",
        "    sensitivity: medium\n",
        "    app: { id: x, name: X, kind: tool }\n",
        "    targets: [ { registry: { hive: hkcr, key: K } } ]\n",
        "    targets: [ { from_json: { file: \"{HOME}\\\\x\", select: /a, format: yaml } } ]\n",
    ] {
        assert!(parse(&one_rule(body)).is_err(), "{body}");
    }
}

#[test]
fn invalid_path_template_rejected() {
    for body in [
        "    targets: [ { path: \"{APPDTA}\\\\x\" } ]\n",
        "    claims: [ \"{HOME}\\\\..\\\\x\" ]\n",
        "    conditions: [ { exists: \"{HOME\" } ]\n",
        "    targets: [ { from_json: { file: \"x\\\\{APPDATA}\", select: /a } } ]\n",
    ] {
        assert!(parse(&one_rule(body)).is_err(), "{body}");
    }
}

#[test]
fn missing_required_fields_rejected() {
    assert!(parse("rules: []\n").is_err());
    assert!(parse("schema_version: 1\n").is_err());
    assert!(parse("schema_version: 1\nrules: [ { disabled: true } ]\n").is_err());
    assert!(parse(&one_rule("    app: { id: x, kind: game }\n")).is_err());
    assert!(parse(&one_rule(
        "    targets: [ { from_json: { select: /a } } ]\n"
    ))
    .is_err());
    assert!(parse(&one_rule(
        "    conditions: [ { file_contains: { path: \"{HOME}\" } } ]\n"
    ))
    .is_err());
}

#[test]
fn yaml_error_has_line() {
    let text = one_rule("    category: app_config\n    titel: X\n");
    let Err(RuleError::Yaml(err)) = parse(&text) else {
        panic!("expected a YAML error");
    };
    let location = err.location().expect("location");
    assert_eq!(location.line(), 5); // the `titel` line, 1-based
}
