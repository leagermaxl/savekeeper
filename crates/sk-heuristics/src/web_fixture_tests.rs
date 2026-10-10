//! H-WEB on the fixtures (SPEC-07 §6, T-07-06): the Electron app of
//! `fixtures/fs/electron-app.yaml` and the Electron and Chromium profiles of
//! `fixtures/fs/heuristics/web-profiles.yaml`, alone and before H-UNK.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sk_core::collector::CollectContext;
use sk_core::config::{Config, HeuristicsConfig};
use sk_core::env::{Environment, KnownFolder};
use sk_core::model::{Category, EvidenceSource, Finding, Sensitivity, Target};
use sk_core::path::{eq_ci, PathSet};
use sk_core::template::PathTemplate;
use sk_core::CancellationToken;
use sk_scan::MemFs;
use sk_testkit::mem_fixture;
use tokio::sync::mpsc::unbounded_channel;

use super::tests::{cache_plus, globs, root_of, run_web, under};
use super::*;
use crate::collector::run;
use crate::heuristic::Heuristic;
use crate::unk::HUnk;

fn root() -> &'static Path {
    Path::new(if cfg!(windows) { r"C:\fake" } else { "/fake" })
}

fn web_profiles() -> (MemFs, Environment) {
    mem_fixture("heuristics/web-profiles", root())
}

/// What the browser rules (SPEC-04) would have claimed.
fn chrome(env: &Environment) -> PathBuf {
    under(env, KnownFolder::LocalAppData, "Google/Chrome/User Data")
}

fn template(env: &Environment, f: &Finding) -> String {
    PathTemplate::from_path(root_of(f), env).as_str().to_owned()
}

fn by_root<'a>(env: &Environment, findings: &'a [Finding], root: &str) -> Vec<&'a Finding> {
    findings
        .iter()
        .filter(|f| template(env, f) == root)
        .collect()
}

#[test]
fn electron_fixture_saves_all_but_the_caches() {
    let (fs, env) = mem_fixture("electron-app", root());
    let cfg = HeuristicsConfig::default();
    let out = run_web(&fs, &env, &PathSet::new(), &cfg, &CancellationToken::new());

    let [f] = out.findings.as_slice() else {
        panic!("{:?}", out.findings)
    };
    assert_eq!(template(&env, f), r"{APPDATA}\Obsidian");
    assert_eq!(f.title, "Obsidian — heuristic.title.app_data");
    assert_eq!(f.category, Category::AppData);
    assert_eq!(f.evidence[0].message_key, "heuristic.web.electron");
    // Include: the whole folder; exclude: the cache rows of §4.6.
    let (include, exclude) = globs(f);
    assert!(include.is_empty());
    assert_eq!(exclude, cache_plus(&[]));
    assert_eq!(f.sensitivity, Sensitivity::None);
    assert_eq!(out.claimed.len(), 1);
    assert!(out.issues.is_empty());
}

#[test]
fn web_profiles_fixture_gives_one_finding_per_profile() {
    let (fs, env) = web_profiles();
    let claimed: PathSet = [chrome(&env)].into_iter().collect();
    let cfg = HeuristicsConfig::default();
    let out = run_web(&fs, &env, &claimed, &cfg, &CancellationToken::new());
    assert!(out.issues.is_empty(), "{:?}", out.issues);

    let found: Vec<(String, &str, &[String], Sensitivity)> = out
        .findings
        .iter()
        .map(|f| {
            (
                template(&env, f),
                f.evidence[0].message_key.as_str(),
                globs(f).1,
                f.sensitivity,
            )
        })
        .collect();
    let electron = cache_plus(&[]);
    let chromite = cache_plus(&["Default/Extensions"]);
    let expected: Vec<(String, &str, &[String], Sensitivity)> = vec![
        (
            r"{APPDATA}\Chatter".to_owned(),
            REASON_ELECTRON,
            &electron,
            Sensitivity::High,
        ),
        (
            r"{APPDATA}\WebShell".to_owned(),
            REASON_CHROMIUM,
            &electron,
            Sensitivity::None,
        ),
        (
            r"{LOCALAPPDATA}\Chromite\User Data".to_owned(),
            REASON_CHROMIUM,
            &chromite,
            Sensitivity::High,
        ),
    ];
    assert_eq!(found, expected);
    for f in &out.findings {
        assert!(globs(f).0.is_empty(), "{}", f.title);
        assert_eq!(f.category, Category::AppData);
    }
    let chromite = &out.findings[2];
    assert_eq!(chromite.title, "Chromite — heuristic.title.app_data");
    assert_eq!(chromite.app.as_ref().unwrap().name, "Chromite");
    assert_eq!(chromite.tags, ["browser-credentials"]);
    assert_eq!(chromite.notes_key.as_deref(), Some("heuristic.dpapi_bound"));
    let claimed: Vec<String> = out
        .claimed
        .iter()
        .map(|p| PathTemplate::from_path(p, &env).as_str().to_owned())
        .collect();
    assert_eq!(
        claimed,
        [
            r"{APPDATA}\Chatter",
            r"{APPDATA}\WebShell",
            r"{LOCALAPPDATA}\Chromite\User Data"
        ]
    );
}

