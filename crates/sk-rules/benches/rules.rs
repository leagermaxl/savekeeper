//! Benchmark of the rules engine on 500 synthetic rules (SPEC-04 §6,
//! NFR-04-01: load and compile ≤ 50 ms, run of all rules ≤ 1 s).
//!
//! Manual run only (SPEC-12 §2.2), never part of `cargo test`:
//!
//! ```text
//! cargo bench -p sk-rules --bench rules
//! SK_RULES_BENCH_REAL=1 cargo bench -p sk-rules --bench rules   # also this machine
//! ```
//!
//! The rules and the fake profile are generated in memory
//! (`tests/common/synthetic_rules.rs`). Measured:
//! - **compile** — `compile_yaml` of the generated texts (parse, validation,
//!   globs and regexes);
//! - **load** — `RuleSet::load` of the same files as a `rules.d` folder (read,
//!   compile, merge, order): the NFR-04-01 load figure; `parse_500` and
//!   `validate_500` split it into YAML parsing and validation with glob and
//!   regex compilation, `builtin` loads the real base for reference. The files (≈ 250 KB)
//!   are written to `<target>/tmp/sk-rules-bench/rules.d`, overwritten on
//!   every run and never removed;
//! - **run** — `RulesCollector::collect` over a `MemFs` profile with
//!   `MemRegistry`: the NFR-04-01 run figure.
//!
//! With `SK_RULES_BENCH_REAL=1` the built-in rules, and the built-in rules
//! plus the synthetic ones, also run against this machine (`RealFs`, the
//! system registry, `Environment::detect`). That part only reads.
//!
//! Each figure is first measured once and printed with the NFR verdict, then
//! handed over to criterion.

#![allow(clippy::expect_used, clippy::unwrap_used)] // benchmark binary: fail loudly

#[path = "../tests/common/synthetic_rules.rs"]
mod synthetic_rules;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use criterion::{BatchSize, Criterion, SamplingMode};
use sk_core::collector::{CollectContext, CollectOutput, Collector};
use sk_core::config::Config;
use sk_core::env::Environment;
use sk_core::fs::FsScanner;
use sk_core::registry::RegistryReader;
use sk_core::CancellationToken;
use sk_rules::compile::{compile, compile_yaml};
use sk_rules::schema::RuleFile;
use sk_rules::{RuleSet, RulesCollector};
use sk_scan::RealFs;
use synthetic_rules::SyntheticRules;
use tokio::runtime::Runtime;
use tokio::sync::mpsc::unbounded_channel;

/// NFR-04-01: rules in the benchmark.
const RULES: usize = 500;
/// Rules per generated file (the built-in base has ≈ 10 files).
const PER_FILE: usize = 50;
/// NFR-04-01: loading and compiling the rules.
const NFR_LOAD: Duration = Duration::from_millis(50);
/// NFR-04-01: running all rules.
const NFR_RUN: Duration = Duration::from_secs(1);

fn verdict(elapsed: Duration, limit: Duration) -> &'static str {
    if elapsed <= limit {
        "PASS"
    } else {
        "FAIL"
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// The generated files as a `rules.d` folder in Cargo's per-target temp dir.
fn write_rules_dir(rules: &SyntheticRules) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("sk-rules-bench")
        .join("rules.d");
    std::fs::create_dir_all(&dir).expect("create rules.d");
    for (name, text) in &rules.files {
        std::fs::write(dir.join(name), text).expect("write rule file");
    }
    dir
}

fn compile_all(rules: &SyntheticRules) -> usize {
    rules
        .files
        .iter()
        .map(|(name, text)| {
            compile_yaml(text)
                .unwrap_or_else(|e| panic!("{name}: {e:?}"))
                .rules
                .len()
        })
        .sum()
}

fn parse_all(rules: &SyntheticRules) -> Vec<RuleFile> {
    rules
        .files
        .iter()
        .map(|(_, text)| RuleFile::from_yaml(text).expect("valid YAML"))
        .collect()
}

fn load(builtin: bool, dir: &Path) -> RuleSet {
    let (set, issues) = RuleSet::load(builtin, Some(dir));
    assert!(issues.is_empty(), "{issues:?}");
    set
}

/// A collector and its context, reused by every iteration.
struct Run {
    collector: RulesCollector,
    ctx: CollectContext,
}

impl Run {
    fn new(
        set: RuleSet,
        env: Environment,
        fs: Arc<dyn FsScanner>,
        registry: Arc<dyn RegistryReader>,
    ) -> Self {
        // Progress events are dropped: the receiver is closed.
        let (events, _) = unbounded_channel();
        let ctx = CollectContext {
            env: Arc::new(env),
            config: Arc::new(Config::default()),
            scanner: fs,
            events,
            cancel: CancellationToken::new(),
        };
        let collector = RulesCollector::new(Arc::new(set)).with_registry(registry);
        Self { collector, ctx }
    }

    fn once(&self, rt: &Runtime) -> CollectOutput {
        rt.block_on(self.collector.collect(&self.ctx))
            .expect("rules collector failed")
    }
}

