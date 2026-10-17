//! Benchmark of H-USR on a generated tree (SPEC-07 §6, NFR-07-02).
//!
//! Manual run only (SPEC-12 §2.2), never part of `cargo test`:
//!
//! ```text
//! SK_BENCH_DIR='D:\savekeeper-bench' cargo bench -p sk-heuristics --bench usr
//! ```
//!
//! Environment variables (as in SPEC-03 §6):
//! - `SK_BENCH_DIR` — folder for the tree, **required**: without it the
//!   benchmark prints a message and does nothing. On Windows a folder on
//!   drive `C:` is refused before anything is created;
//! - `SK_BENCH_FILES` — number of files in the tree (default 1 000 000 under
//!   `cargo bench`, 1 000 when the target is run in test mode).
//!
//! The tree is persistent: it is generated once in `SK_BENCH_DIR\usr-<N>`
//! and reused by later runs. The marker `SK_BENCH_DIR\usr-<N>.marker` next to
//! it describes the tree and is written last, so an interrupted generation is
//! reported instead of being reused. The benchmark never deletes or
//! overwrites anything; a folder without a matching marker is an error that
//! the user resolves by hand.
//!
//! Layout ([`tree`]): the shape of the SPEC-03 tree (10 subfolders per
//! folder, 100 files per leaf), extensions cycling over the groups of
//! SPEC-07 §4.3.2, almost all files empty. Every hundredth leaf holds one
//! 21 MiB `.docx` (made by `set_len`, no data written): about one hundred
//! clusters. Some leaves are ≥ 30% binary, some keep their files in junk
//! folders (`cache`, `logs`), some hold a small `.kdbx` outside any cluster.
//!
//! Measured: `HeuristicsCollector` with only `usr` switched on (git, unk and
//! web off, junk on) on `RealFs`, with `Environment::detect()` without drives
//! and `scan.extra_roots = [tree]`. One timed run is reported with the number
//! of clusters and the verdict against NFR-07-02 scaled to the entries of the
//! tree (2 million files ≤ 90 s, so 1 million ≤ 45 s); a child process
//! (Windows) reports its peak memory (`Get-Process`, as in the SPEC-05
//! benchmark) against 300 MB. Then criterion takes its samples.

#![allow(clippy::expect_used, clippy::unwrap_used)] // benchmark binary: fail loudly

#[path = "usr/tree.rs"]
mod tree;

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use criterion::{Criterion, SamplingMode, Throughput};
use sk_core::collector::{CollectContext, CollectOutput, PostCollector, PriorResults};
use sk_core::config::{Config, HeuristicsConfig};
use sk_core::env::Environment;
use sk_core::fs::FsScanner;
use sk_core::model::{IssueSeverity, ScanIssue};
use sk_core::path::PathSet;
use sk_core::CancellationToken;
use sk_heuristics::HeuristicsCollector;
use sk_scan::RealFs;
use tokio::runtime::Runtime;
use tokio::sync::mpsc::unbounded_channel;

use tree::{drive_letter, prepare_tree, TreeShape};

/// NFR-07-02: 2 million files in at most 90 s, scaled linearly by entries.
const NFR_FILES: f64 = 2_000_000.0;
const NFR_SECONDS: f64 = 90.0;
/// NFR-07-02: memory, bytes (300 MB, not scaled).
const NFR_MEMORY: u64 = 300_000_000;
/// Files in the tree under `cargo bench` (SPEC-07 §6).
const FULL_FILES: u64 = 1_000_000;
/// Files in the tree when the target runs in test mode (`cargo test --benches`).
const SMOKE_FILES: u64 = 1_000;
/// Environment variable that turns this binary into the memory probe; its
/// value is the root of the tree.
const PROBE_VAR: &str = "SK_USR_BENCH_PROBE";
/// Reason key of a cluster finding (SPEC-07 §4.3.3).
const REASON_CLUSTER: &str = "heuristic.usr.cluster";
/// Reason key of a single valuable file (SPEC-07 §4.3.3 p. 5).
const REASON_FILE: &str = "heuristic.usr.valuable_file";

/// What is needed to run the collector on the tree.
struct Setup {
    collector: HeuristicsCollector,
    ctx: CollectContext,
    runtime: Runtime,
}

