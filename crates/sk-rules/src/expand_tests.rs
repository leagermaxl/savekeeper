use std::path::{Path, PathBuf};

use sk_core::fs::ReparseKind;
use sk_core::model::{AppKind, Category, RegHive, Sensitivity};
use sk_core::registry::MemRegistry;
use sk_scan::MemFs;

use super::*;
use crate::compile::compile_yaml;
use crate::conditions::{hive_name, ISSUE_REGISTRY_ACCESS_DENIED};

pub(super) fn root() -> PathBuf {
    PathBuf::from(if cfg!(windows) { r"C:\fake" } else { "/fake" })
}

pub(super) fn env() -> Environment {
    Environment::fake(&root())
}

/// Absolute path of `rel` (`/`-separated) under `{APPDATA}` of the fake env.
pub(super) fn appdata(rel: &str) -> PathBuf {
    let base = root()
        .join("Users")
        .join("user")
        .join("AppData")
        .join("Roaming");
    rel.split('/').fold(base, |p, c| p.join(c))
}

pub(super) fn s(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Compiles `rules` (YAML list items, indented by 2) into rules.
pub(super) fn rules(yaml: &str) -> Vec<CompiledRule> {
    let text = format!("schema_version: 1\nrules:\n{yaml}");
    match compile_yaml(&text) {
        Ok(file) => file.rules,
        Err(errors) => panic!("invalid test rules: {errors:?}\n{text}"),
    }
}

pub(super) fn rule(yaml: &str) -> CompiledRule {
    let mut all = rules(yaml);
    assert_eq!(all.len(), 1);
    all.remove(0)
}

pub(super) const MATCHED: ConditionOutcome = ConditionOutcome {
    matched: true,
    app_running: false,
};

pub(super) struct Setup {
    pub(super) env: Environment,
    pub(super) fs: MemFs,
    pub(super) registry: MemRegistry,
    pub(super) resolve: ResolveContext,
}

impl Setup {
    pub(super) fn new() -> Self {
        Self {
            env: env(),
            fs: MemFs::new(),
            registry: MemRegistry::new(),
            resolve: ResolveContext::default(),
        }
    }

    pub(super) fn file(&mut self, rel: &str) -> &mut Self {
        self.fs.add_file(&s(&appdata(rel)), 10, "-1d", None);
        self
    }

    pub(super) fn dir(&mut self, rel: &str) -> &mut Self {
        self.fs.add_dir(&s(&appdata(rel)));
        self
    }

    pub(super) fn expander(&self) -> TargetExpander<'_> {
        TargetExpander::new(&self.env, &self.fs, &self.registry, &self.resolve)
    }

    pub(super) fn expand(&self, rule: &CompiledRule) -> RuleOutput {
        self.expander().expand(rule, MATCHED)
    }
}

pub(super) fn templates(out: &RuleOutput) -> Vec<String> {
    out.findings
        .iter()
        .map(|f| match &f.target {
            Target::FileSet { root, .. } => root.as_str().to_owned(),
            Target::File { path, .. } => format!("file:{}", path.as_str()),
            Target::Registry { hive, key, .. } => format!("{}:{key}", hive_name(*hive)),
            Target::SystemExport { exporter_id, .. } => exporter_id.clone(),
        })
        .collect()
}

const VSCODE: &str = r#"
  - id: vscode.user-settings
    app: { id: vscode, name: Visual Studio Code, kind: dev_tool, winget: Microsoft.VisualStudioCode }
    category: app_config
    title_key: rules.vscode.user_settings
    notes_key: rules.vscode.notes
    confidence: 0.95
    tags: [ide]
    conditions:
      - process_running: "Code.exe"
    targets:
      - path: "{APPDATA}\\Code\\User"
        include: ["settings.json", "snippets\\**"]
        exclude: ["workspaceStorage/**"]
        label_key: rules.vscode.label.user
      - path: "{APPDATA}\\Code\\argv.json"
        tags: [launch]
        optional: true
      - registry: { hive: hkcu, key: "\\Software\\Classes\\vscode\\" }
        optional: true
        label_key: rules.vscode.label.protocol
    claims:
      - "{APPDATA}\\Code\\Cache"
      - "{APPDATA}\\Code\\User"
"#;

