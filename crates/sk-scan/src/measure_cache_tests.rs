//! `DirStatsCache` rules (SPEC-03 §4.3).

use super::*;
use crate::measure::cache::{norm_path, CacheKey};

/// b/{x/{y/{f1, z/f2}, g}, top} — four folders.
fn tree() -> MemFs {
    let mut fs = MemFs::new();
    fs.add_file(&s("b/x/y/f1.txt"), 1, "2025-01-01T00:00:00Z", None)
        .add_file(&s("b/x/y/z/f2.txt"), 2, "2025-01-01T00:00:00Z", None)
        .add_file(&s("b/x/g.txt"), 4, "2025-01-01T00:00:00Z", None)
        .add_file(&s("b/top.txt"), 8, "2025-01-01T00:00:00Z", None);
    fs
}

fn go(
    fs: &dyn FsScanner,
    target: &Target,
    cache: &DirStatsCache,
    opts: &MeasureOptions,
) -> TargetStats {
    measure(fs, target, cache, opts, &CancellationToken::new())
        .unwrap()
        .unwrap()
}

#[test]
fn nested_root_comes_from_the_cache() {
    let fs = tree();
    let cache = DirStatsCache::new();
    let outer = go(&fs, &set("b", &[], &[]), &cache, &opts());
    assert_eq!(
        (outer.file_count, outer.dir_count, outer.total_bytes),
        (4, 3, 15)
    );
    let walked = fs.calls().read_dir;
    assert_eq!(walked, 4);
    // Root + b/x, b/x/y, b/x/y/z.
    assert_eq!(cache.len(), 4);

    let inner = go(&fs, &set("b/x/y", &[], &[]), &cache, &opts());
    let z = go(&fs, &set("B/X/Y/Z", &[], &[]), &cache, &opts());
    assert_eq!(
        fs.calls().read_dir,
        walked,
        "nested roots are not walked again"
    );
    let direct = |rel: &str| go(&tree(), &set(rel, &[], &[]), &DirStatsCache::new(), &opts());
    assert_eq!(inner, direct("b/x/y"));
    assert_eq!(z, direct("b/x/y/z"));
    assert_eq!(
        (inner.file_count, inner.dir_count, inner.total_bytes),
        (2, 1, 3)
    );
}

#[test]
fn only_three_levels_are_cached() {
    let mut fs = MemFs::new();
    fs.add_file(&s("r/1/2/3/4/f.txt"), 1, "-1d", None);
    let cache = DirStatsCache::new();
    go(&fs, &set("r", &[], &[]), &cache, &opts());
    let walked = fs.calls().read_dir;
    go(&fs, &set("r/1/2/3", &[], &[]), &cache, &opts());
    assert_eq!(fs.calls().read_dir, walked);
    go(&fs, &set("r/1/2/3/4", &[], &[]), &cache, &opts());
    assert_eq!(fs.calls().read_dir, walked + 1);
}

#[test]
fn globs_and_modes_are_part_of_the_key() {
    let fs = tree();
    let cache = DirStatsCache::new();
    let only_txt = go(&fs, &set("b", &["**/g.txt"], &[]), &cache, &opts());
    assert_eq!(only_txt.total_bytes, 4);
    // With globs, subfolders are not stored.
    assert_eq!(cache.len(), 1);
    let walked = fs.calls().read_dir;
    assert_eq!(
        go(&fs, &set("b", &[], &[]), &cache, &opts()).total_bytes,
        15
    );
    assert!(fs.calls().read_dir > walked);

    let mut off = opts();
    off.include_excluded = true;
    let walked = fs.calls().read_dir;
    go(&fs, &set("b", &[], &[]), &cache, &off);
    assert!(fs.calls().read_dir > walked, "another mode is another key");
}

#[test]
fn excluded_subfolders_get_no_entry() {
    let mut fs = MemFs::new();
    fs.add_file(&s("proj/node_modules/m/i.js"), 5, "-1d", None)
        .add_file(&s("proj/a.txt"), 1, "-1d", None);
    let cache = DirStatsCache::new();
    assert_eq!(
        go(&fs, &set("proj", &[], &[]), &cache, &opts()).total_bytes,
        1
    );
    let walked = fs.calls().read_dir;
    let nm = go(&fs, &set("proj/node_modules", &[], &[]), &cache, &opts());
    assert_eq!(nm.total_bytes, 5);
    assert!(fs.calls().read_dir > walked);
}

#[test]
fn incomplete_walks_store_no_subfolders() {
    let cache = DirStatsCache::new();
    let mut limited = opts();
    limited.max_entries = 2;
    let stats = go(&tree(), &set("b", &[], &[]), &cache, &limited);
    assert!(stats.truncated);
    assert_eq!(cache.len(), 1, "truncated: root only");

    let cache = DirStatsCache::new();
    let fs = Wrap::new(tree());
    fs.walk_errors
        .store(1, std::sync::atomic::Ordering::Relaxed);
    let stats = go(&fs, &set("b", &[], &[]), &cache, &opts());
    assert!(stats.truncated);
    assert_eq!(cache.len(), 1, "errors: root only");

    let cache = DirStatsCache::new();
    let mut shallow = opts();
    shallow.max_depth = 2;
    let stats = go(&tree(), &set("b", &[], &[]), &cache, &shallow);
    assert!(!stats.truncated);
    assert_eq!(stats.file_count, 2);
    assert_eq!(cache.len(), 1, "max_depth cut: root only");

    let cache = DirStatsCache::new();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let err = measure(&tree(), &set("b", &[], &[]), &cache, &opts(), &cancel).unwrap_err();
    assert!(matches!(err, FsError::Cancelled));
    assert_eq!(cache.len(), 0, "cancelled: nothing");
}

#[test]
fn keys_are_normalized() {
    assert_eq!(norm_path(Path::new(r"\\?\C:\Users\Max\")), r"c:\users\max");
    assert_eq!(
        norm_path(Path::new(r"\\?\UNC\srv\Share\x")),
        r"\\srv\share\x"
    );
    assert_eq!(norm_path(Path::new("C:/A/b")), r"c:\a\b");
    assert_eq!(norm_path(Path::new(r"C:\")), r"c:\");
    assert_eq!(norm_path(Path::new("/")), r"\");

    let key = |p: &str, inc: &[&str]| {
        let inc: Vec<String> = inc.iter().map(|g| (*g).to_owned()).collect();
        CacheKey::new(Path::new(p), &inc, &[], Mode::Full)
    };
    assert!(key(r"C:\a\b", &[]).nested_in(&key(r"c:\A", &[]), 3));
    assert!(key(r"C:\a\b\c\d", &[]).nested_in(&key(r"C:\a", &[]), 3));
    assert!(!key(r"C:\a\b\c\d\e", &[]).nested_in(&key(r"C:\a", &[]), 3));
    assert!(!key(r"C:\a", &[]).nested_in(&key(r"C:\a", &[]), 3));
    assert!(!key(r"C:\ab", &[]).nested_in(&key(r"C:\a", &[]), 3));
    assert!(key(r"C:\a", &[]).nested_in(&key(r"C:\", &[]), 3));
    assert!(!key(r"C:\a\b", &["*"]).nested_in(&key(r"C:\a", &[]), 3));
    let names_only = CacheKey::new(Path::new(r"C:\a\b"), &[], &[], Mode::NamesOnly);
    assert!(!names_only.nested_in(&key(r"C:\a", &[]), 3));
}
