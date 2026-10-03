use std::path::{Path, PathBuf};

use sk_core::config::Config;
use sk_core::env::{LauncherInfo, StoreUser};
use sk_core::model::{EvidenceSource, Finding, IssueSeverity, RegHive, ScanIssue, Target};
use sk_core::registry::MemRegistry;
use sk_scan::MemFs;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

use super::*;
use crate::compile::compile_yaml;
use crate::conditions::ISSUE_REGISTRY_ACCESS_DENIED;

fn root() -> PathBuf {
    PathBuf::from(if cfg!(windows) { r"C:\fake" } else { "/fake" })
}

/// Absolute path of `rel` (`/`-separated) under `{APPDATA}` of the fake env.
fn appdata(rel: &str) -> PathBuf {
    let base = root()
        .join("Users")
        .join("user")
        .join("AppData")
        .join("Roaming");
    rel.split('/').fold(base, |p, c| p.join(c))
}

fn s(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// A set of `rules` (YAML list items, indented by 2).
fn set(yaml: &str) -> Arc<RuleSet> {
    let text = format!("schema_version: 1\nrules:\n{yaml}");
    match compile_yaml(&text) {
        Ok(file) => Arc::new(RuleSet::from_rules(file.rules)),
        Err(errors) => panic!("invalid test rules: {errors:?}\n{text}"),
    }
}

/// A rule `id` with one target `path` and the given `priority`.
fn rule(id: &str, priority: i32, path: &str) -> String {
    format!(
        r#"
  - id: {id}
    app: {{ id: app, name: App, kind: application }}
    category: app_config
    title_key: rules.{id}
    priority: {priority}
    targets:
      - path: "{path}"
"#
    )
}

fn context(env: Environment, fs: MemFs) -> (CollectContext, UnboundedReceiver<Event>) {
    let (events, rx) = unbounded_channel();
    let ctx = CollectContext {
        env: Arc::new(env),
        config: Arc::new(Config::default()),
        scanner: Arc::new(fs),
        events,
        cancel: CancellationToken::new(),
    };
    (ctx, rx)
}

fn fake_env() -> Environment {
    Environment::fake(&root())
}

fn rule_ids(finding: &Finding) -> Vec<String> {
    finding
        .evidence
        .iter()
        .map(|e| match &e.source {
            EvidenceSource::Rule { rule_id } => rule_id.clone(),
            other => panic!("unexpected evidence source {other:?}"),
        })
        .collect()
}

fn progress(rx: &mut UnboundedReceiver<Event>) -> Vec<(u64, Option<u64>, Option<String>)> {
    let mut out = Vec::new();
    while let Ok(event) = rx.try_recv() {
        match event {
            Event::Progress {
                phase,
                done,
                total,
                current,
            } => {
                assert_eq!(phase, ScanPhase::Collect);
                out.push((done, total, current));
            }
            other => panic!("unexpected event {other:?}"),
        }
    }
    out
}

#[test]
fn id_and_display_key() {
    let collector = RulesCollector::new(Arc::new(RuleSet::default()));
    assert_eq!(collector.id(), "rules");
    assert_eq!(collector.display_key(), "collector.rules");
}

#[tokio::test]
async fn collects_findings_claims_and_issues_of_all_rules() {
    let mut fs = MemFs::new();
    fs.add_dir(&s(&appdata("Alpha")));
    fs.add_file(&s(&appdata("Beta/beta.ini")), 10, "-1d", None);
    let yaml = [
        rule("alpha.config", 100, "{APPDATA}\\\\Alpha"),
        rule("beta.config", 100, "{APPDATA}\\\\Beta\\\\beta.ini"),
        rule("gamma.config", 100, "{APPDATA}\\\\Gamma"),
        r#"
  - id: shared.claims
    app: { id: shared, name: Shared, kind: application }
    category: app_config
    title_key: rules.shared
    claims: ["{APPDATA}\\Alpha", "{APPDATA}\\Cache"]
    conditions:
      - registry_exists: { hive: hkcu, key: "Software\\Denied" }
      - exists: "{APPDATA}\\Alpha"
"#
        .to_owned(),
    ]
    .concat();
    let mut registry = MemRegistry::new();
    registry.add_denied_key(RegHive::Hkcu, "Software\\Denied");
    let collector = RulesCollector::new(set(&yaml)).with_registry(Arc::new(registry));
    let (ctx, _rx) = context(fake_env(), fs);

    let out = collector.collect(&ctx).await.unwrap();

    let roots: Vec<String> = out
        .findings
        .iter()
        .map(|f| match &f.target {
            Target::FileSet { root, .. } => root.as_str().to_owned(),
            Target::File { path, .. } => format!("file:{}", path.as_str()),
            other => panic!("unexpected target {other:?}"),
        })
        .collect();
    assert_eq!(roots, [r"{APPDATA}\Alpha", r"file:{APPDATA}\Beta\beta.ini"]);
    // The denied registry key fails the conditions of `shared.claims`: its
    // claims are not added, and the key is reported once.
    assert_eq!(
        out.claimed_paths,
        [appdata("Alpha"), appdata("Beta/beta.ini")]
    );
    assert_eq!(out.issues.len(), 1);
    assert_eq!(out.issues[0].message_key, ISSUE_REGISTRY_ACCESS_DENIED);
    assert_eq!(out.issues[0].severity, IssueSeverity::Info);
}

#[tokio::test]
async fn claimed_paths_are_kept_once() {
    let mut fs = MemFs::new();
    fs.add_dir(&s(&appdata("Alpha")));
    let yaml = [
        rule("alpha.config", 100, "{APPDATA}\\\\Alpha"),
        r#"
  - id: alpha.claims
    app: { id: alpha, name: Alpha, kind: application }
    category: app_config
    title_key: rules.alpha
    claims: ["{APPDATA}\\Alpha", "{APPDATA}\\Alpha\\Cache"]
"#
        .to_owned(),
    ]
    .concat();
    let (ctx, _rx) = context(fake_env(), fs);
    let out = RulesCollector::new(set(&yaml))
        .with_registry(Arc::new(MemRegistry::new()))
        .collect(&ctx)
        .await
        .unwrap();
    // `alpha.claims` comes first in id order.
    assert_eq!(
        out.claimed_paths,
        [appdata("Alpha"), appdata("Alpha/Cache")]
    );
}

#[tokio::test]
async fn same_finding_id_keeps_higher_priority_and_appends_evidence() {
    let mut fs = MemFs::new();
    fs.add_dir(&s(&appdata("Shared")));
    let yaml = [
        rule("a.low", 50, "{APPDATA}\\\\Shared"),
        rule("z.high", 200, "{APPDATA}\\\\Shared"),
        rule("m.mid", 100, "{APPDATA}\\\\Shared"),
    ]
    .concat();
    let (ctx, _rx) = context(fake_env(), fs);
    let out = RulesCollector::new(set(&yaml))
        .with_registry(Arc::new(MemRegistry::new()))
        .collect(&ctx)
        .await
        .unwrap();
    assert_eq!(out.findings.len(), 1);
    let finding = &out.findings[0];
    assert_eq!(finding.title, "rules.z.high");
    assert_eq!(rule_ids(finding), ["z.high", "m.mid", "a.low"]);
    assert_eq!(out.claimed_paths, [appdata("Shared")]);
}

#[tokio::test]
async fn same_priority_keeps_the_rule_first_by_id() {
    let mut fs = MemFs::new();
    fs.add_dir(&s(&appdata("Shared")));
    let yaml = [
        rule("b.second", 100, "{APPDATA}\\\\Shared"),
        rule("a.first", 100, "{APPDATA}\\\\Shared"),
    ]
    .concat();
    let (ctx, _rx) = context(fake_env(), fs);
    let out = RulesCollector::new(set(&yaml))
        .with_registry(Arc::new(MemRegistry::new()))
        .collect(&ctx)
        .await
        .unwrap();
    assert_eq!(out.findings.len(), 1);
    assert_eq!(out.findings[0].title, "rules.a.first");
    assert_eq!(rule_ids(&out.findings[0]), ["a.first", "b.second"]);
}

#[test]
fn merge_replaces_a_finding_of_a_lower_priority_rule() {
    // `merge` does not rely on the set order: a later rule with a higher
    // priority takes the finding over.
    let mut fs = MemFs::new();
    fs.add_dir(&s(&appdata("Shared")));
    let env = fake_env();
    let registry = MemRegistry::new();
    let resolve = ResolveContext::default();
    let rules = [
        rule("low", 10, "{APPDATA}\\\\Shared"),
        rule("high", 90, "{APPDATA}\\\\Shared"),
    ]
    .concat();
    let compiled = compile_yaml(&format!("schema_version: 1\nrules:\n{rules}"))
        .unwrap()
        .rules;
    let evaluator = ConditionEvaluator::new(&env, &fs, &registry, &resolve);
    let expander = TargetExpander::from_evaluator(&evaluator);
    let outputs: Vec<(&CompiledRule, RuleOutput)> = compiled
        .iter()
        .map(|r| (r, expander.expand(r, evaluator.evaluate(&r.rule))))
        .collect();
    let out = merge(outputs.into_iter());
    assert_eq!(out.findings.len(), 1);
    assert_eq!(out.findings[0].title, "rules.high");
    assert_eq!(rule_ids(&out.findings[0]), ["high", "low"]);
}

#[tokio::test]
async fn progress_reports_every_n_rules_and_the_last_one() {
    let yaml: String = (0..25)
        .map(|i| rule(&format!("r{i:02}"), 100, "{APPDATA}\\\\Missing"))
        .collect();
    let (ctx, mut rx) = context(fake_env(), MemFs::new());
    let out = RulesCollector::new(set(&yaml))
        .with_registry(Arc::new(MemRegistry::new()))
        .collect(&ctx)
        .await
        .unwrap();
    assert!(out.findings.is_empty());

    let events = progress(&mut rx);
    assert!(!events.is_empty());
    // Throttling may hold back intermediate events, never the last one.
    let done: Vec<u64> = events.iter().map(|e| e.0).collect();
    assert!(done.windows(2).all(|w| w[0] < w[1]), "{done:?}");
    assert!(done
        .iter()
        .all(|d| d.is_multiple_of(PROGRESS_EVERY) || *d == 25));
    let (last_done, last_total, last_current) = events.last().unwrap();
    assert_eq!((*last_done, *last_total), (25, Some(25)));
    let current = last_current.as_deref().unwrap();
    assert!(current.starts_with('r') && current.len() == 3, "{current}");
}

#[tokio::test]
async fn cancelled_scan_gives_nothing() {
    let mut fs = MemFs::new();
    fs.add_dir(&s(&appdata("Alpha")));
    let (ctx, mut rx) = context(fake_env(), fs);
    ctx.cancel.cancel();
    let out = RulesCollector::new(set(&rule("alpha.config", 100, "{APPDATA}\\\\Alpha")))
        .with_registry(Arc::new(MemRegistry::new()))
        .collect(&ctx)
        .await
        .unwrap();
    assert_eq!(out, CollectOutput::default());
    assert!(progress(&mut rx).is_empty());
}

#[tokio::test]
async fn empty_set_gives_nothing() {
    let (ctx, mut rx) = context(fake_env(), MemFs::new());
    let out = RulesCollector::new(Arc::new(RuleSet::default()))
        .collect(&ctx)
        .await
        .unwrap();
    assert_eq!(out, CollectOutput::default());
    assert!(progress(&mut rx).is_empty());
}

#[tokio::test]
async fn steam_user_ids_come_from_the_steam_launcher() {
    let steam = root().join("Steam");
    let mut env = fake_env();
    env.launchers.push(LauncherInfo {
        id: "steam".to_owned(),
        root: Some(steam.clone()),
        user_ids: ["111", "222"]
            .into_iter()
            .map(|id| StoreUser {
                id: id.to_owned(),
                alt_id: None,
                name: None,
            })
            .collect(),
        games: Vec::new(),
    });
    let mut fs = MemFs::new();
    for id in ["111", "222"] {
        fs.add_dir(&s(&steam.join("userdata").join(id).join("config")));
    }
    let yaml = rule(
        "steam.userdata-config",
        100,
        "{STEAM}\\\\userdata\\\\{STEAM_USERID}\\\\config",
    );
    let (ctx, _rx) = context(env, fs);
    let out = RulesCollector::new(set(&yaml))
        .with_registry(Arc::new(MemRegistry::new()))
        .collect(&ctx)
        .await
        .unwrap();
    let roots: Vec<String> = out
        .findings
        .iter()
        .map(|f| match &f.target {
            Target::FileSet { root, .. } => root.as_str().to_owned(),
            other => panic!("unexpected target {other:?}"),
        })
        .collect();
    assert_eq!(
        roots,
        [
            r"{STEAM}\userdata\111\config",
            r"{STEAM}\userdata\222\config"
        ]
    );
}

/// Rules meeting the denied key `Software\Denied` (HKCU), in set order:
/// `z.target` (registry target), `m.condition` and `a.condition`
/// (`registry_exists`); `m.condition` and `a.condition` also meet the denied
/// key `Software\Other`.
const DENIED_RULES: &str = r#"
  - id: a.condition
    app: { id: a, name: A, kind: application }
    category: app_config
    title_key: rules.a
    priority: 50
    claims: ["{APPDATA}\\A"]
    conditions:
      - any_of:
          - registry_exists: { hive: hkcu, key: "software\\denied" }
          - registry_exists: { hive: hkcu, key: "Software\\Other" }
  - id: m.condition
    app: { id: m, name: M, kind: application }
    category: app_config
    title_key: rules.m
    priority: 100
    claims: ["{APPDATA}\\M"]
    conditions:
      - any_of:
          - registry_exists: { hive: hkcu, key: "Software\\Other" }
          - registry_exists: { hive: hkcu, key: "Software\\Denied" }
  - id: z.target
    app: { id: z, name: Z, kind: application }
    category: app_config
    title_key: rules.z
    priority: 200
    targets:
      - registry: { hive: hkcu, key: "Software\\Denied" }
"#;

fn denied_registry() -> MemRegistry {
    let mut registry = MemRegistry::new();
    registry.add_denied_key(RegHive::Hkcu, "Software\\Denied");
    registry.add_denied_key(RegHive::Hkcu, "Software\\Other");
    registry
}

/// `(message_key, rule_id, key)` of each issue.
fn issue_summary(issues: &[ScanIssue]) -> Vec<(String, String, String)> {
    issues
        .iter()
        .map(|i| {
            let arg = |name: &str| i.message_args.get(name).cloned().unwrap_or_default();
            (i.message_key.clone(), arg("rule_id"), arg("key"))
        })
        .collect()
}

/// The issues of `DENIED_RULES`: each key names the earliest rule of the
/// set that met it, sorted by `rule_id`.
fn expected_denied() -> Vec<(String, String, String)> {
    [
        ("m.condition", r"Software\Other"),
        ("z.target", r"Software\Denied"),
    ]
    .into_iter()
    .map(|(rule, key)| {
        (
            ISSUE_REGISTRY_ACCESS_DENIED.to_owned(),
            rule.to_owned(),
            key.to_owned(),
        )
    })
    .collect()
}

#[tokio::test]
async fn once_per_scan_issues_are_deterministic() {
    let collector =
        RulesCollector::new(set(DENIED_RULES)).with_registry(Arc::new(denied_registry()));
    for _ in 0..50 {
        let (ctx, _rx) = context(fake_env(), MemFs::new());
        let out = collector.collect(&ctx).await.unwrap();
        assert!(out.findings.is_empty());
        assert_eq!(issue_summary(&out.issues), expected_denied());
    }
}

#[test]
fn once_per_scan_issue_names_the_earliest_rule_in_any_processing_order() {
    // The rules are processed last to first, as a thread schedule could.
    let set = set(DENIED_RULES);
    let env = fake_env();
    let fs = MemFs::new();
    let registry = denied_registry();
    let resolve = ResolveContext::default();
    let evaluator = ConditionEvaluator::new(&env, &fs, &registry, &resolve)
        .rank_rules(set.rules().iter().map(CompiledRule::id));
    let expander = TargetExpander::from_evaluator(&evaluator);
    for rule in set.rules().iter().rev() {
        let out = expander.expand(rule, evaluator.evaluate(&rule.rule));
        // Held back until the end of the run.
        assert!(out.issues.is_empty(), "{:?}", out.issues);
    }
    let mut issues = evaluator.take_issues();
    issues.sort_by(|a, b| issue_order(a).cmp(&issue_order(b)));
    assert_eq!(issue_summary(&issues), expected_denied());
}
