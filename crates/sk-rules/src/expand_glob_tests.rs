//! Tests of path expansion: `glob_root`, multi-valued tokens, `*` claims.

use std::collections::HashSet;

use sk_core::fs::ReparseKind;
use sk_core::model::{IssueSeverity, Target};
use sk_core::template::PathTemplate;

use super::paths::wildcard_match;
use super::tests::{appdata, root, rule, s, templates, Setup};
use super::{ISSUE_GLOB_ROOT_TRUNCATED, MAX_GLOB_ROOT_MATCHES};

const JETBRAINS: &str = r#"
  - id: jetbrains.config
    app: { id: jetbrains, name: JetBrains IDEs, kind: dev_tool }
    category: dev_environment
    title_key: rules.jetbrains.config
    targets:
      - path: "{APPDATA}\\JetBrains\\*"
        glob_root: true
        include: ["options/**", "*.key"]
"#;

#[test]
fn glob_root_gives_a_finding_per_version() {
    let mut setup = Setup::new();
    for version in ["IntelliJIdea2024.1", "PyCharm2023.3", "Rider2024.2"] {
        setup.file(&format!("JetBrains/{version}/options/ide.general.xml"));
    }
    // Links in a `*` segment are not followed and do not match.
    setup.dir("JetBrains/WebStorm2024.1/options");
    setup.fs.add_reparse(
        &s(&appdata("JetBrains/WebStorm2024.1")),
        ReparseKind::Symlink,
    );
    let out = setup.expand(&rule(JETBRAINS));
    assert_eq!(
        templates(&out),
        [
            r"{APPDATA}\JetBrains\IntelliJIdea2024.1",
            r"{APPDATA}\JetBrains\PyCharm2023.3",
            r"{APPDATA}\JetBrains\Rider2024.2",
        ]
    );
    let ids: HashSet<_> = out.findings.iter().map(|f| f.id.clone()).collect();
    assert_eq!(ids.len(), 3);
    let Target::FileSet {
        resolved, include, ..
    } = &out.findings[0].target
    else {
        panic!("expected a FileSet");
    };
    assert_eq!(resolved, &appdata("JetBrains/IntelliJIdea2024.1"));
    assert_eq!(include, &["options/**", "*.key"]);
    assert_eq!(out.claimed_paths.len(), 3);
    assert!(out.issues.is_empty());
}

#[test]
fn glob_root_matches_partial_segments_and_skips_links() {
    let mut setup = Setup::new();
    setup
        .file("Adobe/Adobe Photoshop 2024/Adobe Photoshop 2024 Settings/a.psp")
        .file("Adobe/adobe photoshop cs6/Adobe Photoshop CS6 Settings/a.psp")
        .file("Adobe/Adobe Photoshop 2023/Other/a.psp")
        .file("Adobe/Adobe Photoshop.txt")
        .dir("Adobe/Lightroom");
    setup.fs.add_reparse(
        &s(&appdata("Adobe/Adobe Photoshop Link")),
        ReparseKind::Junction,
    );
    let out = setup.expand(&rule(
        r#"
  - id: adobe.settings
    app: { id: adobe, name: Adobe, kind: application }
    category: app_config
    title_key: rules.adobe.settings
    targets:
      - path: "{APPDATA}\\Adobe\\Adobe Photoshop *\\Adobe Photoshop * Settings"
        glob_root: true
"#,
    ));
    assert_eq!(
        templates(&out),
        [
            r"{APPDATA}\Adobe\Adobe Photoshop 2024\Adobe Photoshop 2024 Settings",
            r"{APPDATA}\Adobe\adobe photoshop cs6\Adobe Photoshop CS6 Settings",
        ]
    );
}

#[test]
fn glob_root_keeps_the_newest_fifty() {
    let mut setup = Setup::new();
    for i in 0..55 {
        // Day offsets: profile 0 is the newest.
        setup.fs.add_file(
            &s(&appdata(&format!("JetBrains/Ide{i:02}"))),
            1,
            &format!("-{}d", i + 1),
            None,
        );
    }
    let out = setup.expand(&rule(JETBRAINS));
    assert_eq!(out.findings.len(), MAX_GLOB_ROOT_MATCHES);
    let kept = templates(&out);
    assert!(kept.contains(&r"file:{APPDATA}\JetBrains\Ide00".to_owned()));
    assert!(kept.contains(&r"file:{APPDATA}\JetBrains\Ide49".to_owned()));
    assert!(!kept.contains(&r"file:{APPDATA}\JetBrains\Ide50".to_owned()));
    assert_eq!(out.issues.len(), 1);
    let issue = &out.issues[0];
    assert_eq!(issue.severity, IssueSeverity::Warning);
    assert_eq!(issue.message_key, ISSUE_GLOB_ROOT_TRUNCATED);
    assert_eq!(issue.path.as_deref(), Some(r"{APPDATA}\JetBrains\*"));
    assert_eq!(issue.message_args["matches"], "55");
    assert_eq!(issue.message_args["limit"], "50");
    assert_eq!(issue.message_args["rule_id"], "jetbrains.config");
}

#[test]
fn truncation_is_reported_only_when_the_rule_fires() {
    let mut setup = Setup::new();
    for i in 0..55 {
        setup.dir(&format!("JetBrains/Ide{i:02}"));
    }
    let out = setup.expand(&rule(
        r#"
  - id: jetbrains.config
    app: { id: jetbrains, name: JetBrains IDEs, kind: dev_tool }
    category: dev_environment
    title_key: rules.jetbrains.config
    targets:
      - path: "{APPDATA}\\JetBrains\\idea.properties"
      - path: "{APPDATA}\\JetBrains\\*"
        glob_root: true
        optional: true
"#,
    ));
    assert_eq!(out, super::RuleOutput::default());
}

