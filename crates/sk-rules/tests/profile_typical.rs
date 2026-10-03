//! Findings and claimed paths of typical rules on `fixtures/fs/profile-typical`
//! (SPEC-04 §6, T-04-05): VS Code, Chrome with two profiles, Firefox, SSH,
//! OBS Studio and Telegram, through conditions and target expansion.
//!
//! The rules mirror the starting base (SPEC-04 §4.7) and live here until the
//! built-in YAML files exist (T-04-07, T-04-08).

use std::path::PathBuf;

use serde_json::{json, Value};
use sk_core::model::RegHive;
use sk_core::template::ResolveContext;
use sk_rules::compile::compile_yaml;
use sk_rules::{ConditionEvaluator, MemRegistry, RuleOutput, TargetExpander};

const RULES: &str = r#"
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
      - path: "{HOME}\\.vscode\\extensions\\extensions.json"
        optional: true
        label_key: rules.vscode.extensions_list
      - registry: { hive: hkcu, key: "Software\\Classes\\vscode", recursive: true }
        optional: true
        label_key: rules.vscode.protocol
    claims:
      - "{APPDATA}\\Code\\Cache"
      - "{APPDATA}\\Code\\CachedData"
      - "{APPDATA}\\Code\\User\\workspaceStorage"
    notes_key: rules.vscode.notes

  - id: chrome.profiles
    app: { id: chrome, name: Google Chrome, kind: application }
    category: app_data
    sensitivity: high
    title_key: rules.chrome.profiles
    targets:
      - path: "{LOCALAPPDATA}\\Google\\Chrome\\User Data"
        include: ["*/Bookmarks", "*/Preferences", "*/Secure Preferences", "*/Extensions/**",
                  "*/Local Extension Settings/**", "*/Login Data*", "*/Web Data", "*/History", "Local State"]
    claims:
      - "{LOCALAPPDATA}\\Google\\Chrome\\User Data\\*\\Cache"
      - "{LOCALAPPDATA}\\Google\\Chrome\\User Data\\*\\Code Cache"
      - "{LOCALAPPDATA}\\Google\\Chrome\\User Data\\*\\GPUCache"
      - "{LOCALAPPDATA}\\Google\\Chrome\\User Data\\*\\Service Worker\\CacheStorage"
    notes_key: rules.browsers.notes

  - id: firefox.profiles
    app: { id: firefox, name: Mozilla Firefox, kind: application }
    category: app_data
    sensitivity: high
    title_key: rules.firefox.profiles
    targets:
      - path: "{APPDATA}\\Mozilla\\Firefox"
        include: ["profiles.ini", "Profiles/*/{places.sqlite,key4.db,logins.json,cert9.db,prefs.js,user.js,extensions/**,chrome/**,containers.json,handlers.json,sessionstore*}"]
    claims:
      - "{LOCALAPPDATA}\\Mozilla\\Firefox\\Profiles"
    notes_key: rules.browsers.notes

  - id: ssh.keys
    app: { id: openssh, name: OpenSSH, kind: dev_tool }
    category: credentials
    sensitivity: high
    title_key: rules.ssh.keys
    targets:
      - path: "{HOME}\\.ssh"

  - id: obs.config
    app: { id: obs-studio, name: OBS Studio, kind: application }
    category: app_config
    title_key: rules.obs.config
    conditions:
      - process_running: obs64.exe
    targets:
      - path: "{APPDATA}\\obs-studio"
        include: ["basic/**", "global.ini", "plugin_config/**"]
        exclude: ["logs/**", "crashes/**", "updates/**"]

  - id: telegram.tdata
    app: { id: telegram, name: Telegram Desktop, kind: application }
    category: app_data
    sensitivity: high
    title_key: rules.telegram.tdata
    targets:
      - path: "{APPDATA}\\Telegram Desktop\\tdata"
        exclude: ["user_data/**", "emoji/**", "dumps/**"]
    notes_key: rules.telegram.notes

  - id: vlc.config
    app: { id: vlc, name: VLC media player, kind: application }
    category: app_config
    title_key: rules.vlc.config
    conditions:
      - exists: "{APPDATA}\\vlc"
    targets:
      - path: "{APPDATA}\\vlc"
        exclude: ["art/**"]

  - id: jetbrains.config
    app: { id: jetbrains, name: JetBrains IDEs, kind: dev_tool }
    category: dev_environment
    title_key: rules.jetbrains.config
    targets:
      - path: "{APPDATA}\\JetBrains\\*"
        glob_root: true
        include: ["options/**", "keymaps/**", "codestyles/**", "templates/**", "colors/**", "*.key"]
    claims:
      - "{LOCALAPPDATA}\\JetBrains"
