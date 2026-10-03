//! Tests of `from_json` targets (SPEC-04 §4.2.1, §5, §6).

use std::path::{Path, PathBuf};

use sk_core::env::{Environment, KnownFolder};
use sk_core::model::{Category, Finding, IssueSeverity, ScanIssue, Target};

use super::super::tests::{appdata, rule, s, templates, Setup, MATCHED};
use super::*;

/// Absolute path of `rel` (`/`-separated) under `{HOME}` of `env`.
pub(super) fn home(env: &Environment, rel: &str) -> PathBuf {
    let base = env
        .known_folder(KnownFolder::Home)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| panic!("no home"));
    rel.split('/').fold(base, |p, c| p.join(c))
}

/// `path` as a JSON string literal.
pub(super) fn q(path: &Path) -> String {
    serde_json::to_string(&s(path)).unwrap_or_else(|e| panic!("{e}"))
}

pub(super) const CONFIG: &str = "obsidian/obsidian.json";

pub(super) fn add_config(setup: &mut Setup, text: &str) {
    let bytes = text.as_bytes();
    let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    setup
        .fs
        .add_file(&s(&appdata(CONFIG)), len, "-1d", Some(bytes));
}

pub(super) fn obsidian(extra: &str) -> crate::compile::CompiledRule {
    rule(&format!(
        r#"
  - id: obsidian.vaults
    app: {{ id: obsidian, name: Obsidian, kind: application }}
    category: user_files
    title_key: rules.obsidian.vault
    targets:
      - from_json:
          file: "{{APPDATA}}\\obsidian\\obsidian.json"
          select: "/vaults/*/path"
{extra}        exclude: [".trash/**"]
        tags: [notes]
"#
    ))
}

pub(super) fn reasons(issues: &[ScanIssue]) -> Vec<(&str, &str)> {
    issues
        .iter()
        .map(|i| {
            assert_eq!(i.message_key, ISSUE_FROM_JSON_SKIPPED);
            assert_eq!(i.severity, IssueSeverity::Info);
            assert_eq!(i.path.as_deref(), Some(i.message_args["path"].as_str()));
            (
                i.message_args["path"].as_str(),
                i.message_args["reason"].as_str(),
            )
        })
        .collect()
}

fn only_issue(issues: &[ScanIssue]) -> &ScanIssue {
    assert_eq!(issues.len(), 1, "{issues:?}");
    &issues[0]
}

#[test]
fn obsidian_vaults_become_findings() {
    let mut setup = Setup::new();
    let notes = home(&setup.env, "Documents/Notes");
    let work = home(&setup.env, "Documents/Work Notes");
    setup.fs.add_dir(&s(&notes)).add_dir(&s(&work));
    let missing = if cfg!(windows) {
        r"E:\Vault"
    } else {
        "/mnt/e/Vault"
    };
    let uri = format!(
        "file://{}{}",
        if cfg!(windows) { "/" } else { "" },
        s(&work).replace('\\', "/").replace(' ', "%20")
    );
    let text = format!(
        r#"{{"vaults":{{
  "f1":{{"path":{},"ts":1,"open":true}},
  "f2":{{"path":{}}},
  "f3":{{"path":{}}}
}},"frame":"native"}}"#,
        q(&notes),
        q(Path::new(missing)),
        q(Path::new(&uri)),
    );
    add_config(&mut setup, &text);

    let out = setup.expand(&obsidian(""));
    assert_eq!(
        templates(&out),
        [r"{DOCUMENTS}\Notes", r"{DOCUMENTS}\Work Notes"]
    );
    let first: &Finding = &out.findings[0];
    let Target::FileSet {
        resolved,
        include,
        exclude,
        ..
    } = &first.target
    else {
        panic!("expected a FileSet");
    };
    assert_eq!(resolved, &notes);
    assert!(include.is_empty());
    assert_eq!(exclude, &[".trash/**"]);
    assert_eq!(first.category, Category::UserFiles);
    assert_eq!(first.title, "rules.obsidian.vault");
    assert_eq!(first.tags, ["notes"]);
    let evidence = &first.evidence[0];
    assert_eq!(evidence.message_key, FROM_JSON_MESSAGE_KEY);
    assert_eq!(
        evidence.message_args,
        BTreeMap::from([
            (
                "file".to_owned(),
                r"{APPDATA}\obsidian\obsidian.json".to_owned()
            ),
            ("select".to_owned(), "/vaults/*/path".to_owned()),
            ("name".to_owned(), "Notes".to_owned()),
        ])
    );
    assert_eq!(
        out.findings[1].evidence[0].message_args["name"],
        "Work Notes"
    );

    // Roots, then the config itself.
    assert_eq!(out.claimed_paths, [notes, work, appdata(CONFIG)]);
    // The vault on a missing drive is skipped with its template.
    let skipped = reasons(&out.issues);
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].1, "missing");
    if cfg!(windows) {
        assert_eq!(skipped[0].0, r"{DRIVE:E}\Vault");
    }
}

