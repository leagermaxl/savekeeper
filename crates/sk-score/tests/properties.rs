//! Properties of the Score phase (SPEC-09 §6.3, T-09-09):
//! - a permutation of the input does not change the output (NFR-09-02);
//! - `0 ≤ value ≤ 1` for every finding;
//! - the sum of `selected_bytes` over the categories equals `all.selected_bytes`;
//! - no `Cache`/`Reinstallable` finding is selected.
//!
//! The case count is 256 by default, so `cargo test` stays fast; the
//! acceptance run of SPEC-09 §8 sets it explicitly:
//!
//! ```text
//! PROPTEST_CASES=10000 cargo test --release -p sk-score --test properties
//! ```

// Helpers outside `#[test]` functions are not covered by `allow-unwrap-in-tests`.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use proptest::prelude::*;
use proptest::test_runner::Config;
use sk_core::config::ScoringConfig;
use sk_core::env::Environment;
use sk_core::model::{
    AppKind, AppRef, Category, Evidence, EvidenceSource, Finding, FindingId, RegHive, Sensitivity,
    Target, TargetStats,
};
use sk_core::template::{PathTemplate, ResolveContext};
use sk_score::{run, ScoreInput, ScoreOutput, IMPORTANCE_ARG, VALUE};
use time::macros::datetime;
use time::{Duration, OffsetDateTime};

const NOW: OffsetDateTime = datetime!(2026-09-28 12:00 UTC);
const MIB: u64 = 1024 * 1024;

const CATEGORIES: [Category; 11] = [
    Category::GameSave,
    Category::GameConfig,
    Category::AppConfig,
    Category::AppData,
    Category::UserFiles,
    Category::DevEnvironment,
    Category::Credentials,
    Category::SystemSettings,
    Category::Reinstallable,
    Category::Cache,
    Category::Unknown,
];

/// Roots and path components: few of them, so that duplicate ids and nesting
/// (with case differences) are frequent.
const ROOTS: [&str; 2] = ["{APPDATA}", "{LOCALAPPDATA}"];
const COMPONENTS: [&str; 5] = ["Foo", "foo", "Saves", "cfg.ini", "x"];
const GLOBS: [&str; 5] = ["*.ini", "**", "Saves/**", "Saves", "**/*.sav"];
const TAGS: [&str; 5] = [
    "git-unsaved",
    "git-no-remote",
    "cloud-synced",
    "alternative",
    "git-clean",
];

fn cases() -> u32 {
    std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(256)
}

fn config() -> Config {
    Config {
        cases: cases(),
        // No `proptest-regressions` files in the source tree.
        failure_persistence: None,
        ..Config::default()
    }
}

fn env(elevated: bool) -> Environment {
    let mut env = Environment::fake(Path::new("/fake"));
    env.is_elevated = elevated;
    env
}

fn resolve(template: &PathTemplate) -> PathBuf {
    template
        .resolve(&env(false), &ResolveContext::default())
        .into_iter()
        .next()
        .unwrap()
}

fn path_template() -> impl Strategy<Value = String> {
    (
        prop::sample::select(&ROOTS[..]),
        prop::collection::vec(prop::sample::select(&COMPONENTS[..]), 1..=4),
    )
        .prop_map(|(root, parts)| format!("{root}\\{}", parts.join("\\")))
}

fn globs() -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec(prop::sample::select(&GLOBS[..]), 0..=2)
        .prop_map(|g| g.into_iter().map(str::to_owned).collect())
}

fn target() -> impl Strategy<Value = Target> {
    prop_oneof![
        5 => (path_template(), globs(), globs()).prop_map(|(t, include, exclude)| {
            let root = PathTemplate::parse(&t).unwrap();
            Target::FileSet { resolved: resolve(&root), root, include, exclude }
        }),
        3 => path_template().prop_map(|t| {
            let path = PathTemplate::parse(&t).unwrap();
            Target::File { resolved: resolve(&path), path }
        }),
        1 => prop::sample::select(&COMPONENTS[..]).prop_map(|k| Target::Registry {
            hive: RegHive::Hkcu,
            key: format!("Software\\{k}"),
            recursive: true,
        }),
    ]
}

/// Any `f32`, including NaN, ∞ and values outside `[0, 1]`.
fn number() -> impl Strategy<Value = f32> {
    prop_oneof![
        8 => 0.0f32..=1.0,
        1 => -2.0f32..3.0,
        1 => prop::sample::select(vec![f32::NAN, f32::INFINITY, f32::NEG_INFINITY]),
    ]
}

fn source() -> impl Strategy<Value = EvidenceSource> {
    (0u8..7, 0u8..3).prop_map(|(kind, n)| {
        let id = format!("s{n}");
        match kind {
            0 => EvidenceSource::User,
            1 => EvidenceSource::Rule { rule_id: id },
            2 => EvidenceSource::Ludusavi {
                game: id,
                manifest_version: "1".to_owned(),
            },
            3 => EvidenceSource::Launcher { launcher: id },
            4 => EvidenceSource::System { exporter_id: id },
            5 => EvidenceSource::Heuristic { heuristic_id: id },
            _ => EvidenceSource::Llm {
                provider: id,
                model: "m".to_owned(),
            },
        }
    })
}