#[test]
fn findings_carry_rule_fields() {
    let mut setup = Setup::new();
    setup.file("Code/User/settings.json").file("Code/argv.json");
    setup
        .registry
        .add_key(RegHive::Hkcu, "Software\\Classes\\vscode");
    let outcome = ConditionOutcome {
        matched: true,
        app_running: true,
    };
    let out = setup.expander().expand(&rule(VSCODE), outcome);
    assert_eq!(
        templates(&out),
        [
            r"{APPDATA}\Code\User",
            r"file:{APPDATA}\Code\argv.json",
            r"HKCU:Software\Classes\vscode",
        ]
    );

    let user = &out.findings[0];
    assert_eq!(
        user.target,
        Target::FileSet {
            root: PathTemplate::parse(r"{APPDATA}\Code\User").unwrap_or_else(|e| panic!("{e}")),
            resolved: appdata("Code/User"),
            include: vec!["settings.json".to_owned(), "snippets/**".to_owned()],
            exclude: vec!["workspaceStorage/**".to_owned()],
        }
    );
    assert_eq!(user.id, FindingId::for_target(&user.target));
    assert_eq!(user.category, Category::AppConfig);
    assert_eq!(user.sensitivity, Sensitivity::None);
    assert_eq!(
        user.title,
        format!("rules.vscode.user_settings{TITLE_LABEL_SEPARATOR}rules.vscode.label.user")
    );
    assert_eq!(user.tags, ["ide", APP_RUNNING_TAG]);
    assert_eq!(user.notes_key.as_deref(), Some("rules.vscode.notes"));
    assert!(!user.default_selected && !user.requires_elevation);
    assert!(user.stats.is_none() && user.score.is_none() && user.children.is_empty());
    let app = user.app.as_ref().unwrap_or_else(|| panic!("no app"));
    assert_eq!(app.id, "vscode");
    assert_eq!(app.kind, AppKind::DevTool);
    assert_eq!(app.source_ids["winget"], "Microsoft.VisualStudioCode");
    assert_eq!(app.installed, None);
    assert_eq!(app.process_names, ["code.exe"]);
    assert_eq!(user.evidence.len(), 1);
    let evidence = &user.evidence[0];
    assert_eq!(
        evidence.source,
        EvidenceSource::Rule {
            rule_id: "vscode.user-settings".to_owned()
        }
    );
    assert_eq!(evidence.message_key, "evidence.rule_match");
    assert!((evidence.confidence - 0.95).abs() < f32::EPSILON);

    // No label: no suffix even with several targets.
    assert_eq!(out.findings[1].title, "rules.vscode.user_settings");
    assert_eq!(out.findings[1].tags, ["ide", "launch", APP_RUNNING_TAG]);
    assert_eq!(
        out.findings[2].target,
        Target::Registry {
            hive: RegHive::Hkcu,
            key: r"Software\Classes\vscode".to_owned(),
            recursive: true,
        }
    );

    // Roots of the findings, then the claims, without repeats.
    assert_eq!(
        out.claimed_paths,
        [
            appdata("Code/User"),
            appdata("Code/argv.json"),
            appdata("Code/Cache")
        ]
    );
}

#[test]
fn optional_targets_may_be_missing() {
    let mut setup = Setup::new();
    setup.file("Code/User/settings.json");
    let out = setup.expand(&rule(VSCODE));
    assert_eq!(templates(&out), [r"{APPDATA}\Code\User"]);
    assert_eq!(out.findings[0].tags, ["ide"]);
}

#[test]
fn optional_targets_alone_do_not_fire_the_rule() {
    let mut setup = Setup::new();
    setup.file("Code/argv.json").dir("Code/Cache");
    setup
        .registry
        .add_key(RegHive::Hkcu, "Software\\Classes\\vscode");
    let out = setup.expand(&rule(VSCODE));
    assert!(out.findings.is_empty() && out.issues.is_empty());
    // Claims follow the conditions, not the targets; a plain claim is not
    // checked for existence.
    assert_eq!(
        out.claimed_paths,
        [appdata("Code/Cache"), appdata("Code/User")]
    );
}

const ALL_OPTIONAL: &str = r#"
  - id: app.data
    app: { id: app, name: App, kind: application }
    category: app_data
    title_key: rules.app.data
    targets:
      - path: "{APPDATA}\\App\\a.ini"
        optional: true
      - path: "{APPDATA}\\App\\b.ini"
        optional: true
      - registry: { hive: hkcu, key: "Software\\App" }
        optional: true