impl Setup {
    fn new(root: &Path) -> Result<Self, String> {
        let detected =
            Environment::detect().map_err(|e| format!("cannot detect the environment: {e}"))?;
        // The scanner keeps the drives for its thread limit (NFR-03-03); the
        // heuristics walk only the tree.
        let scanner: Arc<dyn FsScanner> = Arc::new(RealFs::new(&detected));
        let mut env = detected;
        env.drives.clear();
        let mut config = Config::default();
        config.scan.extra_roots = vec![root.to_path_buf()];
        let mut cfg = HeuristicsConfig::default();
        cfg.enabled.usr = true;
        cfg.enabled.junk = true;
        cfg.enabled.git = false;
        cfg.enabled.unk = false;
        cfg.enabled.web = false;
        let (events, _rx) = unbounded_channel();
        let ctx = CollectContext {
            env: Arc::new(env),
            config: Arc::new(config),
            scanner,
            events,
            cancel: CancellationToken::new(),
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .map_err(|e| format!("cannot start the runtime: {e}"))?;
        Ok(Self {
            collector: HeuristicsCollector::new(cfg),
            ctx,
            runtime,
        })
    }

    /// One run of the collector; a fresh walk each time (the walk cache
    /// lives for one `collect`).
    fn run(&self) -> CollectOutput {
        let claimed = PathSet::default();
        let prior = PriorResults {
            findings: &[],
            claimed: &claimed,
        };
        self.runtime
            .block_on(self.collector.collect(&self.ctx, &prior))
            .expect("heuristics collector")
    }
}

/// Findings with the evidence `key`.
fn count(out: &CollectOutput, key: &str) -> usize {
    out.findings
        .iter()
        .filter(|f| f.evidence.iter().any(|e| e.message_key == key))
        .count()
}

/// Issues worse than `Info`: a walk problem would make the numbers meaningless.
fn problems(out: &CollectOutput) -> Vec<&ScanIssue> {
    out.issues
        .iter()
        .filter(|i| i.severity != IssueSeverity::Info)
        .collect()
}

fn summary(out: &CollectOutput) -> String {
    format!(
        "{} findings: {} clusters, {} valuable files, {} other; {} issues",
        out.findings.len(),
        count(out, REASON_CLUSTER),
        count(out, REASON_FILE),
        out.findings.len() - count(out, REASON_CLUSTER) - count(out, REASON_FILE),
        out.issues.len()
    )
}

fn verdict(pass: bool) -> &'static str {
    if pass {
        "PASS"
    } else {
        "FAIL"
    }
}

/// `SK_BENCH_DIR` as an absolute path; on Windows refused on drive `C:`
/// (subst drives, junctions and UNC paths to `C:` are not resolved).
fn bench_dir(raw: &OsStr) -> Result<PathBuf, String> {
    let dir = std::path::absolute(raw)
        .map_err(|e| format!("cannot resolve SK_BENCH_DIR {raw:?}: {e}"))?;
    if cfg!(windows) && drive_letter(&dir) == Some('C') {
        return Err(format!(
            "SK_BENCH_DIR {} is on drive C:, which is not allowed for generated test data; \
             choose a folder on another drive",
            dir.display()
        ));
    }
    Ok(dir)
}

/// Memory counters of this process from `Get-Process`: working set, peak
/// working set and private bytes (commit). `None` off Windows or without PowerShell.
fn memory() -> Option<(u64, u64, u64)> {
    if !cfg!(windows) {
        return None;
    }
    let script = format!(
        "$p = Get-Process -Id {}; '{{0}} {{1}} {{2}}' -f $p.WorkingSet64, \
         $p.PeakWorkingSet64, $p.PrivateMemorySize64",
        std::process::id()
    );
    let out = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let v: Vec<u64> = text
        .split_whitespace()
        .filter_map(|n| n.parse().ok())
        .collect();
    match v[..] {
        [ws, peak_ws, private] => Some((ws, peak_ws, private)),
        _ => None,
    }
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / 1_000_000.0
}

