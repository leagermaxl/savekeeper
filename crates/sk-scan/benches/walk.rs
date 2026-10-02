//! Benchmark of `RealFs::walk` on a generated tree (SPEC-03 §6, NFR-03-01).
//!
//! Manual run only (SPEC-12 §2.2), never part of `cargo test`:
//!
//! ```text
//! SK_BENCH_DIR='D:\savekeeper-bench' cargo bench -p sk-scan --bench walk
//! ```
//!
//! Environment variables:
//! - `SK_BENCH_DIR` — folder for the tree, **required**: without it the
//!   benchmark prints a message and does nothing. On Windows a folder on
//!   drive `C:` is refused before anything is created;
//! - `SK_BENCH_FILES` — number of files in the tree (default 1 000 000 under
//!   `cargo bench`, 1 000 when the target is run in test mode).
//!
//! The tree is persistent: it is generated once in `SK_BENCH_DIR\walk-<N>`
//! and reused by later runs. The marker `SK_BENCH_DIR\walk-<N>.marker`
//! (next to the tree, so walks do not count it) describes the tree's shape
//! and is written last, so an interrupted generation is reported instead of
//! being reused. The benchmark never deletes or overwrites anything; a folder
//! without a matching marker is an error that the user resolves by hand.
//! Files are empty, so the tree costs only file system metadata.
//!
//! Before handing over to criterion, one timed walk is reported together with
//! the NFR-03-01 verdict (≥ 20 000 entries/s), the drive and the number of
//! walk threads.

#![allow(clippy::expect_used, clippy::unwrap_used)] // benchmark binary: fail loudly

use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
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
/// Version of the generator; bump it when the layout of the tree changes.
const GENERATOR_VERSION: u32 = 1;

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

    /// Contents of the marker file that describes this tree.
    fn marker(&self) -> String {
        format!(
            "savekeeper sk-scan walk bench tree\ngenerator={GENERATOR_VERSION}\nfiles={}\n\
             files_per_dir={FILES_PER_DIR}\nfan_out={FAN_OUT}\nlevels={}\n",
            self.files, self.levels
        )
    }
}

/// Creates a new file; fails if it already exists (never overwrites).
fn create_new(path: &Path) -> io::Result<fs::File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

/// Creates the tree below the empty folder `root` in parallel; files are empty.
fn generate(shape: &TreeShape, root: &Path) -> io::Result<()> {
    (0..shape.leaves).into_par_iter().try_for_each(|k| {
        let dir = shape.leaf_path(root, k);
        fs::create_dir_all(&dir)?;
        for f in 0..shape.files_in(k) {
            create_new(&dir.join(format!("f{f:03}.dat")))?;
        }
        Ok(())
    })
}

/// The tree of `shape` in `base`: reused when its marker matches, generated
/// when neither the tree nor its marker exists. Returns the tree's root and
/// the generation time (`None` when reused). Never deletes or overwrites.
fn prepare_tree(base: &Path, shape: &TreeShape) -> Result<(PathBuf, Option<Duration>), String> {
    let root = base.join(format!("walk-{}", shape.files));
    let marker = base.join(format!("walk-{}.marker", shape.files));
    let fix = format!(
        "nothing was changed; delete {} and {} yourself to regenerate the tree, \
         or set SK_BENCH_DIR to another folder",
        root.display(),
        marker.display()
    );
    let root_exists = exists(&root)?;
    let marker_text = match fs::read_to_string(&marker) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("cannot read {}: {e}", marker.display())),
    };
    match (root_exists, marker_text) {
        (true, Some(text)) if text == shape.marker() => {
            if root.is_dir() {
                Ok((root, None))
            } else {
                Err(format!("{} is not a folder; {fix}", root.display()))
            }
        }
        (true, Some(_)) => Err(format!(
            "{} describes a different tree than requested; {fix}",
            marker.display()
        )),
        (true, None) => Err(format!(
            "{} exists but has no marker {} (an interrupted generation or a foreign \
             folder); {fix}",
            root.display(),
            marker.display()
        )),
        (false, Some(_)) => Err(format!(
            "marker {} exists without its tree {}; {fix}",
            marker.display(),
            root.display()
        )),
        (false, None) => {
            let start = Instant::now();
            let io_err = |what: &str, e: io::Error| format!("{what}: {e}");
            fs::create_dir_all(base).map_err(|e| io_err("cannot create SK_BENCH_DIR", e))?;
            fs::create_dir(&root).map_err(|e| io_err("cannot create the tree folder", e))?;
            generate(shape, &root).map_err(|e| io_err("tree generation failed", e))?;
            create_new(&marker)
                .and_then(|mut f| f.write_all(shape.marker().as_bytes()))
                .map_err(|e| io_err("cannot write the marker", e))?;
            Ok((root, Some(start.elapsed())))
        }
    }
}

/// Whether anything (also a link) exists at `path`.
fn exists(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("cannot inspect {}: {e}", path.display())),
    }
}

/// Uppercase drive letter of `C:\...`, `\\?\C:\...` or `\\.\C:\...`.
fn drive_letter(path: &Path) -> Option<char> {
    match path.components().next()? {
        Component::Prefix(p) => match p.kind() {
            Prefix::Disk(d) | Prefix::VerbatimDisk(d) => Some(char::from(d).to_ascii_uppercase()),
            Prefix::DeviceNS(name) => {
                let name = name.to_str()?;
                let mut chars = name.chars();
                match (chars.next(), chars.next(), chars.next()) {
                    (Some(d), Some(':'), None) if d.is_ascii_alphabetic() => {
                        Some(d.to_ascii_uppercase())
                    }
                    _ => None,
                }
            }
            _ => None,
        },
        _ => None,
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
/// tree itself (e.g. below `%LOCALAPPDATA%\Temp`), as `measure` does for an
/// explicit root (SPEC-03 §4.5).
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

fn bench_walk(c: &mut Criterion, base: &Path, files: u64) -> Result<(), String> {
    let shape = TreeShape::new(files);
    let (root, generated) = prepare_tree(base, &shape)?;
    let origin = match generated {
        Some(t) => format!("generated in {:.1} s", t.as_secs_f64()),
        None => "reused".to_owned(),
    };
    println!(
        "tree: {} files, {} folders, {} entries in {} ({origin})",
        shape.files,
        shape.dirs(),
        shape.entries(),
        root.display(),
    );

    let env = Environment::detect().map_err(|e| format!("cannot detect the environment: {e}"))?;
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
    Ok(())
}

fn run() -> Result<(), String> {
    let Some(raw) = std::env::var_os("SK_BENCH_DIR") else {
        println!(
            "walk bench skipped: set SK_BENCH_DIR to a folder for the persistent test tree \
             (not on drive C:), e.g. SK_BENCH_DIR='D:\\savekeeper-bench'"
        );
        return Ok(());
    };
    let base = bench_dir(&raw)?;
    // `cargo bench` passes `--bench`; without it the target runs in test mode
    // (`cargo test --benches`) and a small tree is walked.
    let full_run = std::env::args().any(|a| a == "--bench");
    let files = match std::env::var("SK_BENCH_FILES") {
        Ok(v) => v
            .parse::<u64>()
            .map_err(|e| format!("SK_BENCH_FILES {v:?} is not a number: {e}"))?,
        Err(_) if full_run => FULL_FILES,
        Err(_) => SMOKE_FILES,
    };
    let mut c = Criterion::default().configure_from_args();
    bench_walk(&mut c, &base, files)?;
    c.final_summary();
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("walk bench: error: {e}");
        std::process::exit(1);
    }
}
