//! Benchmark of `RealFs::walk` on a generated tree (SPEC-03 §6, NFR-03-01).
//!
//! Manual run only (SPEC-12 §2.2), never part of `cargo test`:
//!
//! ```text
//! cargo bench -p sk-scan --bench walk
//! ```
//!
//! Environment variables:
//! - `SK_BENCH_FILES` — number of files in the tree (default 1 000 000 under
//!   `cargo bench`, 1 000 when the target is run in test mode);
//! - `SK_BENCH_DIR` — folder in which the temporary tree is created (default:
//!   the system temp folder). Use it to pick the drive being measured.
//!
//! The tree is created in a fresh temp dir and removed afterwards, also when
//! the benchmark panics (the `TempDir` guard). Files are empty, so the tree
//! costs only file system metadata. Before handing over to criterion, one
//! timed walk is reported together with the NFR-03-01 verdict (≥ 20 000
//! entries/s), the drive and the number of walk threads.

#![allow(clippy::expect_used, clippy::unwrap_used)] // benchmark binary: fail loudly

use std::ffi::OsStr;
use std::fs::{self, File};
use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::Arc;
use std::time::{Duration, Instant};

use criterion::{Criterion, SamplingMode, Throughput};
use rayon::prelude::*;
use sk_core::env::{DriveKind, DriveMedia, Environment};
use sk_scan::{
    CancellationToken, ExcludeSet, Exclusion, FsScanner, PathFilter, RealFs, WalkControl,
    WalkOptions,
};
use tempfile::TempDir;

/// NFR-03-01: minimal walk speed on an SSD, entries per second.
const NFR_ENTRIES_PER_SEC: f64 = 20_000.0;
/// Files in the tree under `cargo bench` (SPEC-03 §6).
const FULL_FILES: u64 = 1_000_000;
/// Files in the tree when the target runs in test mode (`cargo test --benches`).
const SMOKE_FILES: u64 = 1_000;
/// Files in one leaf folder.
const FILES_PER_DIR: u64 = 100;
/// Subfolders of one inner folder.
const FAN_OUT: u64 = 10;

/// Shape of a generated tree: `levels` of folders with [`FAN_OUT`] children
/// each, files only in the `leaves` folders of the last level.
#[derive(Debug, Clone, Copy)]
struct TreeShape {
    files: u64,
    leaves: u64,
    levels: u32,
}

impl TreeShape {
    fn new(files: u64) -> Self {
        let leaves = files.div_ceil(FILES_PER_DIR).max(1);
        let mut levels = 1;
        while FAN_OUT.pow(levels) < leaves {
            levels += 1;
        }
        Self {
            files,
            leaves,
            levels,
        }
    }

    /// Folders at all levels: level `l` has one folder per distinct prefix
    /// of the leaf numbers.
    fn dirs(&self) -> u64 {
        (1..=self.levels)
            .map(|l| self.leaves.div_ceil(FAN_OUT.pow(self.levels - l)))
            .sum()
    }

    /// Entries a full walk visits: every folder and every file.
    fn entries(&self) -> u64 {
        self.files + self.dirs()
    }

    /// Path of leaf `k` below `root`: its digits in base [`FAN_OUT`], one
    /// folder per level (`d0\d3\d7`).
    fn leaf_path(&self, root: &Path, k: u64) -> PathBuf {
        let mut path = root.to_path_buf();
        for l in (0..self.levels).rev() {
            path.push(format!("d{}", (k / FAN_OUT.pow(l)) % FAN_OUT));
        }
        path
    }

    /// Files of leaf `k`: [`FILES_PER_DIR`], the last leaf takes the rest.
    fn files_in(&self, k: u64) -> u64 {
        FILES_PER_DIR.min(self.files - k * FILES_PER_DIR)
    }
}

/// Creates the tree below `root` in parallel; files are empty.
fn generate(shape: &TreeShape, root: &Path) -> std::io::Result<()> {
    (0..shape.leaves).into_par_iter().try_for_each(|k| {
        let dir = shape.leaf_path(root, k);
        fs::create_dir_all(&dir)?;
        for f in 0..shape.files_in(k) {
            File::create(dir.join(format!("f{f:03}.dat")))?;
        }
        Ok(())
    })
}

/// Removes the leaves in parallel, then the temp dir itself.
fn remove(shape: &TreeShape, tmp: TempDir) -> std::io::Result<()> {
    let root = tmp.path().to_path_buf();
    (0..shape.leaves)
        .into_par_iter()
        .try_for_each(|k| fs::remove_dir_all(shape.leaf_path(&root, k)))?;
    tmp.close()
}

/// Uppercase drive letter of `path`.
fn drive_letter(path: &Path) -> Option<char> {
    match path.components().next()? {
        Component::Prefix(p) => match p.kind() {
            Prefix::Disk(d) | Prefix::VerbatimDisk(d) => Some(char::from(d).to_ascii_uppercase()),
            _ => None,
        },
        _ => None,
    }
}