"#;

#[test]
fn all_optional_targets_fire_on_any() {
    let setup = Setup::new();
    assert_eq!(setup.expand(&rule(ALL_OPTIONAL)), RuleOutput::default());

    let mut setup = Setup::new();
    setup.file("App/b.ini");
    let out = setup.expand(&rule(ALL_OPTIONAL));
    assert_eq!(templates(&out), [r"file:{APPDATA}\App\b.ini"]);

    let mut setup = Setup::new();
    setup.registry.add_key(RegHive::Hkcu, "Software\\App");
    let out = setup.expand(&rule(ALL_OPTIONAL));
    assert_eq!(templates(&out), [r"HKCU:Software\App"]);
}

#[test]
fn claims_only_rule_fires_by_conditions() {
    let setup = Setup::new();
    let claims_only = rule(
        r#"
  - id: spotify.none
    app: { id: spotify, name: Spotify, kind: application }
    category: cache
    title_key: rules.spotify.none
    claims: [ "{APPDATA}\\Spotify" ]
"#,
    );
    let out = setup.expand(&claims_only);
    assert!(out.findings.is_empty());
    assert_eq!(out.claimed_paths, [appdata("Spotify")]);
    let out = setup
        .expander()
        .expand(&claims_only, ConditionOutcome::default());
    assert_eq!(out, RuleOutput::default());
}

#[test]
fn unmatched_conditions_give_nothing() {
    let mut setup = Setup::new();
    setup.file("Code/User/settings.json");
    let out = setup
        .expander()
        .expand(&rule(VSCODE), ConditionOutcome::default());
    assert_eq!(out, RuleOutput::default());
}

#[test]
fn single_target_title_has_no_suffix() {
    let mut setup = Setup::new();
    setup.file("App/config.ini");
    let out = setup.expand(&rule(
        r#"
  - id: app.config
    app: { id: app, name: App, kind: application }
    category: app_config
    title: "App settings"
    targets:
      - path: "{APPDATA}\\App\\config.ini"
        label_key: rules.app.label
"#,
    ));
    assert_eq!(out.findings[0].title, "App settings");
    assert!(matches!(out.findings[0].target, Target::File { .. }));
}

#[test]
fn reparse_roots_are_findings() {
    let mut setup = Setup::new();
    setup
        .fs
        .add_reparse(&s(&appdata("Junction")), ReparseKind::Junction);
    setup.dir("DirLink").file("FileLink");
    setup
        .fs
        .add_reparse(&s(&appdata("DirLink")), ReparseKind::Symlink)
        .add_reparse(&s(&appdata("FileLink")), ReparseKind::Symlink);
    let out = setup.expand(&rule(
        r#"
  - id: app.linked
    app: { id: app, name: App, kind: application }
    category: app_data
    title_key: rules.app.linked
    targets:
      - path: "{APPDATA}\\Junction"
      - path: "{APPDATA}\\DirLink"
      - path: "{APPDATA}\\FileLink"
"#,
    ));
    // A folder link is a FileSet, a file link a File (by
    // FILE_ATTRIBUTE_DIRECTORY); Measure tags them `reparse_root`.
    assert_eq!(
        templates(&out),
        [
            r"{APPDATA}\Junction",
            r"{APPDATA}\DirLink",
            r"file:{APPDATA}\FileLink",
        ]
    );
    assert_eq!(
        out.claimed_paths,
        [appdata("Junction"), appdata("DirLink"), appdata("FileLink")]
    );
}

const PUTTY: &str = r#"
  - id: putty.sessions
    app: { id: putty, name: PuTTY, kind: dev_tool }
    category: dev_environment
    title_key: rules.putty.sessions
    targets:
      - registry: { hive: hkcu, key: "Software\\SimonTatham\\PuTTY", recursive: false }
"#;

#[test]
fn registry_target_needs_a_readable_key() {
    let setup = Setup::new();
    assert_eq!(setup.expand(&rule(PUTTY)), RuleOutput::default());

    let mut setup = Setup::new();
    setup
        .registry
        .add_key(RegHive::Hkcu, "Software\\SimonTatham\\PuTTY\\Sessions");
    let out = setup.expand(&rule(PUTTY));
    assert_eq!(
        out.findings[0].target,
        Target::Registry {
            hive: RegHive::Hkcu,
            key: r"Software\SimonTatham\PuTTY".to_owned(),
            recursive: false,
        }
    );
    assert!(out.claimed_paths.is_empty());
}

