//! The whole Score phase: `run()` (SPEC-09 §4.1, §5, T-09-08).

use sk_core::config::ScoringConfig;
use sk_core::model::{Category, Finding, Sensitivity, Totals};
use time::Duration;

use crate::nested_tests::{by_title, ev, file, fileset, finding, heuristic, rule};
use crate::score_table_tests::{env, llm, stats, MIB, NOW};
use crate::{
    run, ScoreInput, ScoreOutput, BAD_CONFIG_KEY, BAD_NUMBER_KEY, CATEGORY_CONFLICT_KEY,
    CONFLICT_TAG, IMPORTANCE_ARG, NESTED_TAG, RESTORABLE_TAG, SENSITIVE_SELECTED_KEY,
    UNMEASURED_TAG,
};

fn run_all(findings: Vec<Finding>, cfg: &ScoringConfig) -> ScoreOutput {
    let env = env();
    run(
        ScoreInput {
            findings,
            env: &env,
            now: NOW,
        },
        cfg,
    )
}

fn has_tag(f: &Finding, tag: &str) -> bool {
    f.tags.iter().any(|t| t == tag)
}

fn measured(mut f: Finding, bytes: u64, days_ago: i64) -> Finding {
    f.stats = Some(stats(bytes, days_ago));
    f
}

/// A scan-like input: a duplicate pair, a nested child, a cache, a
/// credential store and an unmeasured `FileSet`.
fn sample() -> Vec<Finding> {
    let parent = measured(
        finding(
            fileset("{APPDATA}\\Foo", &[], &[]),
            Category::AppData,
            "Foo",
        ),
        10 * MIB,
        3,
    );
    let child = measured(
        finding(
            file("{APPDATA}\\Foo\\settings.json"),
            Category::AppConfig,
            "settings",
        ),
        MIB,
        3,
    );
    let save = measured(
        finding(
            fileset("{APPDATA}\\Game\\Saves", &[], &[]),
            Category::GameSave,
            "Saves",
        ),
        5 * MIB,
        1,
    );
    // The same id seen by the heuristics as Unknown: merged into `save`.
    let mut save_dup = save.clone();
    save_dup.category = Category::Unknown;
    save_dup.evidence = vec![ev(heuristic("unk"), "evidence.unknown")];
    let cache = measured(
        finding(
            fileset("{LOCALAPPDATA}\\Cache", &[], &[]),
            Category::Cache,
            "Cache",
        ),
        50 * MIB,
        1,
    );
    let mut keys = measured(
        finding(file("{HOME}\\keys.kdbx"), Category::Credentials, "keys"),
        MIB,
        5,
    );
    keys.sensitivity = Sensitivity::High;
    let docs = finding(
        fileset("{HOME}\\Docs", &[], &[]),
        Category::UserFiles,
        "Docs",
    );
    vec![parent, child, save, save_dup, cache, keys, docs]
}

#[test]
fn empty_input_gives_empty_output() {
    let out = run_all(Vec::new(), &ScoringConfig::default());
    assert!(out.findings.is_empty());
    assert_eq!(out.totals, Totals::default());
    assert!(out.issues.is_empty());
}

#[test]
fn runs_all_steps() {
    let out = run_all(sample(), &ScoringConfig::default());

    // Merged by id: six findings remain.
    assert_eq!(out.findings.len(), 6);
    let save = by_title(&out.findings, "Saves");
    assert_eq!(save.category, Category::GameSave);
    assert_eq!(save.evidence.len(), 2);

    // Nested, scored and selected.
    let parent = by_title(&out.findings, "Foo");
    let child = by_title(&out.findings, "settings");
    assert!(has_tag(child, NESTED_TAG));
    assert_eq!(parent.children, vec![child.id.clone()]);
    assert!(!child.default_selected);
    assert!(out.findings.iter().all(|f| f.score.is_some()));
    assert!(parent.default_selected);
    assert!(save.default_selected);
    let cache = by_title(&out.findings, "Cache");
    assert!(!cache.default_selected);
    assert!(has_tag(cache, RESTORABLE_TAG));

    // Sorted by §4.8: Credentials, GameSave, UserFiles, AppData, AppConfig, Cache.
    let titles: Vec<_> = out.findings.iter().map(|f| f.title.as_str()).collect();
    assert_eq!(
        titles,
        ["keys", "Saves", "Docs", "Foo", "settings", "Cache"]
    );

    // Totals over top-level findings by `default_selected`.
    assert_eq!(out.totals.all.count, 5);
    assert_eq!(out.totals.all.bytes, (10 + 5 + 50 + 1) * MIB);
    assert_eq!(out.totals.all.selected_count, 4);
    assert_eq!(out.totals.all.selected_bytes, (10 + 5 + 1) * MIB);
    assert_eq!(out.totals.sensitive_selected, 1);
    assert!(!out.totals.by_category.contains_key(&Category::AppConfig));

    // One sensitive issue for the credential store.
    let keys: Vec<_> = out.issues.iter().map(|i| i.message_key.as_str()).collect();
    assert_eq!(keys, [SENSITIVE_SELECTED_KEY]);
}

