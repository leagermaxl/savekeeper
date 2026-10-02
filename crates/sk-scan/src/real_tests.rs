//! Tests of `RealFs` (T-03-04). Everything happens in temp dirs.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use sk_core::env::{DriveKind, DriveMedia, Environment};

use super::*;
use crate::{CloudState, EntryKind, ExcludeSet, Exclusion, MemFs, PathFilter};

#[cfg(windows)]
#[path = "real_win_tests.rs"]
mod windows_only;

/// Excludes entries by name, ignoring case.
#[derive(Debug, Default)]
struct NameFilter(Vec<&'static str>);

impl PathFilter for NameFilter {
    fn check(&self, _: &Path, name: &OsStr, _: bool) -> Exclusion {
        let name = name.to_string_lossy();
        if self.0.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
            Exclusion::Exclude
        } else {
            Exclusion::Keep
        }
    }
}

fn opts() -> WalkOptions {
    WalkOptions {
        max_depth: 32,
        max_entries: 2_000_000,
        follow_links: false,
        excludes: Arc::new(NameFilter::default()),
        include: None,
        exclude: None,
        threads: 4,
    }
}

fn globs(patterns: &[&str]) -> GlobSet {
    let mut set = GlobSetBuilder::new();
    for p in patterns {
        set.add(
            GlobBuilder::new(p)
                .case_insensitive(true)
                .literal_separator(true)
                .build()
                .unwrap(),
        );
    }
    set.build().unwrap()
}

fn real() -> RealFs {
    RealFs::new(&Environment::fake(Path::new("/nonexistent")))
}

fn write(root: &Path, rel: &str, len: usize) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, vec![b'x'; len]).unwrap();
}

/// `a/x.txt, a/b/y.dat, a/b/c/z.dat, top.sav` — the tree of the `MemFs` tests.
fn sample() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a/x.txt", 3);
    write(dir.path(), "a/b/y.dat", 10);
    write(dir.path(), "a/b/c/z.dat", 20);
    write(dir.path(), "top.sav", 5);
    dir
}

/// The same tree in a `MemFs` under `/r`.
fn sample_mem() -> MemFs {
    let mut fs = MemFs::new();
    fs.add_file("/r/a/x.txt", 3, "", None)
        .add_file("/r/a/b/y.dat", 10, "", None)
        .add_file("/r/a/b/c/z.dat", 20, "", None)
        .add_file("/r/top.sav", 5, "", None);
    fs
}

/// Sorted relative paths (`/`-separated) visited by a walk, and its stats.
fn walk_with(
    fs: &dyn FsScanner,
    root: &Path,
    opts: &WalkOptions,
    mut control: impl FnMut(&DirEntryInfo) -> WalkControl,
) -> (Vec<String>, WalkStats) {
    let mut seen = Vec::new();
    let stats = fs
        .walk(
            root,
            opts,
            &mut |e| {
                let parts: Vec<_> = e.rel.iter().map(|c| c.to_string_lossy()).collect();
                seen.push(parts.join("/"));
                control(e)
            },
            &CancellationToken::new(),
        )
        .unwrap();
    seen.sort();
    (seen, stats)
}