fn heuristic_of(f: &Finding) -> &str {
    match &f.evidence[0].source {
        EvidenceSource::Heuristic { heuristic_id } => heuristic_id,
        other => panic!("{other:?}"),
    }
}

fn context(fs: MemFs, env: Environment) -> CollectContext {
    let (events, _rx) = unbounded_channel();
    CollectContext {
        env: Arc::new(env),
        config: Arc::new(Config::default()),
        scanner: Arc::new(fs),
        events,
        cancel: CancellationToken::new(),
    }
}

/// Before H-UNK, a profile is neither `Unknown` nor found twice; the rest of
/// its application folder is H-UNK's, without the profile.
#[test]
fn profiles_are_not_unknown_for_h_unk() {
    let (fs, env) = web_profiles();
    let ctx = context(fs, env.clone());
    let heuristics: Vec<Box<dyn Heuristic>> = vec![Box::new(HWeb::new()), Box::new(HUnk::new())];
    let claimed: PathSet = [chrome(&env)].into_iter().collect();
    let cfg = HeuristicsConfig::default();
    let out = run(&heuristics, &cfg, &ctx, Vec::new(), claimed.clone());
    assert!(out.issues.is_empty(), "{:?}", out.issues);

    for root in [
        r"{APPDATA}\Chatter",
        r"{APPDATA}\WebShell",
        r"{LOCALAPPDATA}\Chromite\User Data",
    ] {
        let found = by_root(&env, &out.findings, root);
        let [f] = found.as_slice() else {
            panic!("{root}: {found:?}")
        };
        assert_eq!(f.category, Category::AppData, "{root}");
        assert_eq!(heuristic_of(f), "h-web");
    }
    // The application folder of `User Data`: a finding of H-UNK on the rest.
    let [rest] = by_root(&env, &out.findings, r"{LOCALAPPDATA}\Chromite")[..] else {
        panic!("{:?}", out.findings)
    };
    let Target::FileSet { exclude, .. } = &rest.target else {
        panic!("{:?}", rest.target)
    };
    assert!(exclude.iter().any(|e| e == "User Data"), "{exclude:?}");
    // The caches-only folder and the folder without markers are H-UNK's.
    for root in [r"{LOCALAPPDATA}\Updater", r"{APPDATA}\Notepad4"] {
        let found = by_root(&env, &out.findings, root);
        let [f] = found.as_slice() else {
            panic!("{root}: {found:?}")
        };
        assert_eq!(heuristic_of(f), "h-unk");
    }
    // The claimed browser gives nothing.
    let chrome = chrome(&env);
    assert!(out
        .findings
        .iter()
        .all(|f| !eq_ci(root_of(f), &chrome) && !root_of(f).starts_with(&chrome)));
    // No summary of a profile goes to the LLM.
    assert!(out
        .unknown_summaries
        .iter()
        .all(|s| !s.path.as_str().contains("Chatter") && !s.path.as_str().contains("User Data")));

    // The same file system gives the same output.
    let again = run(&heuristics, &cfg, &ctx, Vec::new(), claimed);
    assert_eq!(again, out);
}