#[test]
fn file_sets_without_stats_are_tagged_unmeasured() {
    let out = run_all(sample(), &ScoringConfig::default());
    let docs = by_title(&out.findings, "Docs");
    assert!(has_tag(docs, UNMEASURED_TAG));
    // R = 0.5, S = 0, and the size rule does not apply.
    assert_eq!(docs.score.as_ref().unwrap().components["recency"], 0.5);
    assert_eq!(docs.score.as_ref().unwrap().components["size_penalty"], 0.0);
    assert!(docs.default_selected);
    for title in ["Foo", "Saves", "Cache", "keys"] {
        assert!(!has_tag(by_title(&out.findings, title), UNMEASURED_TAG));
    }

    // Only `FileSet`s: a `File` without stats is not tagged.
    let bare = finding(file("{HOME}\\note.txt"), Category::UserFiles, "note");
    let out = run_all(vec![bare], &ScoringConfig::default());
    assert!(!has_tag(&out.findings[0], UNMEASURED_TAG));
    let sorted = out.findings[0].tags.windows(2).all(|w| w[0] < w[1]);
    assert!(sorted);
}

#[test]
fn output_does_not_depend_on_input_order() {
    let cfg = ScoringConfig::default();
    let forward = run_all(sample(), &cfg);
    let mut reversed = sample();
    reversed.reverse();
    assert_eq!(run_all(reversed, &cfg), forward);
    let mut rotated = sample();
    rotated.rotate_left(3);
    assert_eq!(run_all(rotated, &cfg), forward);
}

#[test]
fn now_comes_from_the_caller() {
    let old = measured(
        finding(fileset("{HOME}\\Old", &[], &[]), Category::AppData, "Old"),
        MIB,
        0,
    );
    let env = env();
    let at = |now| {
        let out = run(
            ScoreInput {
                findings: vec![old.clone()],
                env: &env,
                now,
            },
            &ScoringConfig::default(),
        );
        out.findings[0].score.as_ref().unwrap().components["recency"]
    };
    assert_eq!(at(NOW), 1.0);
    assert!(at(NOW + Duration::days(400)) < 1.0);
}

#[test]
fn invalid_config_is_replaced_and_reported() {
    let cfg = ScoringConfig {
        select_threshold: 2.0,
        ..ScoringConfig::default()
    };
    let out = run_all(sample(), &cfg);
    assert_eq!(out.issues[0].message_key, BAD_CONFIG_KEY);
    // The default threshold 0.40 is used, not 2.0.
    assert!(by_title(&out.findings, "Foo").default_selected);
}

#[test]
fn issues_come_in_phase_order() {
    let cfg = ScoringConfig {
        size_penalty_cap: f32::NAN,
        ..ScoringConfig::default()
    };
    // A dangerous conflict: Rule Reinstallable vs Heuristic GameSave C=0.8.
    let winner = finding(
        fileset("{LOCALAPPDATA}\\Bar", &[], &[]),
        Category::Reinstallable,
        "Bar",
    );
    let mut loser = winner.clone();
    loser.category = Category::GameSave;
    loser.evidence = vec![ev(heuristic("h"), "evidence.h")];
    // A NaN confidence (merge input check) and a non-numeric LLM importance
    // (scoring).
    let mut nan = finding(file("{HOME}\\nan.txt"), Category::AppConfig, "nan");
    nan.evidence[0].confidence = f32::NAN;
    let mut unknown = finding(
        fileset("{HOME}\\Mystery", &[], &[]),
        Category::Unknown,
        "Mystery",
    );
    let mut verdict = ev(llm(), "evidence.llm");
    verdict
        .message_args
        .insert(IMPORTANCE_ARG.to_owned(), "abc".to_owned());
    unknown.evidence = vec![verdict];
    let mut keys = finding(file("{HOME}\\keys.kdbx"), Category::Credentials, "keys");
    keys.evidence = vec![ev(rule("keys"), "evidence.rule_match")];

    let input = vec![unknown, keys, nan, loser, winner];
    let out = run_all(input.clone(), &cfg);
    let keys: Vec<_> = out.issues.iter().map(|i| i.message_key.as_str()).collect();
    assert_eq!(
        keys,
        [
            BAD_CONFIG_KEY,
            BAD_NUMBER_KEY,
            CATEGORY_CONFLICT_KEY,
            BAD_NUMBER_KEY,
            SENSITIVE_SELECTED_KEY,
        ]
    );
    assert!(has_tag(by_title(&out.findings, "Bar"), CONFLICT_TAG));

    let mut reversed = input;
    reversed.reverse();
    assert_eq!(run_all(reversed, &cfg).issues, out.issues);
}