#[test]
fn unity_hub_projects_keep_document_order() {
    let mut setup = Setup::new();
    let tool = home(&setup.env, "Projects/Tool");
    let game = home(&setup.env, "Projects/Game");
    setup.fs.add_dir(&s(&tool)).add_dir(&s(&game));
    let text = format!(
        r#"{{"schema_version":"v1","data":{{
  {tool}:{{"path":{tool},"title":"Tool","version":"2022.3.1f1","isFavorite":false}},
  {game}:{{"path":{game},"title":"Game","version":"6000.0.1f1","isFavorite":true}}
}}}}"#,
        tool = q(&tool),
        game = q(&game),
    );
    setup.fs.add_file(
        &s(&appdata("UnityHub/projects-v1.json")),
        10,
        "-1d",
        Some(text.as_bytes()),
    );
    let out = setup.expand(&rule(
        r#"
  - id: unityhub.projects
    app: { id: unityhub, name: Unity Hub, kind: dev_tool }
    category: user_files
    title_key: rules.unityhub.project
    targets:
      - from_json: { file: "{APPDATA}\\UnityHub\\projects-v1.json", select: "/data/*/path" }
        exclude: ["Library/**", "Temp/**", "Logs/**", "obj/**"]
        tags: [project]
"#,
    ));
    assert_eq!(
        templates(&out),
        [r"{HOME}\Projects\Tool", r"{HOME}\Projects\Game"]
    );
    assert!(out.issues.is_empty());
}

#[test]
fn jsonc_with_comments_arrays_and_files() {
    let mut setup = Setup::new();
    let vault = home(&setup.env, "Vault");
    let single = home(&setup.env, "notes.md");
    setup.fs.add_dir(&s(&vault));
    setup.fs.add_file(&s(&single), 5, "-1d", None);
    let text = format!(
        "{{\n  // recent vaults\n  \"vaults\": [\n    {{ \"path\": {} }}, /* the file */\n    {{ \"path\": {} }},\n  ],\n}}\n",
        q(&vault),
        q(&single)
    );
    add_config(&mut setup, &text);

    let jsonc = obsidian("          format: jsonc\n");
    let out = setup.expand(&jsonc);
    assert_eq!(templates(&out), [r"{HOME}\Vault", r"file:{HOME}\notes.md"]);
    assert!(out.issues.is_empty());

    // The same text is not strict JSON.
    let out = setup.expand(&obsidian(""));
    assert!(out.findings.is_empty());
    let parse = only_issue(&out.issues);
    assert_eq!(parse.message_key, ISSUE_FROM_JSON_PARSE);
    assert_eq!(parse.severity, IssueSeverity::Warning);
    assert_eq!(parse.message_args["line"], "2");
    assert_eq!(parse.message_args["rule_id"], "obsidian.vaults");
    assert_eq!(
        parse.path.as_deref(),
        Some(r"{APPDATA}\obsidian\obsidian.json")
    );
    // The config is still claimed.
    assert_eq!(out.claimed_paths, [appdata(CONFIG)]);
}

