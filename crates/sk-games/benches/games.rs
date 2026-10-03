//! Acceptance benchmark of the games collector on the embedded Ludusavi
//! snapshot (SPEC-05 §8, NFR-05-01..03).
//!
//! Manual run only (SPEC-12 §2.2), never part of `cargo test`:
//!
//! ```text
//! cargo bench -p sk-games --bench games
//! SK_GAMES_BENCH_REAL=1 cargo bench -p sk-games --bench games   # also this machine
//! ```
//!
//! Measured, offline (no network is used):
//! - **store** — [`ManifestStore::load`] without a cache YAML, i.e. on the
//!   embedded snapshot (`third_party/ludusavi/manifest.yaml`): *cold* (the
//!   index file is overwritten with an empty one first, so the snapshot is
//!   unpacked, parsed and the index written: NFR-05-01) and *warm* (from the
//!   postcard index `cache/ludusavi-index.bin`). The data folder is
//!   `<target>/tmp/sk-games-bench/savekeeper-data` (the index is ≈ 6 MB),
//!   overwritten on every run and never removed;
//! - **collector** — [`GamesCollector::collect`] (manifest load from the index
//!   included) with 200 installed Steam games on an in-memory file system:
//!   NFR-05-02;
//! - **memory** (Windows) — a child process of this benchmark loads the
//!   manifest from the index and runs the collector; its working set and
//!   private bytes are read with `Get-Process` before, after the load and
//!   after the collector, plus the peaks: NFR-05-03. A heap estimate of the
//!   parsed manifest is printed on every OS.
//!
//! With `SK_GAMES_BENCH_REAL=1` the collector also runs on this machine
//! (`Environment::detect`, launchers by `enrich`, `RealFs`, the system
//! registry), twice, with the file-system calls of each run and its slowest
//! include probes (no criterion samples: a run can be long). That part only
//! reads; the second run is the figure with a warm file-system cache.
//!
//! Each figure is first measured once and printed with the NFR verdict, then
//! handed over to criterion.

#![allow(clippy::expect_used, clippy::unwrap_used)] // benchmark binary: fail loudly

#[path = "games/memory.rs"]
mod memory;
#[path = "games/timed_fs.rs"]
mod timed_fs;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use criterion::{BatchSize, Criterion, SamplingMode};
use sk_core::collector::{CollectContext, CollectOutput, Collector};
use sk_core::config::Config;
use sk_core::env::{Environment, InstalledGame, KnownFolder, LauncherInfo, StoreUser};
use sk_core::fs::FsScanner;
use sk_core::registry::MemRegistry;
use sk_core::CancellationToken;
use sk_games::{GamesCollector, Manifest, ManifestSource, ManifestStore};
use sk_scan::{MemFs, RealFs};
use timed_fs::TimedFs;
use tokio::runtime::Runtime;
use tokio::sync::mpsc::unbounded_channel;

/// NFR-05-01: parsing the full manifest.
const NFR_PARSE: Duration = Duration::from_secs(3);
/// NFR-05-02: the whole collector with 200 installed games.
const NFR_COLLECT: Duration = Duration::from_secs(5);
/// NFR-05-02: installed games in the benchmark.
const GAMES: usize = 200;
/// Environment variable that turns this binary into the memory probe.
const CHILD_VAR: &str = "SK_GAMES_BENCH_MEMORY_CHILD";

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

/// `savekeeper-data` of the benchmark in Cargo's per-target temp dir.
fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("sk-games-bench")
        .join("savekeeper-data")
}

fn offline_config() -> Config {
    let mut config = Config::default();
    config.games.auto_update = false;
    config
}

fn store(data: &Path) -> ManifestStore {
    ManifestStore::new(&offline_config(), data)
}

fn load(rt: &Runtime, data: &Path) -> Arc<Manifest> {
    let store = store(data);
    let manifest = rt
        .block_on(store.load(false, &CancellationToken::new()))
        .expect("embedded snapshot loads");
    assert!(
        matches!(manifest.meta.source, ManifestSource::Embedded { .. }),
        "{:?}: a cache YAML in {} hides the snapshot",
        manifest.meta.source,
        data.display()
    );
    assert!(store.take_issues().is_empty(), "store issues");
    manifest
}

/// Makes the next load rebuild the index from the snapshot (a damaged index
/// is ignored, SPEC-05 §4.1). The file is overwritten, never removed.
fn spoil_index(data: &Path) {
    let cache = data.join("cache");
    std::fs::create_dir_all(&cache).expect("create cache dir");
    std::fs::write(cache.join("ludusavi-index.bin"), b"").expect("overwrite index");
}