/// The drive of `root` and the walk threads NFR-03-03 gives it.
fn drive_report(env: &Environment, root: &Path) -> (String, usize) {
    let cpus = std::thread::available_parallelism().map_or(1, usize::from);
    let drive = drive_letter(root).and_then(|l| env.drives.iter().find(|d| d.letter == l));
    let threads = match drive {
        Some(d) if d.kind == DriveKind::Network || d.media == DriveMedia::Hdd => 2,
        _ => cpus.min(8),
    };
    let name = drive.map_or_else(
        || "unknown drive (counted as SSD)".to_owned(),
        |d| {
            format!(
                "{}: {:?} {:?} {}",
                d.letter,
                d.kind,
                d.media,
                d.fs.as_deref().unwrap_or("?")
            )
        },
    );
    (format!("{name}, {cpus} logical CPUs"), threads)
}

/// The built-in exclusions; without full-path ones when they cover the
/// tree itself (it lives in `%LOCALAPPDATA%\Temp` by default), as `measure`
/// does for an explicit root (SPEC-03 §4.5).
fn excludes(env: &Environment, root: &Path) -> Arc<dyn PathFilter> {
    let full = ExcludeSet::builtin(env);
    let probe = root.join("d0");
    if full.check(&probe, OsStr::new("d0"), true) == Exclusion::Exclude {
        Arc::new(full.names_only())
    } else {
        Arc::new(full)
    }
}

/// One full walk; panics if it does not visit exactly `expected` entries.
fn walk_once(fs: &RealFs, root: &Path, opts: &WalkOptions, expected: u64) -> Duration {
    let cancel = CancellationToken::new();
    let start = Instant::now();
    let stats = fs
        .walk(root, opts, &mut |_| WalkControl::Continue, &cancel)
        .expect("walk of the generated tree");
    let elapsed = start.elapsed();
    assert_eq!(stats.entries, expected, "walk stats: {stats:?}");
    assert!(
        !stats.truncated && stats.errors == 0,
        "walk stats: {stats:?}"
    );
    elapsed
}

fn bench_walk(c: &mut Criterion, full_run: bool) {
    let files = std::env::var("SK_BENCH_FILES")
        .ok()
        .map(|v| v.parse::<u64>().expect("SK_BENCH_FILES is a number"))
        .unwrap_or(if full_run { FULL_FILES } else { SMOKE_FILES });
    let shape = TreeShape::new(files);
    let mut builder = tempfile::Builder::new();
    builder.prefix("sk-bench-walk-");
    let tmp = match std::env::var_os("SK_BENCH_DIR") {
        Some(base) => builder.tempdir_in(base),
        None => builder.tempdir(),
    }
    .expect("temp dir for the tree");
    let root = tmp.path().to_path_buf();

    let start = Instant::now();
    generate(&shape, &root).expect("generate the tree");
    println!(
        "tree: {} files, {} folders, {} entries in {} (generated in {:.1} s)",
        shape.files,
        shape.dirs(),
        shape.entries(),
        root.display(),
        start.elapsed().as_secs_f64()
    );

    let env = Environment::detect().expect("detect the environment");
    let (drive, threads) = drive_report(&env, &root);
    let fs = RealFs::new(&env);
    let opts = WalkOptions {
        max_depth: 32,
        max_entries: 2_000_000,
        follow_links: false,
        excludes: excludes(&env, &root),
        include: None,
        exclude: None,
        threads: 0, // the drive limit (NFR-03-03)
    };

    let first = walk_once(&fs, &root, &opts, shape.entries());
    let rate = shape.entries() as f64 / first.as_secs_f64();
    println!(
        "first walk: {} entries in {:.3} s = {:.0} entries/s; {threads} threads; {drive}; \
         NFR-03-01 (>= {NFR_ENTRIES_PER_SEC:.0}/s): {}",
        shape.entries(),
        first.as_secs_f64(),
        rate,
        if rate >= NFR_ENTRIES_PER_SEC {
            "PASS"
        } else {
            "FAIL"
        }
    );

    let mut group = c.benchmark_group("walk");
    group
        .sampling_mode(SamplingMode::Flat)
        .sample_size(10)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time((first * 12).max(Duration::from_secs(1)))
        .throughput(Throughput::Elements(shape.entries()));
    group.bench_function(format!("real_fs_{}_files", shape.files), |b| {
        b.iter(|| walk_once(&fs, &root, &opts, shape.entries()));
    });
    group.finish();

    let start = Instant::now();
    remove(&shape, tmp).expect("remove the tree");
    println!("tree removed in {:.1} s", start.elapsed().as_secs_f64());
}

fn main() {
    // `cargo bench` passes `--bench`; without it the target runs in test mode
    // (`cargo test --benches`) and only a small tree is walked.
    let full_run = std::env::args().any(|a| a == "--bench");
    let mut c = Criterion::default().configure_from_args();
    bench_walk(&mut c, full_run);
    c.final_summary();
}
