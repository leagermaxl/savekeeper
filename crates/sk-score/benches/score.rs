//! Benchmark of the Score phase on 10 000 synthetic findings with nesting up
//! to 8 levels deep (SPEC-09 §6.4, NFR-09-01: ≤ 100 ms in release).
//!
//! Manual run only (SPEC-12 §2.2), never part of `cargo test` or CI:
//!
//! ```text
//! cargo bench -p sk-score --bench score
//! ```
//!
//! The findings are generated in memory: 1 250 chains of 8 nested targets
//! `{APPDATA}\AppN\L1\…\L7` (seven `FileSet`s and a `File` at the bottom),
//! with categories mixed so that both absorption and the FR-09-03 excludes of
//! §4.4 happen, every tenth finding duplicated under the same id (§4.3), and
//! varied stats, evidence and tags. Measured: the whole [`run`] (config
//! validation, merges, scoring, selection, sorting and totals).
//!
//! The figure is first measured once and printed with the NFR verdict, then
//! handed over to criterion.

#![allow(clippy::expect_used, clippy::unwrap_used)] // benchmark binary: fail loudly

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use criterion::{BatchSize, Criterion};
use sk_core::config::ScoringConfig;
use sk_core::env::Environment;
use sk_core::model::{
    AppKind, AppRef, Category, Evidence, EvidenceSource, Finding, FindingId, Sensitivity, Target,
    TargetStats,
};
use sk_core::template::{PathTemplate, ResolveContext};
use sk_score::{run, ScoreInput, IMPORTANCE_ARG};
use time::macros::datetime;
use time::OffsetDateTime;

/// Number of findings (NFR-09-01).
const FINDINGS: usize = 10_000;
/// Nesting depth of a chain (SPEC-09 §6.4).
const DEPTH: usize = 8;
/// NFR-09-01 budget.
const NFR: Duration = Duration::from_millis(100);
const NOW: OffsetDateTime = datetime!(2026-09-28 12:00 UTC);
const MIB: u64 = 1024 * 1024;

/// Categories of the levels of a chain: important data under restorable or
/// unknown folders (FR-09-03) as well as plain absorption.
const LEVELS: [Category; DEPTH] = [
    Category::AppData,
    Category::Cache,
    Category::GameSave,
    Category::AppConfig,
    Category::Unknown,
    Category::UserFiles,
    Category::Reinstallable,
    Category::Credentials,
];

fn env() -> Environment {
    let root = PathBuf::from(if cfg!(windows) { r"C:\fake" } else { "/fake" });
    let mut env = Environment::fake(&root);
    env.is_elevated = false;
    env
}

fn source(i: usize) -> EvidenceSource {
    let id = format!("src{}", i % 7);
    match i % 5 {
        0 => EvidenceSource::Rule { rule_id: id },
        1 => EvidenceSource::Ludusavi {
            game: id,
            manifest_version: "1".to_owned(),
        },
        2 => EvidenceSource::System { exporter_id: id },
        3 => EvidenceSource::Heuristic { heuristic_id: id },
        _ => EvidenceSource::Llm {
            provider: id,
            model: "m".to_owned(),
        },
    }
}

fn evidence(i: usize) -> Evidence {
    let mut message_args = BTreeMap::new();
    if i % 5 == 4 {
        message_args.insert(IMPORTANCE_ARG.to_owned(), "0.6".to_owned());
    }
    Evidence {
        source: source(i),
        message_key: format!("evidence.k{}", i % 3),
        message_args,
        confidence: (i % 10) as f32 / 10.0,
        importance: None,
    }
}

fn stats(i: usize) -> TargetStats {
    TargetStats {
        total_bytes: (i as u64 % 97) * 37 * MIB,
        file_count: 1 + i as u64 % 50,
        dir_count: i as u64 % 5,
        newest_mtime: Some(NOW - time::Duration::days((i % 1500) as i64)),
        oldest_mtime: None,
        locked_files: 0,
        cloud_only_bytes: 0,
        cloud_only_files: 0,
        largest_file_bytes: None,
        truncated: false,
    }
}