/// NFR-05-01: cold and warm loads of the embedded snapshot.
fn bench_store(c: &mut Criterion, rt: &Runtime, data: &Path) {
    spoil_index(data);
    let start = Instant::now();
    let manifest = load(rt, data);
    let cold = start.elapsed();
    let index = std::fs::metadata(data.join("cache").join("ludusavi-index.bin"))
        .map(|m| m.len())
        .unwrap_or(0);
    println!(
        "store cold (unpack snapshot + parse + write index): {} games in {:.3} s; \
         NFR-05-01 (<= {} s): {}",
        manifest.meta.games,
        cold.as_secs_f64(),
        NFR_PARSE.as_secs(),
        verdict(cold, NFR_PARSE)
    );
    println!(
        "heap estimate of the parsed manifest: {:.1} MB (index file {:.1} MB)",
        memory::mb(memory::heap_estimate(&manifest)),
        memory::mb(index)
    );
    drop(manifest);

    let start = Instant::now();
    let manifest = load(rt, data);
    let warm = start.elapsed();
    println!(
        "store warm (postcard index): {} games in {:.1} ms",
        manifest.meta.games,
        ms(warm)
    );
    drop(manifest);

    let mut group = c.benchmark_group("store_load");
    group
        .sampling_mode(SamplingMode::Flat)
        .sample_size(10)
        .measurement_time((cold * 12).max(Duration::from_secs(2)));
    group.bench_function("cold", |b| {
        b.iter_batched(
            || spoil_index(data),
            |()| load(rt, data),
            BatchSize::PerIteration,
        )
    });
    group.bench_function("warm", |b| b.iter(|| load(rt, data)));
    group.finish();
}

/// Every `len / GAMES`-th non-alias entry, as an installed game: (key, steam id).
fn picked_games(manifest: &Manifest) -> Vec<(String, Option<u32>)> {
    let mut keys: Vec<&String> = manifest
        .games
        .iter()
        .filter(|(_, g)| !g.is_alias())
        .map(|(k, _)| k)
        .collect();
    keys.sort_unstable();
    keys.iter()
        .step_by((keys.len() / GAMES).max(1))
        .take(GAMES)
        .map(|k| ((*k).clone(), manifest.games[*k].steam.map(|s| s.id)))
        .collect()
}

/// A fake profile with Steam (two accounts) and `games` installed in its
/// main library; the file system holds only the Steam and game folders.
fn memfs_profile(games: &[(String, Option<u32>)]) -> (MemFs, Environment) {
    let root = PathBuf::from(if cfg!(windows) { r"C:\fake" } else { "/fake" });
    let mut env = Environment::fake(&root);
    let steam = env.known_folders[&KnownFolder::ProgramFilesX86].join("Steam");
    let mut fs = MemFs::new();
    fs.add_dir(&steam.to_string_lossy());
    let installed = games
        .iter()
        .enumerate()
        .map(|(i, (key, steam_id))| {
            let install_dir = steam
                .join("steamapps")
                .join("common")
                .join(format!("Game{i}"));
            fs.add_dir(&install_dir.to_string_lossy());
            InstalledGame {
                store_game_id: steam_id.map_or_else(|| format!("9{i}"), |id| id.to_string()),
                name: key.clone(),
                install_dir,
                size_bytes: Some(1 << 30),
                manifest_key: None,
            }
        })
        .collect();
    let user = |id: &str, name: &str| StoreUser {
        id: id.to_owned(),
        alt_id: None,
        name: Some(name.to_owned()),
    };
    env.launchers.push(LauncherInfo {
        id: "steam".to_owned(),
        root: Some(steam),
        user_ids: vec![user("1", "A"), user("2", "B")],
        games: installed,
    });
    (fs, env)
}

/// A collector with its context, reused by every iteration.
struct Run {
    collector: GamesCollector,
    ctx: CollectContext,
}

impl Run {
    fn new(
        data: &Path,
        env: Environment,
        fs: Arc<dyn FsScanner>,
        registry: Option<MemRegistry>,
    ) -> Self {
        // Progress events are dropped: the receiver is closed.
        let (events, _) = unbounded_channel();
        let ctx = CollectContext {
            env: Arc::new(env),
            config: Arc::new(offline_config()),
            scanner: fs,
            events,
            cancel: CancellationToken::new(),
        };
        let mut collector = GamesCollector::new(store(data)).with_network(false);
        if let Some(registry) = registry {
            collector = collector.with_registry(Arc::new(registry));
        }
        Self { collector, ctx }
    }