fn evidence() -> impl Strategy<Value = Evidence> {
    (
        source(),
        0u8..3,
        number(),
        prop::option::of(number()),
        prop::option::of(prop::sample::select(vec!["0.3", "0.9", "abc", "NaN"])),
    )
        .prop_map(|(source, key, confidence, importance, arg)| {
            let mut message_args = BTreeMap::new();
            if let Some(arg) = arg {
                message_args.insert(IMPORTANCE_ARG.to_owned(), arg.to_owned());
            }
            Evidence {
                source,
                message_key: format!("evidence.k{key}"),
                message_args,
                confidence,
                importance,
            }
        })
}

fn stats() -> impl Strategy<Value = TargetStats> {
    (0u64..=64 * 1024 * MIB, -30i64..3000).prop_map(|(bytes, days_ago)| TargetStats {
        total_bytes: bytes,
        file_count: 1,
        dir_count: 0,
        newest_mtime: Some(NOW - Duration::days(days_ago)),
        oldest_mtime: None,
        locked_files: 0,
        cloud_only_bytes: 0,
        cloud_only_files: 0,
        largest_file_bytes: None,
        truncated: false,
    })
}

fn app() -> impl Strategy<Value = AppRef> {
    (0u8..2, prop::option::of(any::<bool>())).prop_map(|(n, installed)| AppRef {
        id: format!("app{n}"),
        name: format!("App {n}"),
        kind: AppKind::Application,
        source_ids: BTreeMap::new(),
        installed,
        process_names: vec![],
    })
}

fn sensitivity() -> impl Strategy<Value = Sensitivity> {
    prop::sample::select(vec![Sensitivity::None, Sensitivity::Low, Sensitivity::High])
}

fn finding() -> impl Strategy<Value = Finding> {
    (
        target(),
        prop::sample::select(&CATEGORIES[..]),
        prop::option::of(app()),
        0u8..4,
        // At least one: a finding without evidence trips the `debug_assert!` of §5.
        prop::collection::vec(evidence(), 1..=3),
        prop::option::of(stats()),
        sensitivity(),
        any::<bool>(),
        prop::sample::subsequence(&TAGS[..], 0..=2),
    )
        .prop_map(
            |(target, category, app, title, evidence, stats, sensitivity, elevation, tags)| {
                Finding {
                    id: FindingId::for_target(&target),
                    target,
                    category,
                    app,
                    title: format!("Title {title}"),
                    evidence,
                    stats,
                    sensitivity,
                    score: None,
                    default_selected: false,
                    requires_elevation: elevation,
                    tags: tags.into_iter().map(str::to_owned).collect(),
                    children: vec![],
                    notes_key: None,
                }
            },
        )
}

fn findings() -> impl Strategy<Value = Vec<Finding>> {
    prop::collection::vec(finding(), 0..=24)
}

fn run_with(findings: Vec<Finding>, elevated: bool) -> ScoreOutput {
    let env = env(elevated);
    run(
        ScoreInput {
            findings,
            env: &env,
            now: NOW,
        },
        &ScoringConfig::default(),
    )
}

proptest! {
    #![proptest_config(config())]

    /// NFR-09-02: the output does not depend on the input order.
    #[test]
    fn permutation_does_not_change_output(
        (input, shuffled) in findings().prop_flat_map(|v| (Just(v.clone()), Just(v).prop_shuffle())),
        elevated in any::<bool>(),
    ) {
        let expected = run_with(input, elevated);
        let actual = run_with(shuffled, elevated);
        prop_assert_eq!(actual, expected);
    }

    /// `0 ≤ value ≤ 1` for any input, including NaN and out-of-range numbers.
    #[test]
    fn value_is_in_unit_interval(input in findings(), elevated in any::<bool>()) {
        let out = run_with(input, elevated);
        for f in &out.findings {
            let score = f.score.as_ref().unwrap();
            prop_assert!((0.0..=1.0).contains(&score.value), "value {} of {:?}", score.value, f.title);
            prop_assert_eq!(score.components.get(VALUE).copied(), Some(score.value));
        }
    }

    /// The per-category `selected_bytes` add up to `all.selected_bytes`
    /// (and the same for the other counters).
    #[test]
    fn category_totals_add_up(input in findings(), elevated in any::<bool>()) {
        let totals = run_with(input, elevated).totals;
        let by_category = totals.by_category.values();
        let selected_bytes: u64 = by_category.clone().map(|c| c.selected_bytes).sum();
        let bytes: u64 = by_category.clone().map(|c| c.bytes).sum();
        let count: u32 = by_category.clone().map(|c| c.count).sum();
        let selected_count: u32 = by_category.map(|c| c.selected_count).sum();
        prop_assert_eq!(selected_bytes, totals.all.selected_bytes);
        prop_assert_eq!(bytes, totals.all.bytes);
        prop_assert_eq!(count, totals.all.count);
        prop_assert_eq!(selected_count, totals.all.selected_count);
    }

    /// FR-09-05: `Cache` and `Reinstallable` are never selected by default.
    #[test]
    fn restorable_is_never_selected(input in findings(), elevated in any::<bool>()) {
        let out = run_with(input, elevated);
        for f in &out.findings {
            let restorable = matches!(f.category, Category::Cache | Category::Reinstallable);
            prop_assert!(!(restorable && f.default_selected), "{:?} {:?} selected", f.category, f.title);
        }
    }
}