/// Times one run, prints it with the NFR-04-01 verdict, then benchmarks it.
fn bench_run(c: &mut Criterion, rt: &Runtime, label: &str, rules: usize, run: &Run) {
    let start = Instant::now();
    let out = run.once(rt);
    let first = start.elapsed();
    println!(
        "{label}: {rules} rules -> {} findings, {} claimed paths, {} issues; first run {:.2} ms; \
         NFR-04-01 run (<= {} ms): {}",
        out.findings.len(),
        out.claimed_paths.len(),
        out.issues.len(),
        ms(first),
        NFR_RUN.as_millis(),
        verdict(first, NFR_RUN)
    );
    let mut group = c.benchmark_group("rules_run");
    group
        .sampling_mode(SamplingMode::Flat)
        .sample_size(20)
        .measurement_time((first * 30).clamp(Duration::from_secs(1), Duration::from_secs(10)));
    group.bench_function(label, |b| b.iter(|| run.once(rt)));
    group.finish();
}

fn synthetic(c: &mut Criterion, rt: &Runtime, rules: &SyntheticRules, dir: &Path) {
    println!(
        "synthetic: {} rules in {} files, {} KB of YAML",
        rules.rules,
        rules.files.len(),
        rules.bytes() / 1024
    );

    let start = Instant::now();
    let compiled = compile_all(rules);
    let first = start.elapsed();
    assert_eq!(compiled, rules.rules);
    // Informational: files one after another; the NFR figure is `load` below.
    println!(
        "compile (files one by one, in memory): first {:.2} ms",
        ms(first)
    );

    let start = Instant::now();
    let set = load(false, dir);
    let first = start.elapsed();
    assert_eq!(set.len(), rules.rules);
    println!(
        "load (read rules.d + compile + merge): first {:.2} ms; NFR-04-01 load+compile (<= {} ms): {}",
        ms(first),
        NFR_LOAD.as_millis(),
        verdict(first, NFR_LOAD)
    );

    let start = Instant::now();
    let builtin = RuleSet::builtin().expect("built-in rules").len();
    let first = start.elapsed();
    println!(
        "built-in base (reference): {builtin} rules loaded in {:.2} ms, \
         {:.1} ms per 500 rules at the same per-rule cost",
        ms(first),
        ms(first) * RULES as f64 / builtin.max(1) as f64
    );

    let mut group = c.benchmark_group("rules_load");
    group
        .sample_size(20)
        .measurement_time(Duration::from_secs(12));
    // Where the time goes: YAML parsing alone, then validation and the
    // glob/regex compilation of already parsed files.
    group.bench_function("parse_500", |b| b.iter(|| parse_all(rules)));
    group.bench_function("validate_500", |b| {
        b.iter_batched(
            || parse_all(rules),
            |files| {
                files
                    .into_iter()
                    .map(|f| compile(f).expect("valid").rules.len())
                    .sum::<usize>()
            },
            BatchSize::SmallInput,
        )
    });
    group.bench_function("compile_500", |b| b.iter(|| compile_all(rules)));
    group.bench_function("load_500", |b| b.iter(|| load(false, dir)));
    // Reference: the real built-in base, for the per-rule cost of real rules.
    group.bench_function("builtin", |b| {
        b.iter(|| RuleSet::builtin().expect("built-in rules"))
    });
    group.finish();

    let root = PathBuf::from(if cfg!(windows) { r"C:\fake" } else { "/fake" });
    let (fs, env, registry) = synthetic_rules::profile(&root, rules.rules);
    let run = Run::new(set, env, Arc::new(fs), Arc::new(registry));
    let out = run.once(rt);
    assert_eq!(out.findings.len(), rules.expected_findings);
    assert!(out.issues.is_empty(), "{:?}", out.issues);
    bench_run(c, rt, "memfs_500", rules.rules, &run);
}

/// The built-in rules, then built-in plus synthetic, on this machine.
fn real(c: &mut Criterion, rt: &Runtime, dir: &Path) {
    let env = Environment::detect().expect("detect environment");
    let fs: Arc<dyn FsScanner> = Arc::new(RealFs::new(&env));
    let registry: Arc<dyn RegistryReader> = Arc::new(sk_core::registry::SystemRegistry);

    let start = Instant::now();
    let builtin = RuleSet::builtin().expect("built-in rules");
    println!(
        "real: built-in {} rules loaded in {:.2} ms",
        builtin.len(),
        ms(start.elapsed())
    );
    let count = builtin.len();
    let run = Run::new(builtin, env.clone(), fs.clone(), registry.clone());
    bench_run(c, rt, "real_builtin", count, &run);

    let start = Instant::now();
    let all = load(true, dir);
    println!(
        "real: built-in + synthetic {} rules loaded in {:.2} ms",
        all.len(),
        ms(start.elapsed())
    );
    let count = all.len();
    let run = Run::new(all, env, fs, registry);
    bench_run(c, rt, "real_builtin_plus_500", count, &run);
}

fn main() {
    let mut c = Criterion::default().configure_from_args();
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("tokio runtime");

    let rules = synthetic_rules::synthetic_rules(RULES, PER_FILE);
    let dir = write_rules_dir(&rules);
    synthetic(&mut c, &rt, &rules, &dir);

    if std::env::var_os("SK_RULES_BENCH_REAL").is_some_and(|v| v == "1") {
        real(&mut c, &rt, &dir);
    } else {
        println!(
            "real machine run skipped: set SK_RULES_BENCH_REAL=1 to run the rules on this machine"
        );
    }

    c.final_summary();
}