#[test]
fn every_steam_user_gets_its_own_finding() {
    let mut setup = Setup::new();
    let steam = root().join("Steam");
    setup.env.launchers.push(sk_core::env::LauncherInfo {
        id: "steam".to_owned(),
        root: Some(steam.clone()),
        user_ids: Vec::new(),
        games: Vec::new(),
    });
    setup.resolve.steam_user_ids = vec!["111".to_owned(), "222".to_owned(), "333".to_owned()];
    for user in ["111", "222"] {
        let config = steam.join("userdata").join(user).join("config");
        setup
            .fs
            .add_file(&s(&config.join("localconfig.vdf")), 1, "-1d", None);
    }
    let out = setup.expand(&rule(
        r#"
  - id: steam.userdata-config
    app: { id: steam, name: Steam, kind: application }
    category: game_config
    title_key: rules.steam.userdata_config
    targets:
      - path: "{STEAM}\\userdata\\{STEAM_USERID}\\config"
"#,
    ));
    assert_eq!(
        templates(&out),
        [
            r"{STEAM}\userdata\111\config",
            r"{STEAM}\userdata\222\config"
        ]
    );
    assert_ne!(out.findings[0].id, out.findings[1].id);
}

#[test]
fn all_drives_become_drive_letters() {
    let setup = Setup::new();
    let template = PathTemplate::parse(r"{DRIVE:*}\RetroArch").unwrap_or_else(|e| panic!("{e}"));
    let specialized = template.specialize(&setup.env, &setup.resolve);
    let texts: Vec<&str> = specialized.iter().map(PathTemplate::as_str).collect();
    // `Environment::fake` has one fixed drive, C.
    assert_eq!(texts, [r"{DRIVE:C}\RetroArch"]);

    let plain = PathTemplate::parse(r"{APPDATA}\X").unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(plain.specialize(&setup.env, &setup.resolve), [plain]);
    let steam =
        PathTemplate::parse(r"{STEAM}\userdata\{STEAM_USERID}").unwrap_or_else(|e| panic!("{e}"));
    assert!(steam.specialize(&setup.env, &setup.resolve).is_empty());
    // `{PACKAGE:name}` is not specialized (SPEC-02 §3.2).
    let package = PathTemplate::parse(r"{PACKAGE:Microsoft.WindowsTerminal}\LocalState")
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(package.specialize(&setup.env, &setup.resolve), [package]);
}

#[test]
fn drive_findings_get_the_drive_letter() {
    let mut setup = Setup::new();
    let drive_c = PathTemplate::parse(r"{DRIVE:C}\RetroArch").unwrap_or_else(|e| panic!("{e}"));
    let resolved = drive_c.resolve(&setup.env, &setup.resolve).remove(0);
    setup.fs.add_dir(&s(&resolved));
    let out = setup.expand(&rule(
        r#"
  - id: retroarch.saves
    app: { id: retroarch, name: RetroArch, kind: application }
    category: game_save
    title_key: rules.retroarch.saves
    targets:
      - path: "{DRIVE:*}\\RetroArch"
"#,
    ));
    assert_eq!(templates(&out), [r"{DRIVE:C}\RetroArch"]);
    let expected = Target::FileSet {
        root: drive_c,
        resolved,
        include: Vec::new(),
        exclude: Vec::new(),
    };
    assert_eq!(
        out.findings[0].id,
        sk_core::model::FindingId::for_target(&expected)
    );
}

#[test]
fn claims_expand_wildcards() {
    let mut setup = Setup::new();
    setup
        .dir("Chrome/User Data/Default/Cache")
        .dir("Chrome/User Data/Profile 1/Cache")
        .dir("Chrome/User Data/Profile 2/Code Cache")
        .dir("Chrome/User Data/Linked/Cache");
    setup.fs.add_reparse(
        &s(&appdata("Chrome/User Data/Linked")),
        ReparseKind::Junction,
    );
    let out = setup.expand(&rule(
        r#"
  - id: chrome.none
    app: { id: chrome, name: Chrome, kind: application }
    category: cache
    title_key: rules.chrome.none
    claims:
      - "{APPDATA}\\Chrome\\User Data\\*\\Cache"
      - "{APPDATA}\\Spotify"
"#,
    ));
    assert!(out.findings.is_empty());
    // A `*` claim keeps existing paths only; a plain claim is not checked.
    assert_eq!(
        out.claimed_paths,
        [
            appdata("Chrome/User Data/Default/Cache"),
            appdata("Chrome/User Data/Profile 1/Cache"),
            appdata("Spotify"),
        ]
    );
}

#[test]
fn wildcard_matching() {
    assert!(wildcard_match("*", "anything"));
    assert!(wildcard_match("*", ""));
    assert!(wildcard_match("Adobe Photoshop *", "adobe photoshop 2024"));
    assert!(wildcard_match("*.pst", "Archive.PST"));
    assert!(wildcard_match("a*b*c", "aXXbYYbc"));
    assert!(wildcard_match("ПРОФИЛЬ*", "профиль 1"));
    assert!(!wildcard_match("*.pst", "Archive.ost"));
    assert!(!wildcard_match("a*b", "ac"));
    assert!(!wildcard_match("abc", "ab"));
    assert!(!wildcard_match("ab", "abc"));
    // Glob syntax other than `*` is literal.
    assert!(wildcard_match("[a]?", "[A]?"));
    assert!(!wildcard_match("?", "x"));
}