fn target(env: &Environment, chain: usize, level: usize) -> Target {
    let mut template = format!("{{APPDATA}}\\App{chain}");
    for l in 1..=level {
        template.push_str(&format!("\\L{l}"));
    }
    if level + 1 == DEPTH {
        template.push_str(".dat");
    }
    let parsed = PathTemplate::parse(&template).expect("valid template");
    let resolved = parsed
        .resolve(env, &ResolveContext::default())
        .into_iter()
        .next()
        .expect("resolvable template");
    if level + 1 == DEPTH {
        Target::File {
            path: parsed,
            resolved,
        }
    } else {
        Target::FileSet {
            root: parsed,
            resolved,
            include: vec![],
            exclude: if chain.is_multiple_of(4) {
                vec!["**/*.log".to_owned()]
            } else {
                vec![]
            },
        }
    }
}

fn finding(env: &Environment, i: usize) -> Finding {
    let (chain, level) = (i / DEPTH, i % DEPTH);
    let target = target(env, chain, level);
    let mut tags = vec![];
    if i.is_multiple_of(11) {
        tags.push("cloud-synced".to_owned());
    }
    if i.is_multiple_of(13) {
        tags.push("git-unsaved".to_owned());
    }
    Finding {
        id: FindingId::for_target(&target),
        target,
        category: LEVELS[level],
        app: (!i.is_multiple_of(3)).then(|| AppRef {
            id: format!("app{chain}"),
            name: format!("App {chain}"),
            kind: AppKind::Application,
            source_ids: BTreeMap::new(),
            installed: Some(i.is_multiple_of(2)),
            process_names: vec![],
        }),
        title: format!("App {chain} level {level}"),
        evidence: vec![evidence(i), evidence(i + 1)],
        stats: (!i.is_multiple_of(17)).then(|| stats(i)),
        sensitivity: if i.is_multiple_of(19) {
            Sensitivity::High
        } else {
            Sensitivity::None
        },
        score: None,
        default_selected: false,
        requires_elevation: i.is_multiple_of(23),
        tags,
        children: vec![],
        notes_key: None,
    }
}

/// `FINDINGS` findings: chains of `DEPTH`, every tenth one duplicated under
/// the same id with another category and evidence.
fn synthetic(env: &Environment) -> Vec<Finding> {
    let mut out = Vec::with_capacity(FINDINGS);
    let mut i = 0;
    while out.len() < FINDINGS {
        let f = finding(env, i);
        if i % 10 == 9 && out.len() + 1 < FINDINGS {
            let mut dup = f.clone();
            dup.category = Category::Unknown;
            dup.evidence = vec![evidence(i + 3)];
            out.push(dup);
        }
        out.push(f);
        i += 1;
    }
    out
}

fn once(findings: Vec<Finding>, env: &Environment, cfg: &ScoringConfig) -> sk_score::ScoreOutput {
    run(
        ScoreInput {
            findings,
            env,
            now: NOW,
        },
        cfg,
    )
}

fn main() {
    let mut c = Criterion::default().configure_from_args();
    let env = env();
    let cfg = ScoringConfig::default();
    let input = synthetic(&env);
    assert_eq!(input.len(), FINDINGS);

    let batch = input.clone();
    let start = Instant::now();
    let out = once(batch, &env, &cfg);
    let first = start.elapsed();
    let nested = out
        .findings
        .iter()
        .filter(|f| f.tags.iter().any(|t| t == sk_score::NESTED_TAG))
        .count();
    println!(
        "{FINDINGS} findings (depth {DEPTH}) -> {} merged, {nested} nested, {} selected, {} issues; \
         first run {:.2} ms; NFR-09-01 (<= {} ms): {}",
        out.findings.len(),
        out.totals.all.selected_count,
        out.issues.len(),
        first.as_secs_f64() * 1000.0,
        NFR.as_millis(),
        if first <= NFR { "ok" } else { "EXCEEDED" }
    );

    let mut group = c.benchmark_group("score");
    group.sample_size(20);
    group.bench_function("run_10000", |b| {
        b.iter_batched(
            || input.clone(),
            |findings| once(findings, &env, &cfg),
            BatchSize::LargeInput,
        )
    });
    group.finish();
    c.final_summary();
}