"#;

fn root() -> PathBuf {
    PathBuf::from(if cfg!(windows) { r"C:\fake" } else { "/fake" })
}

/// Replaces the fake root at the start of every string with `[root]` and
/// makes the rest `/`-separated, so the snapshot is the same on every OS.
fn redact(value: &mut Value, root: &str) {
    match value {
        Value::String(s) => {
            if let Some(rest) = s.strip_prefix(root) {
                *s = format!("[root]{}", rest.replace('\\', "/"));
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|v| redact(v, root)),
        Value::Object(map) => map.values_mut().for_each(|v| redact(v, root)),
        _ => {}
    }
}

#[test]
fn profile_typical_snapshot() {
    let (fs, mut env) = sk_testkit::mem_fixture("profile-typical", &root());
    env.running_processes = vec!["explorer.exe".to_owned(), "obs64.exe".to_owned()];
    let mut registry = MemRegistry::new();
    registry.add_key(
        RegHive::Hkcu,
        "Software\\Classes\\vscode\\shell\\open\\command",
    );
    let resolve = ResolveContext::default();

    let mut rules = compile_yaml(RULES)
        .unwrap_or_else(|e| panic!("invalid rules: {e:?}"))
        .rules;
    // The order of a RuleSet: priority desc, then id (all priorities equal).
    rules.sort_by(|a, b| a.id().cmp(b.id()));

    let evaluator = ConditionEvaluator::new(&env, &fs, &registry, &resolve);
    let expander = TargetExpander::from_evaluator(&evaluator);
    let mut total = RuleOutput::default();
    for rule in &rules {
        let out = expander.expand(rule, evaluator.evaluate(&rule.rule));
        total.findings.extend(out.findings);
        total.claimed_paths.extend(out.claimed_paths);
        total.issues.extend(out.issues);
    }
    total.issues.extend(evaluator.take_issues());

    // Every typical application gives findings; VLC and JetBrains are absent.
    let rule_ids: Vec<String> = total
        .findings
        .iter()
        .filter_map(|f| match &f.evidence[0].source {
            sk_core::model::EvidenceSource::Rule { rule_id } => Some(rule_id.clone()),
            _ => None,
        })
        .collect();
    for id in [
        "chrome.profiles",
        "firefox.profiles",
        "obs.config",
        "ssh.keys",
        "telegram.tdata",
        "vscode.user-settings",
    ] {
        assert!(rule_ids.iter().any(|r| r == id), "no finding from {id}");
    }
    assert!(!rule_ids
        .iter()
        .any(|r| r == "vlc.config" || r == "jetbrains.config"));
    // JetBrains has no conditions: its claim is added although no IDE is
    // installed; VLC's condition fails, so it claims nothing.
    let claimed = |rel: &str| {
        total
            .claimed_paths
            .iter()
            .any(|p| p.ends_with(rel.replace('/', std::path::MAIN_SEPARATOR_STR)))
    };
    assert!(claimed("AppData/Local/JetBrains"));
    assert!(!claimed("AppData/Roaming/vlc"));

    let mut snapshot = json!({
        "findings": total.findings,
        "claimed_paths": total.claimed_paths,
        "issues": total.issues,
    });
    redact(&mut snapshot, &root().to_string_lossy());
    insta::assert_json_snapshot!("profile_typical", snapshot);
}
