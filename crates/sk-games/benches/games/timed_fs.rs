//! A file-system wrapper of the games benchmark that counts the calls of the
//! collector and keeps its slowest walks (include probes, SPEC-05 §4.7
//! step 3), to show where the time of a real-machine run goes.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sk_core::fs::{
    DirEntryInfo, EntryMeta, FsError, FsScanner, Readability, WalkControl, WalkOptions, WalkStats,
};
use sk_core::CancellationToken;

/// Slowest walks kept for the report.
const SLOWEST: usize = 10;

/// Time and count of one kind of call.
#[derive(Debug, Default, Clone, Copy)]
struct Tally {
    calls: u64,
    time: Duration,
}

impl Tally {
    fn add(&mut self, time: Duration) {
        self.calls += 1;
        self.time += time;
    }
}

/// One finished walk.
#[derive(Debug, Clone)]
struct Walk {
    root: PathBuf,
    time: Duration,
    entries: u64,
    truncated: bool,
}

#[derive(Debug, Default)]
struct Log {
    metadata: Tally,
    exists: Tally,
    read_dir: Tally,
    walk: Tally,
    walk_entries: u64,
    reads: Tally,
    slowest: Vec<Walk>,
}

/// `inner` with call statistics.
pub struct TimedFs<F> {
    inner: F,
    log: Mutex<Log>,
}

impl<F: FsScanner> TimedFs<F> {
    pub fn new(inner: F) -> Self {
        Self {
            inner,
            log: Mutex::new(Log::default()),
        }
    }

    fn with_log(&self, f: impl FnOnce(&mut Log)) {
        if let Ok(mut log) = self.log.lock() {
            f(&mut log);
        }
    }

    /// Prints the statistics and resets them; `home` is shown as `{HOME}`.
    pub fn report(&self, home: &Path) {
        let Ok(mut log) = self.log.lock() else {
            return;
        };
        let line = |name: &str, t: Tally| {
            println!(
                "    {name}: {} calls, {:.1} ms",
                t.calls,
                t.time.as_secs_f64() * 1000.0
            );
        };
        println!("  file system calls:");
        line("metadata", log.metadata);
        line("exists", log.exists);
        line("read_dir", log.read_dir);
        line("walk", log.walk);
        println!("    walk entries visited: {}", log.walk_entries);
        line("reads", log.reads);
        println!("  slowest walks:");
        for w in &log.slowest {
            let root = w.root.strip_prefix(home).map_or_else(
                |_| w.root.display().to_string(),
                |rest| format!("{{HOME}}\\{}", rest.display()),
            );
            println!(
                "    {:.1} ms, {} entries{}: {root}",
                w.time.as_secs_f64() * 1000.0,
                w.entries,
                if w.truncated { " (truncated)" } else { "" }
            );
        }
        *log = Log::default();
    }
}

fn timed<T>(f: impl FnOnce() -> T) -> (T, Duration) {
    let start = Instant::now();
    let value = f();
    (value, start.elapsed())
}

impl<F: FsScanner> FsScanner for TimedFs<F> {
    fn metadata(&self, path: &Path) -> Result<EntryMeta, FsError> {
        let (r, t) = timed(|| self.inner.metadata(path));
        self.with_log(|l| l.metadata.add(t));
        r
    }

    fn exists(&self, path: &Path) -> bool {
        let (r, t) = timed(|| self.inner.exists(path));
        self.with_log(|l| l.exists.add(t));
        r
    }

    fn read_dir(&self, path: &Path) -> Result<Vec<DirEntryInfo>, FsError> {
        let (r, t) = timed(|| self.inner.read_dir(path));
        self.with_log(|l| l.read_dir.add(t));
        r
    }

    fn walk(
        &self,
        root: &Path,
        opts: &WalkOptions,
        visit: &mut dyn FnMut(&DirEntryInfo) -> WalkControl,
        cancel: &CancellationToken,
    ) -> Result<WalkStats, FsError> {
        let (r, t) = timed(|| self.inner.walk(root, opts, visit, cancel));
        let (entries, truncated) = r.as_ref().map_or((0, false), |s| (s.entries, s.truncated));
        self.with_log(|l| {
            l.walk.add(t);
            l.walk_entries += entries;
            l.slowest.push(Walk {
                root: root.to_path_buf(),
                time: t,
                entries,
                truncated,
            });
            l.slowest.sort_by_key(|w| std::cmp::Reverse(w.time));
            l.slowest.truncate(SLOWEST);
        });
        r
    }

    fn read_head(&self, path: &Path, max: usize) -> Result<Vec<u8>, FsError> {
        let (r, t) = timed(|| self.inner.read_head(path, max));
        self.with_log(|l| l.reads.add(t));
        r
    }

    fn read_small(&self, path: &Path, max: usize) -> Result<Vec<u8>, FsError> {
        let (r, t) = timed(|| self.inner.read_small(path, max));
        self.with_log(|l| l.reads.add(t));
        r
    }

    fn probe_readable(&self, path: &Path) -> Readability {
        let (r, t) = timed(|| self.inner.probe_readable(path));
        self.with_log(|l| l.reads.add(t));
        r
    }
}
