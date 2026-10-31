//! The eval set `fixtures/llm-eval` is well-formed (SPEC-08 §6.3), and the
//! eval metrics are computed correctly. No network.

#[allow(dead_code)]
#[path = "../examples/eval/dataset.rs"]
mod dataset;
#[allow(dead_code)]
#[path = "../examples/eval/report.rs"]
mod report;

use std::path::Path;

use sk_core::env::Environment;
use sk_core::model::Category;
use sk_core::privacy::redact;
use sk_llm::{final_check, user_message, ClassifyItem};

use dataset::{LoadedCase, CATEGORIES, MIN_CASES, MIN_PER_CATEGORY};
use report::{confusion_markdown, Column, Metrics, Outcome};

fn load() -> Vec<LoadedCase> {
    match dataset::load_dir(&dataset::default_dir()) {
        Ok(cases) => cases,
        Err(e) => panic!("eval set: {e}"),
    }
}

#[test]
fn eval_set_has_enough_cases_per_category() {
    let cases = load();
    assert!(cases.len() >= MIN_CASES, "{} cases", cases.len());
    for (category, n) in dataset::per_category(&cases) {
        if category != Category::Unknown {
            assert!(n >= MIN_PER_CATEGORY, "{category:?}: {n} cases");
        }
    }
    let traps = cases.iter().filter(|c| c.case.trap.is_some()).count();
    assert!(traps >= 20, "{traps} traps");
}

#[test]
fn eval_set_is_consistent() {
    let cases = load();
    let problems = dataset::problems(&cases);
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn eval_set_covers_the_traps_of_spec() {
    let cases = load();
    let trap = |needle: &str| {
        cases.iter().any(|c| {
            c.case
                .trap
                .as_deref()
                .is_some_and(|t| t.to_lowercase().contains(needle))
        })
    };
    // SPEC-08 §6.3: a large cache with .json, a game in Documents, an Electron app.
    assert!(trap("game in documents"));
    assert!(trap("electron"));
    let big_json_cache = cases.iter().any(|c| {
        c.case.expected_category == Category::Cache
            && c.case.summary.total_bytes > 1 << 30
            && c.case.summary.ext_histogram.iter().any(|e| e.ext == "json")
    });
    assert!(big_json_cache);
}

/// A fake environment whose user and machine names do not occur in the set
/// (the default fake user `user` is an ordinary word in real folder names).
fn env() -> Environment {
    let mut env = Environment::fake(Path::new(r"C:\Users\qzevaluser"));
    env.user_name = "qzevaluser".to_owned();
    env.machine_name = "QZ-EVAL-PC".to_owned();
    env
}

#[test]
fn eval_names_are_already_redacted() {
    // Names reach the LLM after `redact` (SPEC-03 §4.4); the set must look the same.
    // The path is a `PathTemplate` without `redact`, so real UWP package family
    // names and SID folders stay as they are (`final_check` allows them, T-08-15).
    let env = env();
    for LoadedCase { case, .. } in load() {
        let s = &case.summary;
        let names = s
            .sample_names
            .iter()
            .chain(s.top_children.iter().map(|c| &c.name));
        for name in names {
            assert_eq!(&redact(name, &env), name, "{}", case.id);
        }
    }
}

#[test]
fn eval_requests_pass_the_privacy_check() {
    let env = env();
    let cases = load();
    let items: Vec<ClassifyItem> = cases
        .iter()
        .enumerate()
        .map(|(n, c)| dataset::to_item(&c.case, dataset::wire_id(n)))
        .collect();
    for chunk in items.chunks(8) {
        let message = user_message(chunk);
        assert!(
            final_check(&message, &env).is_ok(),
            "{:?}",
            final_check(&message, &env)
        );
        assert!(!message.contains("C:\\\\Users"), "{message}");
    }
}

#[test]
fn mixed_order_is_deterministic_and_mixes_categories() {
    let cases = load();
    let a = dataset::mixed_order(&cases);
    assert_eq!(a, dataset::mixed_order(&cases));
    let first: std::collections::BTreeSet<Category> = a
        .iter()
        .take(8)
        .map(|&i| cases[i].case.expected_category)
        .collect();
    assert!(first.len() > 2, "first batch is too uniform: {first:?}");
}

fn case(
    id: &str,
    expected: Category,
    acceptable: &[Category],
    must_not: &[Category],
) -> LoadedCase {
    let json = serde_json::json!({
        "id": id,
        "expected_category": expected,
        "acceptable_categories": acceptable,
        "must_not": must_not,
        "trap": if must_not.is_empty() { None } else { Some("t") },
        "summary": {
            "path": "{APPDATA}\\X", "total_bytes": 1, "file_count": 1, "dir_count": 0,
            "max_depth": 0, "ext_histogram": [], "sample_names": [], "top_children": [],
            "markers": [], "truncated": false
        }
    });
    match serde_json::from_value(json) {
        Ok(case) => LoadedCase {
            file: "t.json".to_owned(),
            case,
        },
        Err(e) => panic!("{e}"),
    }
}

fn predicted(category: Category) -> Outcome {
    Outcome::Predicted {
        category,
        confidence: 0.9,
        app_guess: None,
        reason: "r".to_owned(),
    }
}

#[test]
fn metrics_count_strict_acceptable_and_errors() {
    let cases = [
        case("a", Category::GameSave, &[], &[]),
        case("b", Category::AppConfig, &[Category::AppData], &[]),
        case("c", Category::Cache, &[], &[Category::UserFiles]),
        case("d", Category::Credentials, &[], &[]),
        case("e", Category::Credentials, &[], &[]),
    ];
    let outcomes = [
        predicted(Category::GameSave),
        predicted(Category::AppData),
        predicted(Category::UserFiles),
        Outcome::Missing,
        Outcome::Failed("timeout".to_owned()),
    ];
    let m = Metrics::compute(&cases, &outcomes);
    assert_eq!((m.total, m.strict, m.acceptable), (5, 1, 2));
    assert_eq!(m.must_not_hits, 1);
    assert_eq!((m.traps, m.traps_acceptable), (1, 0));
    assert!((m.strict_accuracy() - 0.2).abs() < 1e-9);
    assert!((m.acceptable_accuracy() - 0.4).abs() < 1e-9);
    let row = &m.confusion[&Category::Credentials];
    assert_eq!(row.get(&Column::Missing), Some(&1));
    assert_eq!(row.get(&Column::Failed), Some(&1));
    assert_eq!(m.per_category[&Category::AppConfig], (1, 0, 1));

    let table = confusion_markdown(&m);
    assert!(table.contains("| game_save (gs) | 1 |"), "{table}");
    assert_eq!(table.lines().count(), 2 + 4, "{table}");
    assert_eq!(CATEGORIES.len(), 11);
}

#[test]
fn empty_run_has_zero_accuracy() {
    let m = Metrics::compute(&[], &[]);
    assert_eq!(m.strict_accuracy(), 0.0);
    assert_eq!(m.acceptable_accuracy(), 0.0);
}