    fn once(&self, rt: &Runtime) -> CollectOutput {
        rt.block_on(self.collector.collect(&self.ctx))
            .expect("games collector failed")
    }
}

/// One timed run.
fn timed_run(rt: &Runtime, run: &Run) -> (CollectOutput, Duration) {
    let start = Instant::now();
    let out = run.once(rt);
    (out, start.elapsed())
}

/// Prints a first and a second (warm) run with the NFR-05-02 verdict.
fn print_runs(label: &str, games: usize, out: &CollectOutput, first: Duration, warm: Duration) {
    println!(
        "{label}: {games} installed games -> {} findings, {} claimed paths, {} issues; \
         first run {:.1} ms, second run {:.1} ms (manifest load included); \
         NFR-05-02 (<= {} s): {}",
        out.findings.len(),
        out.claimed_paths.len(),
        out.issues.len(),
        ms(first),
        ms(warm),
        NFR_COLLECT.as_secs(),
        verdict(warm, NFR_COLLECT)
    );
}

/// Times one run (and a second, warm one), prints it with the NFR-05-02
/// verdict, then benchmarks it.
fn bench_run(c: &mut Criterion, rt: &Runtime, label: &str, games: usize, run: &Run) {
    let (_, first) = timed_run(rt, run);
    let (out, warm) = timed_run(rt, run);
    print_runs(label, games, &out, first, warm);
    let mut group = c.benchmark_group("games_collect");
    group
        .sampling_mode(SamplingMode::Flat)
        .sample_size(10)
        .measurement_time((warm * 12).clamp(Duration::from_secs(2), Duration::from_secs(30)));
    group.bench_function(label, |b| b.iter(|| run.once(rt)));
    group.finish();
}

/// NFR-05-02 on the fake profile.
fn bench_memfs(c: &mut Criterion, rt: &Runtime, data: &Path) {
    let games = picked_games(&load(rt, data));
    let (fs, env) = memfs_profile(&games);
    let run = Run::new(data, env, Arc::new(fs), Some(MemRegistry::new()));
    bench_run(c, rt, "memfs_200", games.len(), &run);
}

/// The collector on this machine (read only): two timed runs with the file
/// system calls of each, without criterion samples (a run can be long).
fn real(rt: &Runtime, data: &Path) {
    let mut env = Environment::detect().expect("detect environment");
    let home = env.known_folders[&KnownFolder::Home].clone();
    let fs = Arc::new(TimedFs::new(RealFs::new(&env)));
    let start = Instant::now();
    let issues = sk_games::enrich(&mut env, fs.as_ref());
    let launchers: Vec<String> = env
        .launchers
        .iter()
        .map(|l| format!("{} ({} games)", l.id, l.games.len()))
        .collect();
    let games: usize = env.launchers.iter().map(|l| l.games.len()).sum();
    println!(
        "real: launchers detected in {:.1} ms: [{}], {} launcher issues",
        ms(start.elapsed()),
        launchers.join(", "),
        issues.len()
    );
    fs.report(&home);
    let run = Run::new(data, env, fs.clone(), None);
    let (_, first) = timed_run(rt, &run);
    println!("real: first run {:.1} ms", ms(first));
    fs.report(&home);
    let (out, warm) = timed_run(rt, &run);
    println!("real: second run {:.1} ms", ms(warm));
    fs.report(&home);
    print_runs("real_machine", games, &out, first, warm);
}

fn main() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("tokio runtime");
    let data = data_dir();
    if std::env::var_os(CHILD_VAR).is_some() {
        // Memory probe: only load and collect, then report (no criterion).
        memory::child(
            || load(&rt, &data),
            |games| {
                let (fs, env) = memfs_profile(games);
                Run::new(&data, env, Arc::new(fs), Some(MemRegistry::new())).once(&rt)
            },
            picked_games,
        );
        return;
    }

    let mut c = Criterion::default().configure_from_args();
    bench_store(&mut c, &rt, &data);
    // The index is valid now: the probe measures a load from it.
    memory::parent(CHILD_VAR);
    bench_memfs(&mut c, &rt, &data);
    if std::env::var_os("SK_GAMES_BENCH_REAL").is_some_and(|v| v == "1") {
        real(&rt, &data);
    } else {
        println!(
            "real machine run skipped: set SK_GAMES_BENCH_REAL=1 to run the collector on this machine"
        );
    }
    c.final_summary();
}