#[test]
fn denied_registry_key_is_reported_once() {
    let mut setup = Setup::new();
    setup
        .registry
        .add_denied_key(RegHive::Hkcu, "Software\\SimonTatham\\PuTTY");
    let expander = setup.expander();
    let first = expander.expand(&rule(PUTTY), MATCHED);
    assert!(first.findings.is_empty());
    assert_eq!(first.issues.len(), 1);
    let issue = &first.issues[0];
    assert_eq!(issue.severity, IssueSeverity::Info);
    assert_eq!(issue.message_key, ISSUE_REGISTRY_ACCESS_DENIED);
    assert_eq!(issue.message_args["hive"], "HKCU");
    assert_eq!(issue.message_args["key"], r"Software\SimonTatham\PuTTY");
    assert_eq!(issue.message_args["rule_id"], "putty.sessions");
    assert!(expander.expand(&rule(PUTTY), MATCHED).issues.is_empty());
}

#[test]
fn denied_key_report_is_shared_with_conditions() {
    let mut setup = Setup::new();
    setup
        .registry
        .add_denied_key(RegHive::Hkcu, "Software\\SimonTatham\\PuTTY");
    let with_condition = rules(
        r#"
  - id: putty.probe
    app: { id: putty, name: PuTTY, kind: dev_tool }
    category: dev_environment
    title_key: rules.putty.probe
    conditions: [ { registry_exists: { hive: hkcu, key: "software\\simontatham\\putty" } } ]
    claims: [ "{APPDATA}\\PuTTY" ]
"#,
    )
    .remove(0);

    // Condition first: the target does not report the key again.
    let evaluator = ConditionEvaluator::new(&setup.env, &setup.fs, &setup.registry, &setup.resolve);
    let expander = TargetExpander::from_evaluator(&evaluator);
    assert!(!evaluator.evaluate(&with_condition.rule).matched);
    assert_eq!(evaluator.take_issues().len(), 1);
    assert!(expander.expand(&rule(PUTTY), MATCHED).issues.is_empty());

    // Target first: the condition does not report it again.
    let evaluator = ConditionEvaluator::new(&setup.env, &setup.fs, &setup.registry, &setup.resolve);
    let expander = TargetExpander::from_evaluator(&evaluator);
    assert_eq!(expander.expand(&rule(PUTTY), MATCHED).issues.len(), 1);
    assert!(!evaluator.evaluate(&with_condition.rule).matched);
    assert!(evaluator.take_issues().is_empty());
}

#[test]
fn from_json_target_without_config_has_no_roots() {
    let mut setup = Setup::new();
    setup.file("App/config.ini");
    let out = setup.expand(&rule(
        r#"
  - id: app.data
    app: { id: app, name: App, kind: application }
    category: user_files
    title_key: rules.app.data
    targets:
      - from_json: { file: "{APPDATA}\\App\\app.json", select: "/vaults/*/path" }
      - path: "{APPDATA}\\App\\config.ini"
"#,
    ));
    assert_eq!(templates(&out), [r"file:{APPDATA}\App\config.ini"]);
    // A missing config is not a problem (§4.2.1 step 1) and is not claimed.
    assert!(out.issues.is_empty());
    assert_eq!(out.claimed_paths, [appdata("App/config.ini")]);

    // A required `from_json` target without roots: the optional one alone
    // does not fire the rule.
    let out = setup.expand(&rule(
        r#"
  - id: app.data
    app: { id: app, name: App, kind: application }
    category: user_files
    title_key: rules.app.data
    targets:
      - from_json: { file: "{APPDATA}\\App\\app.json", select: "/vaults/*/path" }
      - path: "{APPDATA}\\App\\config.ini"
        optional: true
"#,
    ));
    assert!(out.findings.is_empty());
}

#[test]
fn disabled_rule_gives_nothing() {
    let mut setup = Setup::new();
    setup.file("Code/User/settings.json");
    let out = setup.expand(&rule("  - { id: vscode.user-settings, disabled: true }\n"));
    assert_eq!(out, RuleOutput::default());
}
