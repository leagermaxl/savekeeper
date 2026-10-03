//! Regression guard for NFR-04-01 (SPEC-04 §3.2): 500 synthetic rules load,
//! compile and run on a fake profile, give the expected findings, and stay
//! far below generous time bounds even in a debug build.
//!
//! The real numbers come from the manual benchmark
//! `cargo bench -p sk-rules --bench rules` (release build).

mod common;
#[path = "common/synthetic_rules.rs"]
mod synthetic_rules;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use sk_rules::compile::compile_yaml;
use sk_rules::RuleSet;

/// Rules in the synthetic base (NFR-04-01).
const RULES: usize = 500;
/// Rules per generated file.
const PER_FILE: usize = 50;
/// Debug-build bounds against gross regressions only: an unoptimized build
/// on a busy CI runner is many times slower than the release NFR
/// (50 ms load, 1 s run).
const DEBUG_LOAD_BOUND: Duration = Duration::from_secs(10);
const DEBUG_RUN_BOUND: Duration = Duration::from_secs(10);

/// The generated files as a `rules.d` folder in Cargo's per-target temp dir.
/// The files are overwritten on every run and never removed (a few hundred KB).
fn write_rules_dir(rules: &synthetic_rules::SyntheticRules) -> std::io::Result<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("sk-rules-perf-test")
        .join("rules.d");
    std::fs::create_dir_all(&dir)?;
    for (name, text) in &rules.files {
        std::fs::write(dir.join(name), text)?;
    }
    Ok(dir)
}

#[tokio::test]
async fn synthetic_500_rules_load_and_run() {
    let rules = synthetic_rules::synthetic_rules(RULES, PER_FILE);

    // Every generated file compiles without warnings.
    for (name, text) in &rules.files {
        let compiled = compile_yaml(text).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert!(
            compiled.warnings.is_empty(),
            "{name}: {:?}",
            compiled.warnings
        );
    }

    let dir = write_rules_dir(&rules).unwrap();
    let start = Instant::now();
    let (set, issues) = RuleSet::load(false, Some(&dir));
    let load = start.elapsed();
    assert_eq!(issues, vec![]);
    assert_eq!(set.len(), RULES);

    let (fs, env, registry) = synthetic_rules::profile(&common::root(), RULES);
    let start = Instant::now();
    let out = common::collect(set, fs, env, registry).await;
    let run = start.elapsed();

    assert_eq!(out.issues, vec![]);
    assert_eq!(out.findings.len(), rules.expected_findings);
    assert!(!out.claimed_paths.is_empty());
    println!(
        "{RULES} rules ({} KB): load {:.1} ms, run {:.1} ms -> {} findings, {} claimed paths",
        rules.bytes() / 1024,
        load.as_secs_f64() * 1000.0,
        run.as_secs_f64() * 1000.0,
        out.findings.len(),
        out.claimed_paths.len()
    );
    assert!(load < DEBUG_LOAD_BOUND, "load took {load:?}");
    assert!(run < DEBUG_RUN_BOUND, "run took {run:?}");
}
