//! Benchmark of parsing the full Ludusavi manifest (SPEC-05 §6, NFR-05-01: ≤ 3 s).
//!
//! Manual run only (SPEC-12 §2.2), never part of `cargo test`:
//!
//! ```text
//! SK_LUDUSAVI_MANIFEST='D:\ludusavi\manifest.yaml' cargo bench -p sk-games --bench manifest
//! ```
//!
//! Two inputs are measured:
//! - **real** — the file named by `SK_LUDUSAVI_MANIFEST` (`data/manifest.yaml`
//!   of github.com/mtkennerly/ludusavi-manifest, downloaded by hand). Without
//!   the variable this part prints a message and is skipped. The file is only
//!   read;
//! - **synthetic** — a manifest generated in memory (`tests/common/synthetic.rs`),
//!   `SK_SYNTHETIC_MB` megabytes (default 40 under `cargo bench`, 1 when the
//!   target runs in test mode). Nothing is written to disk.
//!
//! Each input is first parsed once and reported with the NFR-05-01 verdict,
//! then handed over to criterion.

#![allow(clippy::expect_used, clippy::unwrap_used)] // benchmark binary: fail loudly

#[path = "../tests/common/synthetic.rs"]
mod synthetic;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use criterion::{Criterion, SamplingMode, Throughput};
use sk_games::{Manifest, ManifestSource};

/// NFR-05-01: maximal time to parse the full manifest.
const NFR_PARSE: Duration = Duration::from_secs(3);
/// Synthetic manifest size under `cargo bench`, MB (the real one is ~40 MB, NFR-05-01).
const FULL_SYNTHETIC_MB: usize = 40;
/// Synthetic manifest size when the target runs in test mode, MB.
const SMOKE_SYNTHETIC_MB: usize = 1;

fn parse_once(yaml: &[u8]) -> (Manifest, Duration) {
    let start = Instant::now();
    let manifest = Manifest::parse(yaml, ManifestSource::Cache).expect("manifest must parse");
    (manifest, start.elapsed())
}

/// Parses `yaml` once, prints the verdict, then benchmarks it with criterion.
fn bench_input(c: &mut Criterion, label: &str, yaml: &[u8]) {
    let (manifest, first) = parse_once(yaml);
    let files: usize = manifest.games.values().map(|g| g.files.len()).sum();
    let aliases = manifest.games.values().filter(|g| g.is_alias()).count();
    println!(
        "{label}: {:.1} MB, {} entries ({aliases} aliases, {files} file rules) parsed in {:.3} s; \
         NFR-05-01 (<= {} s): {}",
        yaml.len() as f64 / (1024.0 * 1024.0),
        manifest.meta.games,
        first.as_secs_f64(),
        NFR_PARSE.as_secs(),
        if first <= NFR_PARSE { "PASS" } else { "FAIL" }
    );
    drop(manifest);

    let mut group = c.benchmark_group("manifest_parse");
    group
        .sampling_mode(SamplingMode::Flat)
        .sample_size(10)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time((first * 12).max(Duration::from_secs(1)))
        .throughput(Throughput::Bytes(yaml.len() as u64));
    group.bench_function(label, |b| b.iter(|| parse_once(yaml)));
    group.finish();
}

fn run() -> Result<(), String> {
    // `cargo bench` passes `--bench`; without it the target runs in test mode
    // (`cargo test --benches`) on a small synthetic manifest.
    let full_run = std::env::args().any(|a| a == "--bench");
    let mut c = Criterion::default().configure_from_args();

    match std::env::var_os("SK_LUDUSAVI_MANIFEST") {
        Some(raw) => {
            let path = PathBuf::from(raw);
            let yaml =
                std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            bench_input(&mut c, "real", &yaml);
        }
        None => println!(
            "real manifest skipped: set SK_LUDUSAVI_MANIFEST to a downloaded \
             ludusavi-manifest data/manifest.yaml, e.g. \
             SK_LUDUSAVI_MANIFEST='D:\\ludusavi\\manifest.yaml'"
        ),
    }

    let mb = match std::env::var("SK_SYNTHETIC_MB") {
        Ok(v) => v
            .parse::<usize>()
            .map_err(|e| format!("SK_SYNTHETIC_MB {v:?} is not a number: {e}"))?,
        Err(_) if full_run => FULL_SYNTHETIC_MB,
        Err(_) => SMOKE_SYNTHETIC_MB,
    };
    if mb > 0 {
        let synthetic = synthetic::synthetic_manifest(mb << 20);
        bench_input(&mut c, "synthetic", synthetic.yaml.as_bytes());
    }

    c.final_summary();
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("manifest bench: error: {e}");
        std::process::exit(1);
    }
}