#[test]
fn walk_visits_all_entries_with_metadata() {
    let dir = sample();
    let mut entries = Vec::new();
    let stats = real()
        .walk(
            dir.path(),
            &opts(),
            &mut |e| {
                entries.push(e.clone());
                WalkControl::Continue
            },
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(stats.entries, 7);
    assert_eq!((stats.errors, stats.skipped_excluded), (0, 0));
    assert!(!stats.truncated);
    let z = entries
        .iter()
        .find(|e| e.rel == Path::new("a").join("b").join("c").join("z.dat"))
        .unwrap();
    assert_eq!(z.depth, 4);
    assert_eq!(z.path, dir.path().join(&z.rel));
    assert_eq!(z.meta.kind, EntryKind::File);
    assert_eq!(z.meta.size, 20);
    assert_eq!(z.meta.cloud, CloudState::Local);
    assert!(z.meta.mtime.is_some());
    let a = entries.iter().find(|e| e.rel == Path::new("a")).unwrap();
    assert_eq!((a.depth, a.meta.kind), (1, EntryKind::Dir));
    // The root itself is not visited.
    assert!(entries.iter().all(|e| e.rel != Path::new("")));
}

/// `RealFs` and `MemFs` agree on SPEC-03 §4.1 «Поведение walk».
#[test]
fn walk_matches_mem_fs() {
    let dir = sample();
    let mem = sample_mem();
    let real = real();
    let variants: Vec<fn(&mut WalkOptions)> = vec![
        |_| {},
        |o| o.excludes = Arc::new(NameFilter(vec!["B"])),
        |o| o.include = Some(globs(&["**/*.DAT"])),
        |o| o.exclude = Some(globs(&["a/b"])),
        |o| o.max_depth = 2,
        |o| o.max_depth = 0,
        |o| o.max_entries = 7,
        |o| {
            o.include = Some(globs(&["*.sav"]));
            o.max_entries = 4;
        },
    ];
    for (i, change) in variants.iter().enumerate() {
        let mut o = opts();
        change(&mut o);
        let (real_seen, real_stats) = walk_with(&real, dir.path(), &o, |_| WalkControl::Continue);
        let (mem_seen, mem_stats) = walk_with(&mem, Path::new("/r"), &o, |_| WalkControl::Continue);
        assert_eq!(real_seen, mem_seen, "variant {i}");
        assert_eq!(real_stats, mem_stats, "variant {i}");
    }
}

#[test]
fn walk_skip_dir_and_stop() {
    let dir = sample();
    let (seen, _) = walk_with(&real(), dir.path(), &opts(), |e| {
        if e.rel == Path::new("a").join("b") {
            WalkControl::SkipDir
        } else {
            WalkControl::Continue
        }
    });
    assert_eq!(seen, ["a", "a/b", "a/x.txt", "top.sav"]);

    let (seen, stats) = walk_with(&real(), dir.path(), &opts(), |e| {
        if e.depth == 3 {
            WalkControl::Stop
        } else {
            WalkControl::Continue
        }
    });
    assert!(seen.contains(&"a/b/c".to_owned()));
    assert!(!seen.contains(&"a/b/c/z.dat".to_owned()));
    assert_eq!(stats.entries, seen.len() as u64);
    assert!(!stats.truncated);
}

#[test]
fn walk_limits_entries() {
    let dir = sample();
    let mut o = opts();
    o.max_entries = 3;
    let (seen, stats) = walk_with(&real(), dir.path(), &o, |_| WalkControl::Continue);
    assert_eq!(seen.len(), 3);
    assert_eq!(stats.entries, 3);
    assert!(stats.truncated);
}

#[test]
fn walk_is_cancelled() {
    let dir = sample();
    let real = real();
    let cancel = CancellationToken::new();
    let mut visited = 0;
    let result = real.walk(
        dir.path(),
        &opts(),
        &mut |_| {
            visited += 1;
            cancel.cancel();
            WalkControl::Continue
        },
        &cancel,
    );
    assert!(matches!(result, Err(FsError::Cancelled)));
    assert_eq!(visited, 1);
    let again = real.walk(dir.path(), &opts(), &mut |_| WalkControl::Continue, &cancel);
    assert!(matches!(again, Err(FsError::Cancelled)));
}

#[test]
fn walk_root_errors() {
    let dir = sample();
    let mut visit = |_: &DirEntryInfo| WalkControl::Continue;
    let cancel = CancellationToken::new();
    assert!(matches!(
        real().walk(&dir.path().join("none"), &opts(), &mut visit, &cancel),
        Err(FsError::NotFound)
    ));
    assert!(matches!(
        real().walk(&dir.path().join("top.sav"), &opts(), &mut visit, &cancel),
        Err(FsError::Io(_))
    ));
}

/// Built-in exclusions, conditional ones included (SPEC-03 §4.2 step 3, §4.5).
#[test]
fn walk_applies_builtin_and_conditional_exclusions() {
    let dir = tempfile::tempdir().unwrap();
    let env = Environment::fake(&dir.path().join("profile"));
    let tree = dir.path().join("tree");
    for rel in [
        "rust/Cargo.toml",
        "rust/target/debug/app.exe",
        "rust/src/main.rs",
        "plain/target/keep.txt",
        "py/venv/pyvenv.cfg",
        "py/venv/lib/site.py",
        "py2/venv/notes.txt",
        "cs/App.csproj",
        "cs/bin/App.dll",
        "cs/obj/cache.bin",
        "web/node_modules/left-pad/index.js",
        "web/index.html",
    ] {
        write(&tree, rel, 1);
    }
    let mut o = opts();
    o.excludes = Arc::new(ExcludeSet::builtin(&env));
    let (seen, stats) = walk_with(&real(), &tree, &o, |_| WalkControl::Continue);
    for gone in [
        "rust/target",
        "py/venv",
        "cs/bin",
        "cs/obj",
        "web/node_modules",
    ] {
        assert!(
            !seen.iter().any(|s| s.starts_with(gone)),
            "{gone}: {seen:?}"
        );
    }
    for kept in [
        "rust/src/main.rs",
        "plain/target/keep.txt",
        "py2/venv/notes.txt",
        "cs/App.csproj",
        "web/index.html",
    ] {
        assert!(seen.contains(&kept.to_owned()), "{kept}: {seen:?}");
    }
    assert_eq!(stats.skipped_excluded, 5);

    // User globs: by name anywhere, and by absolute path.
    let user = format!(
        "{}/**",
        tree.join("web").to_string_lossy().replace('\\', "/")
    );
    let set = ExcludeSet::with_user(&env, &["*.RS".to_owned(), user]).unwrap();
    o.excludes = Arc::new(set);
    let (seen, _) = walk_with(&real(), &tree, &o, |_| WalkControl::Continue);
    assert!(!seen.contains(&"rust/src/main.rs".to_owned()));
    assert!(seen.contains(&"rust/src".to_owned()));
    assert!(seen.contains(&"web".to_owned()));
    assert!(!seen.iter().any(|s| s.starts_with("web/")), "{seen:?}");
}

/// Many folders on several threads: every entry is visited exactly once.
#[test]
fn parallel_walk_visits_each_entry_once() {
    let dir = tempfile::tempdir().unwrap();
    for d in 0..30 {
        for f in 0..10 {
            write(dir.path(), &format!("d{d}/sub/f{f}.txt"), 1);
        }
    }
    let real = real();
    for threads in [1, 3, 8] {
        let mut o = opts();
        o.threads = threads;
        let mut seen = HashSet::new();
        let stats = real
            .walk(
                dir.path(),
                &o,
                &mut |e| {
                    assert!(seen.insert(e.rel.clone()), "twice: {:?}", e.rel);
                    WalkControl::Continue
                },
                &CancellationToken::new(),
            )
            .unwrap();
        assert_eq!(seen.len(), 30 * 12, "threads {threads}");
        assert_eq!(stats.entries, 30 * 12);
    }
    // One pool per thread count, reused between walks.
    let pools = real.pools.lock().unwrap();
    assert!(pools.len() <= 3);
}

#[test]
fn metadata_exists_and_read_dir() {
    let dir = sample();
    let real = real();
    let meta = real.metadata(&dir.path().join("top.sav")).unwrap();
    assert_eq!((meta.kind, meta.size), (EntryKind::File, 5));
    assert_eq!(real.metadata(dir.path()).unwrap().kind, EntryKind::Dir);
    assert!(matches!(
        real.metadata(&dir.path().join("none")),
        Err(FsError::NotFound)
    ));
    assert!(real.exists(&dir.path().join("a").join("x.txt")));
    assert!(!real.exists(&dir.path().join("none")));

    let listed = real.read_dir(dir.path()).unwrap();
    let names: Vec<_> = listed.iter().map(|e| e.rel.clone()).collect();
    assert_eq!(names, [PathBuf::from("a"), PathBuf::from("top.sav")]);
    assert!(listed.iter().all(|e| e.depth == 1));
    assert_eq!(listed[1].path, dir.path().join("top.sav"));
    assert_eq!(listed[1].meta.size, 5);
    assert!(matches!(
        real.read_dir(&dir.path().join("none")),
        Err(FsError::NotFound)
    ));
    assert_eq!(
        real.probe_readable(&dir.path().join("top.sav")),
        Readability::Ok
    );
}

#[test]
fn thread_count_by_drive() {
    let ssd = Some((DriveKind::Fixed, DriveMedia::Ssd));
    let hdd = Some((DriveKind::Fixed, DriveMedia::Hdd));
    let unknown = Some((DriveKind::Fixed, DriveMedia::Unknown));
    let net = Some((DriveKind::Network, DriveMedia::Unknown));
    assert_eq!(walk_threads(0, ssd, 16), 8);
    assert_eq!(walk_threads(0, ssd, 4), 4);
    assert_eq!(walk_threads(3, ssd, 16), 3);
    assert_eq!(walk_threads(12, ssd, 16), 8);
    assert_eq!(walk_threads(12, unknown, 6), 6);
    assert_eq!(walk_threads(0, hdd, 16), 2);
    assert_eq!(walk_threads(8, hdd, 16), 2);
    assert_eq!(walk_threads(1, hdd, 16), 1);
    assert_eq!(walk_threads(0, net, 16), 2);
    assert_eq!(
        walk_threads(4, Some((DriveKind::Network, DriveMedia::Ssd)), 16),
        2
    );
    // A drive missing from `env.drives` counts as an SSD.
    assert_eq!(walk_threads(0, None, 16), 8);
    assert_eq!(walk_threads(0, None, 0), 1);
}

#[test]
fn drive_of_the_root() {
    let mut env = Environment::fake(Path::new("/nonexistent"));
    env.drives[0].letter = 'd';
    env.drives[0].media = DriveMedia::Hdd;
    let real = RealFs::new(&env);
    if cfg!(windows) {
        assert_eq!(drive_letter(Path::new(r"d:\Games")), Some('D'));
        assert_eq!(drive_letter(Path::new(r"\\?\D:\Games")), Some('D'));
        assert_eq!(drive_letter(Path::new(r"\\srv\share\x")), None);
        assert_eq!(real.threads(Path::new(r"D:\Games"), 0), 2);
        assert_eq!(real.threads(Path::new(r"\\?\D:\Games"), 8), 2);
    }
    assert_eq!(drive_letter(Path::new("/home/user")), None);
    // Not in `env.drives`: as an SSD, so never more than 8.
    assert!(real.threads(Path::new("/home/user"), 0) <= 8);
}

#[cfg(unix)]
#[test]
fn symlinks_are_reported_but_not_entered() {
    let dir = sample();
    std::os::unix::fs::symlink(dir.path().join("a"), dir.path().join("link")).unwrap();
    let mut kinds = Vec::new();
    let (seen, _) = walk_with(&real(), dir.path(), &opts(), |e| {
        kinds.push((e.rel.clone(), e.meta.kind));
        WalkControl::Continue
    });
    assert!(seen.contains(&"link".to_owned()));
    assert!(!seen.iter().any(|s| s.starts_with("link/")));
    assert!(kinds.contains(&(
        PathBuf::from("link"),
        EntryKind::Reparse(crate::ReparseKind::Symlink)
    )));
    let stats = real()
        .walk(
            &dir.path().join("link"),
            &opts(),
            &mut |_| WalkControl::Continue,
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(stats, WalkStats::default());
}