#[test]
fn escaped_pointer_segments() {
    let mut setup = Setup::new();
    let vault = home(&setup.env, "Vault");
    setup.fs.add_dir(&s(&vault));
    add_config(
        &mut setup,
        &format!(r#"{{"a/b": {{"x~y": [{}]}}}}"#, q(&vault)),
    );
    let escaped = rule(
        r#"
  - id: app.vaults
    app: { id: app, name: App, kind: application }
    category: user_files
    title_key: rules.app.vault
    targets:
      - from_json: { file: "{APPDATA}\\obsidian\\obsidian.json", select: "/a~1b/x~0y/*" }
"#,
    );
    assert_eq!(templates(&setup.expand(&escaped)), [r"{HOME}\Vault"]);
}

#[test]
fn config_over_one_mib_is_not_read() {
    let mut setup = Setup::new();
    let size = u64::try_from(FROM_JSON_MAX_BYTES).unwrap_or(u64::MAX) + 1;
    setup.fs.add_file(&s(&appdata(CONFIG)), size, "-1d", None);
    let out = setup.expand(&obsidian(""));
    assert!(out.findings.is_empty());
    let large = only_issue(&out.issues);
    assert_eq!(large.message_key, ISSUE_FROM_JSON_TOO_LARGE);
    assert_eq!(large.severity, IssueSeverity::Warning);
    assert_eq!(large.message_args["limit"], FROM_JSON_MAX_BYTES.to_string());
    assert_eq!(out.claimed_paths, [appdata(CONFIG)]);

    // Exactly 1 MiB is read.
    let mut setup = Setup::new();
    let vault = home(&setup.env, "Vault");
    setup.fs.add_dir(&s(&vault));
    let mut text = format!(r#"{{"vaults":{{"a":{{"path":{}}}}}}}"#, q(&vault));
    text.push_str(&" ".repeat(FROM_JSON_MAX_BYTES - text.len()));
    add_config(&mut setup, &text);
    assert_eq!(templates(&setup.expand(&obsidian(""))), [r"{HOME}\Vault"]);
}

#[test]
fn empty_selection_and_missing_config() {
    let mut setup = Setup::new();
    let out = setup.expand(&obsidian(""));
    assert_eq!(out, RuleOutput::default());

    add_config(&mut setup, r#"{"vaults":{"a":{"path":7}}}"#);
    let out = setup.expand(&obsidian(""));
    assert!(out.findings.is_empty());
    let empty = only_issue(&out.issues);
    assert_eq!(empty.message_key, ISSUE_FROM_JSON_EMPTY);
    assert_eq!(empty.severity, IssueSeverity::Info);
    assert_eq!(empty.message_args["select"], "/vaults/*/path");
    assert_eq!(out.claimed_paths, [appdata(CONFIG)]);
}

#[test]
fn max_matches_keeps_the_first_values() {
    let mut setup = Setup::new();
    let names = ["C", "A", "B"];
    let mut members = Vec::new();
    for name in names {
        let vault = home(&setup.env, name);
        setup.fs.add_dir(&s(&vault));
        members.push(format!(r#""{name}":{{"path":{}}}"#, q(&vault)));
    }
    add_config(
        &mut setup,
        &format!(r#"{{"vaults":{{{}}}}}"#, members.join(",")),
    );
    let out = setup.expand(&obsidian("          max_matches: 2\n"));
    assert_eq!(templates(&out), [r"{HOME}\C", r"{HOME}\A"]);
    let truncated = only_issue(&out.issues);
    assert_eq!(truncated.message_key, ISSUE_FROM_JSON_TRUNCATED);
    assert_eq!(truncated.severity, IssueSeverity::Warning);
    assert_eq!(truncated.message_args["matches"], "3");
    assert_eq!(truncated.message_args["limit"], "2");
}

#[test]
fn unreadable_config_is_reported() {
    for cloud in [false, true] {
        let mut setup = Setup::new();
        add_config(&mut setup, r#"{"vaults":{}}"#);
        let path = s(&appdata(CONFIG));
        if cloud {
            setup.fs.set_cloud_only(&path);
        } else {
            setup.fs.set_locked(&path);
        }
        let out = setup.expand(&obsidian(""));
        assert!(out.findings.is_empty());
        let unreadable = only_issue(&out.issues);
        assert_eq!(unreadable.message_key, ISSUE_FROM_JSON_UNREADABLE);
        assert_eq!(unreadable.severity, IssueSeverity::Warning);
        assert!(!unreadable.message_args["error"].is_empty());
    }
}

#[test]
fn finding_ids_do_not_depend_on_the_user() {
    let ids = |user: &str| {
        let mut setup = Setup::new();
        let rename = |p: &Path| PathBuf::from(s(p).replace("user", user));
        for path in setup.env.known_folders.values_mut() {
            *path = rename(path);
        }
        setup.env.user_name = user.to_owned();
        let vault = home(&setup.env, "Documents/Notes");
        setup.fs.add_dir(&s(&vault));
        let config = rename(&appdata(CONFIG));
        let text = format!(r#"{{"vaults":{{"a":{{"path":{}}}}}}}"#, q(&vault));
        setup
            .fs
            .add_file(&s(&config), 10, "-1d", Some(text.as_bytes()));
        let out = setup.expand(&obsidian(""));
        assert_eq!(out.findings.len(), 1, "{user}: {:?}", out.issues);
        out.findings
            .iter()
            .map(|f| f.id.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(ids("user"), ids("maxim"));
}

const MIXED: &str = r#"
  - id: app.data
    app: { id: app, name: App, kind: application }
    category: user_files
    title_key: rules.app.data
    targets:
      - from_json: { file: "{APPDATA}\\obsidian\\obsidian.json", select: "/vaults/*/path" }
        optional: OPTIONAL_JSON
      - path: "{APPDATA}\\App\\config.ini"
        optional: OPTIONAL_PATH
"#;

#[test]
fn from_json_targets_follow_the_firing_rule() {
    let mixed = |json: bool, path: bool| {
        rule(
            &MIXED
                .replace("OPTIONAL_JSON", &json.to_string())
                .replace("OPTIONAL_PATH", &path.to_string()),
        )
    };
    // A required `from_json` target without roots: the optional path alone
    // does not fire the rule, but problems with the config are reported.
    let mut setup = Setup::new();
    setup.file("App/config.ini");
    add_config(&mut setup, r#"{"vaults":{}}"#);
    let out = setup.expand(&mixed(false, true));
    assert!(out.findings.is_empty());
    assert_eq!(only_issue(&out.issues).message_key, ISSUE_FROM_JSON_EMPTY);
    assert_eq!(out.claimed_paths, [appdata(CONFIG)]);

    // With a root the rule fires and the optional target gives its finding.
    let vault = home(&setup.env, "Vault");
    setup.fs.add_dir(&s(&vault));
    add_config(
        &mut setup,
        &format!(r#"{{"vaults":{{"a":{{"path":{}}}}}}}"#, q(&vault)),
    );
    let out = setup.expand(&mixed(false, true));
    assert_eq!(
        templates(&out),
        [r"{HOME}\Vault", r"file:{APPDATA}\App\config.ini"]
    );
    assert_eq!(
        out.claimed_paths,
        [vault, appdata("App/config.ini"), appdata(CONFIG)]
    );

    // An optional `from_json` target is not read when the rule does not fire.
    let mut setup = Setup::new();
    add_config(&mut setup, "{ broken");
    let out = setup.expand(&mixed(true, false));
    assert_eq!(out, RuleOutput::default());
    // ... and read when it does.
    setup.file("App/config.ini");
    let out = setup.expander().expand(&mixed(true, false), MATCHED);
    assert_eq!(templates(&out), [r"file:{APPDATA}\App\config.ini"]);
    assert_eq!(only_issue(&out.issues).message_key, ISSUE_FROM_JSON_PARSE);
}