/// The memory probe: one run of the collector in a fresh process.
fn probe(root: &Path) -> Result<(), String> {
    let setup = Setup::new(root)?;
    let Some((base, _, _)) = memory() else {
        println!("memory probe: Get-Process gave no counters, memory not measured");
        return Ok(());
    };
    let out = setup.run();
    let (after, peak_ws, private) = memory().unwrap_or((base, base, base));
    println!(
        "memory probe (child process, Get-Process): working set {:.1} MB before the run, \
         {:.1} MB after it (private {:.1} MB); peak working set {:.1} MB; {}; \
         NFR-07-02 (peak <= {} MB): {}",
        mb(base),
        mb(after),
        mb(private),
        mb(peak_ws),
        summary(&out),
        NFR_MEMORY / 1_000_000,
        verdict(peak_ws <= NFR_MEMORY)
    );
    Ok(())
}

/// Runs [`probe`] in a child process (Windows), or says why not.
fn spawn_probe(root: &Path) {
    if !cfg!(windows) {
        println!("memory probe skipped: Get-Process is Windows-only");
        return;
    }
    let exe = std::env::current_exe().expect("benchmark executable");
    match Command::new(exe).env(PROBE_VAR, root).status() {
        Ok(s) if s.success() => {}
        other => println!("memory probe failed: {other:?}"),
    }
}

fn bench_usr(c: &mut Criterion, base: &Path, files: u64) -> Result<(), String> {
    let shape = TreeShape::new(files);
    let (root, generated) = prepare_tree(base, &shape)?;
    let origin = match generated {
        Some(t) => format!("generated in {:.1} s", t.as_secs_f64()),
        None => "reused".to_owned(),
    };
    println!(
        "tree: {} files, {} folders, {} entries, {:.2} GiB of file data in {} ({origin})",
        shape.files,
        shape.dirs(),
        shape.entries(),
        shape.bytes() as f64 / f64::from(1u32 << 30),
        root.display(),
    );

    let setup = Setup::new(&root)?;
    let start = Instant::now();
    let out = setup.run();
    let first = start.elapsed();
    let issues = problems(&out);
    if !issues.is_empty() {
        return Err(format!("the run reported problems: {issues:?}"));
    }
    if count(&out, REASON_CLUSTER) == 0 {
        return Err(format!("no clusters found: {}", summary(&out)));
    }
    let budget = NFR_SECONDS * shape.files as f64 / NFR_FILES;
    let rate = shape.entries() as f64 / first.as_secs_f64();
    println!(
        "first run: {:.3} s, {rate:.0} entries/s; {}; NFR-07-02 scaled to {} files \
         (<= {budget:.3} s): {}",
        first.as_secs_f64(),
        summary(&out),
        shape.files,
        verdict(first.as_secs_f64() <= budget)
    );

    spawn_probe(&root);

    let mut group = c.benchmark_group("h_usr");
    group
        .sampling_mode(SamplingMode::Flat)
        .sample_size(10)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time((first * 12).max(Duration::from_secs(1)))
        .throughput(Throughput::Elements(shape.entries()));
    group.bench_function(format!("real_fs_{}_files", shape.files), |b| {
        b.iter(|| setup.run());
    });
    group.finish();
    Ok(())
}

fn run() -> Result<(), String> {
    if let Some(root) = std::env::var_os(PROBE_VAR) {
        return probe(Path::new(&root));
    }
    let Some(raw) = std::env::var_os("SK_BENCH_DIR") else {
        println!(
            "usr bench skipped: set SK_BENCH_DIR to a folder for the persistent test tree \
             (not on drive C:), e.g. SK_BENCH_DIR='D:\\savekeeper-bench'"
        );
        return Ok(());
    };
    let base = bench_dir(&raw)?;
    // `cargo bench` passes `--bench`; without it the target runs in test mode
    // (`cargo test --benches`) and a small tree is used.
    let full_run = std::env::args().any(|a| a == "--bench");
    let files = match std::env::var("SK_BENCH_FILES") {
        Ok(v) => v
            .parse::<u64>()
            .map_err(|e| format!("SK_BENCH_FILES {v:?} is not a number: {e}"))?,
        Err(_) if full_run => FULL_FILES,
        Err(_) => SMOKE_FILES,
    };
    let mut c = Criterion::default().configure_from_args();
    bench_usr(&mut c, &base, files)?;
    c.final_summary();
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("usr bench: error: {e}");
        std::process::exit(1);
    }
}
