use std::path::PathBuf;
use std::sync::Arc;

use sk_core::env::{CloudProvider, CloudRoot, Environment};
use sk_core::model::{ChildStat, ExtStat, FolderSummary, Marker};

use super::*;
use crate::{MemFs, RealFs, ReparseKind};

pub(super) fn root() -> PathBuf {
    PathBuf::from(if cfg!(windows) { r"C:\fake" } else { "/fake" })
}

pub(super) fn env() -> Environment {
    Environment::fake(&root())
}

/// Absolute path of `rel` (`/`-separated) under the fake root.
pub(super) fn p(rel: &str) -> PathBuf {
    rel.split('/').fold(root(), |p, c| p.join(c))
}

pub(super) fn s(rel: &str) -> String {
    p(rel).to_string_lossy().into_owned()
}

pub(super) fn opts() -> SummaryOptions {
    SummaryOptions::new(Arc::new(ExcludeSet::builtin(&env())))
}

/// A `MemFs` with files `(rel, size)` under the fake root.
pub(super) fn tree(files: &[(&str, u64)]) -> MemFs {
    let mut fs = MemFs::new();
    for (rel, size) in files {
        fs.add_file(&s(rel), *size, "-1d", None);
    }
    fs
}

pub(super) fn run_env(fs: &dyn FsScanner, rel: &str, env: &Environment) -> FolderSummary {
    summarize(fs, &p(rel), env, &opts(), &CancellationToken::new()).unwrap()
}

pub(super) fn run(fs: &dyn FsScanner, rel: &str) -> FolderSummary {
    run_env(fs, rel, &env())
}

pub(super) fn markers(fs: &dyn FsScanner, rel: &str) -> Vec<Marker> {
    run(fs, rel).markers
}

fn t(s: &str) -> time::OffsetDateTime {
    time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).unwrap()
}

fn ext(ext: &str, count: u64, bytes: u64) -> ExtStat {
    ExtStat {
        ext: ext.to_owned(),
        count,
        bytes,
    }
}

#[test]
fn counts_sizes_depth_and_times() {
    let mut fs = MemFs::new();
    fs.add_file(&s("d/a.cfg"), 10, "2024-01-01T00:00:00Z", None)
        .add_file(&s("d/sub/b.sav"), 300, "2025-06-01T00:00:00Z", None)
        .add_file(&s("d/sub/deep/c.txt"), 5, "not a time", None)
        .add_dir(&s("d/empty"))
        // An app alias counts as a file of 0 bytes, links are ignored.
        .add_file(&s("d/alias.exe"), 100, "-1d", None)
        .add_reparse(&s("d/alias.exe"), ReparseKind::AppExecLink)
        .add_reparse(&s("d/link"), ReparseKind::Symlink)
        .add_reparse(&s("d/sub/deep/deeper/junction"), ReparseKind::Junction);
    let sum = run(&fs, "d");
    assert_eq!(sum.path, PathTemplate::from_path(&p("d"), &env()));
    assert_eq!(sum.total_bytes, 315);
    assert_eq!(sum.file_count, 4);
    assert_eq!(sum.dir_count, 4);
    assert_eq!(sum.max_depth, 3);
    assert_eq!(sum.oldest_mtime, Some(t("2024-01-01T00:00:00Z")));
    assert!(sum.newest_mtime > Some(t("2025-06-01T00:00:00Z")));
    assert!(!sum.truncated);
    assert!(!sum.sample_names.iter().any(|n| n.contains("link")));
}

#[test]
fn root_cases() {
    let mut fs = tree(&[("d/f.txt", 3)]);
    fs.add_reparse(&s("j"), ReparseKind::Junction);
    let cancel = CancellationToken::new();
    let go =
        |fs: &MemFs, rel: &str, env: &Environment| summarize(fs, &p(rel), env, &opts(), &cancel);
    assert!(matches!(go(&fs, "missing", &env()), Err(FsError::NotFound)));
    assert!(matches!(go(&fs, "d/f.txt", &env()), Err(FsError::Io(_))));

    // A junction root is not entered; only the path markers are kept.
    let mut cloud_env = env();
    cloud_env.cloud_roots.push(CloudRoot {
        provider: CloudProvider::OneDrive,
        path: root(),
    });
    let sum = go(&fs, "j", &cloud_env).unwrap();
    assert_eq!((sum.file_count, sum.dir_count, sum.total_bytes), (0, 0, 0));
    assert_eq!(sum.markers, vec![Marker::CloudSynced]);
    assert!(sum.sample_names.is_empty() && sum.ext_histogram.is_empty());
    assert_eq!(fs.calls().read_dir, 0);

    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let r = summarize(&fs, &p("d"), &env(), &opts(), &cancelled);
    assert!(matches!(r, Err(FsError::Cancelled)));
}

#[test]
fn cloud_placeholders() {
    let mut fs = MemFs::new();
    // A cloud placeholder folder is entered; a placeholder file is a file.
    fs.add_dir(&s("od/docs"))
        .add_reparse(&s("od/docs"), ReparseKind::CloudPlaceholder)
        .add_file(&s("od/docs/a.pdf"), 40, "-1d", None)
        .add_reparse(&s("od/docs/a.pdf"), ReparseKind::CloudPlaceholder)
        .add_file(&s("od/docs/b.db"), 4096, "-1d", None)
        .set_cloud_only(&s("od/docs/b.db"));
    let sum = run(&fs, "od");
    assert_eq!(
        (sum.file_count, sum.dir_count, sum.total_bytes),
        (2, 1, 4136)
    );
    assert_eq!(sum.top_children[0].name, "docs");
    // The cloud-only database is never read.
    assert_eq!(fs.calls().read_head, 0);
    assert!(!sum.markers.contains(&Marker::SqliteFiles));

    // A placeholder folder as the root is walked.
    let sum = run(&fs, "od/docs");
    assert_eq!(sum.file_count, 2);
}

#[test]
fn histogram_is_sorted_and_limited() {
    let mut files: Vec<(String, u64)> = Vec::new();
    // 16 extensions with 1..=16 files.
    for i in 1..=16u64 {
        for k in 0..i {
            files.push((format!("h/e{i:02}/f{k}.x{i:02}"), 1));
        }
    }
    files.push(("h/.gitignore".to_owned(), 2));
    files.push(("h/a.TAR.GZ".to_owned(), 2));
    let refs: Vec<(&str, u64)> = files.iter().map(|(r, n)| (r.as_str(), *n)).collect();
    let sum = run(&tree(&refs), "h");
    assert_eq!(sum.ext_histogram.len(), 15);
    assert_eq!(sum.ext_histogram[0], ext("x16", 16, 16));
    assert_eq!(sum.ext_histogram[14], ext("x02", 2, 2));

    // Ties: bytes descending, then extension ascending; `.gitignore` has none.
    let sum = run(
        &tree(&[
            ("t/b.zz", 1),
            ("t/a.yy", 1),
            ("t/.gitignore", 5),
            ("t/x.tar.gz", 1),
        ]),
        "t",
    );
    let order: Vec<&str> = sum.ext_histogram.iter().map(|e| e.ext.as_str()).collect();
    assert_eq!(order, ["", "gz", "yy", "zz"]);
}

#[test]
fn sample_names_follow_the_three_steps() {
    let mut fs = MemFs::new();
    // Step 1: top-level entries by lowercase name: 7 candidates, 5 taken.
    for name in ["b.txt", "A.ini", "c", "D.log", "e.json", "f.cfg", "g.md"] {
        if name == "c" {
            fs.add_dir(&s(&format!("r/{name}")));
        } else {
            fs.add_file(&s(&format!("r/{name}")), 1, "2020-01-01T00:00:00Z", None);
        }
    }
    // Step 2: the newest files; `f.cfg` is not top-level-chosen, so it competes.
    for i in 0..12u32 {
        let mtime = format!("2024-01-{:02}T00:00:00Z", i + 1);
        fs.add_file(&s(&format!("r/c/new{i:02}.dat")), 1, &mtime, None);
    }
    fs.add_file(&s("r/c/d/no-time.bin"), 1, "bad", None);
    // Step 3: extensions not yet seen, in histogram order; smallest path each.
    fs.add_file(&s("r/z/b.png"), 1, "2019-01-01T00:00:00Z", None)
        .add_file(&s("r/z/a.png"), 1, "2019-01-01T00:00:00Z", None)
        .add_file(&s("r/z/q.mp3"), 9, "2019-01-01T00:00:00Z", None);
    let sum = run(&fs, "r");
    let sep = |a: &str| a.replace('/', "\\");
    let mut expected: Vec<String> = ["A.ini", "b.txt", "c", "D.log", "e.json"]
        .map(String::from)
        .to_vec();
    expected.extend((2..12).rev().map(|i| sep(&format!("c/new{i:02}.dat"))));
    // Extensions left: bin(1), cfg(1), md(1), mp3(1, 9 bytes), png(2) → png, mp3, bin, cfg, md.
    expected.extend(["z/a.png", "z/q.mp3", "c/d/no-time.bin", "f.cfg", "g.md"].map(sep));
    assert_eq!(sum.sample_names, expected[..20]);
}

#[test]
fn names_are_redacted() {
    let mut fs = MemFs::new();
    fs.add_file(&s("r/user-notes.txt"), 1, "-1d", None)
        .add_file(&s("r/user/mail me@example.org now.txt"), 5, "-1d", None);
    let sum = run(&fs, "r");
    assert!(sum
        .sample_names
        .contains(&"<redacted>-notes.txt".to_owned()));
    assert!(sum
        .sample_names
        .contains(&r"<redacted>\mail <redacted> now.txt".to_owned()));
    assert!(sum.sample_names.contains(&"<redacted>".to_owned()));
    assert_eq!(
        sum.top_children,
        vec![ChildStat {
            name: "<redacted>".to_owned(),
            bytes: 5,
            files: 1
        }]
    );
}

#[test]
fn top_children_are_the_largest_subfolders() {
    let mut files: Vec<(String, u64)> = Vec::new();
    for i in 0..12u64 {
        files.push((format!("r/d{i:02}/x/f.bin"), i * 10));
    }
    files.push(("r/big.bin".to_owned(), 10_000));
    files.push(("r/Tie/a".to_owned(), 110));
    files.push(("r/tie2/a".to_owned(), 55));
    files.push(("r/tie2/b".to_owned(), 55));
    let refs: Vec<(&str, u64)> = files.iter().map(|(r, n)| (r.as_str(), *n)).collect();
    let mut fs = tree(&refs);
    fs.add_dir(&s("r/empty"));
    let sum = run(&fs, "r");
    let got: Vec<(&str, u64, u64)> = sum
        .top_children
        .iter()
        .map(|c| (c.name.as_str(), c.bytes, c.files))
        .collect();
    assert_eq!(
        got,
        [
            ("tie2", 110, 2),
            ("d11", 110, 1),
            ("Tie", 110, 1),
            ("d10", 100, 1),
            ("d09", 90, 1),
            ("d08", 80, 1),
            ("d07", 70, 1),
            ("d06", 60, 1),
            ("d05", 50, 1),
            ("d04", 40, 1),
        ]
    );
}

#[test]
fn limits_and_exclusions() {
    let local = "Users/user/AppData/Local";
    let mut fs = tree(&[
        ("w/a/b/c/deep.txt", 1),
        ("w/node_modules/m.js", 8),
        ("w/x.txt", 1),
        (&format!("{local}/Temp/app/keep.cfg"), 4),
        (&format!("{local}/Temp/app/node_modules/m.js"), 8),
    ]);
    fs.add_dir(&s("w/empty"));
    let sum = run(&fs, "w");
    assert_eq!((sum.file_count, sum.total_bytes), (2, 2));
    assert!(sum.top_children.iter().all(|c| c.name != "node_modules"));

    // A depth cut is not a truncation; an entry limit is.
    let mut o = opts();
    o.max_depth = 2;
    let cut = summarize(&fs, &p("w"), &env(), &o, &CancellationToken::new()).unwrap();
    assert_eq!(
        (cut.file_count, cut.max_depth, cut.truncated),
        (1, 2, false)
    );
    o.max_entries = 2;
    let cut = summarize(&fs, &p("w"), &env(), &o, &CancellationToken::new()).unwrap();
    assert!(cut.truncated);

    // A root inside a path exclusion keeps only the name exclusions.
    let sum = run(&fs, &format!("{local}/Temp/app"));
    assert_eq!((sum.file_count, sum.total_bytes), (1, 4));
}

/// The files of the determinism tests: `(rel, size, mtime, content)`.
const SAME_TREE: &[(&str, u64, &str, Option<&[u8]>)] = &[
    ("a/x.txt", 3, "2024-03-01T10:00:00Z", None),
    ("a/b/y.dat", 10, "2024-03-02T10:00:00Z", None),
    ("a/b/c/z.dat", 20, "2024-03-02T10:00:00Z", None),
    ("B/Top.sav", 5, "2024-02-01T10:00:00Z", None),
    ("top.SAV", 5, "2024-03-02T10:00:00Z", None),
    ("cfg/settings.json", 7, "2024-01-01T10:00:00Z", None),
    (
        "data/store.db",
        32,
        "2024-01-05T10:00:00Z",
        Some(b"SQLite format 3\x000123456789abcdef"),
    ),
    ("Local State", 2, "2024-01-05T10:00:00Z", None),
    ("Default/Preferences", 2, "2024-01-05T10:00:00Z", None),
];

#[test]
fn same_tree_in_any_insertion_order_gives_the_same_summary() {
    let build = |order: &mut dyn Iterator<Item = &(&str, u64, &str, Option<&[u8]>)>| {
        let mut fs = MemFs::new();
        for (rel, size, mtime, content) in order {
            fs.add_file(&s(&format!("same/{rel}")), *size, mtime, *content);
        }
        fs
    };
    let forward = run(&build(&mut SAME_TREE.iter()), "same");
    let backward = run(&build(&mut SAME_TREE.iter().rev()), "same");
    assert_eq!(forward, backward);
    assert!(forward.markers.contains(&Marker::SqliteFiles));
    assert!(forward.markers.contains(&Marker::ChromiumProfile));
}

#[test]
fn real_fs_and_mem_fs_give_the_same_summary() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("same");
    let mut mem = MemFs::new();
    for (rel, size, mtime, content) in SAME_TREE {
        let path = rel.split('/').fold(base.clone(), |p, c| p.join(c));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let len = usize::try_from(*size).unwrap();
        let bytes = content.map_or_else(|| vec![0; len], <[u8]>::to_vec);
        std::fs::write(&path, &bytes).unwrap();
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(t(mtime).into()).unwrap();
        mem.add_file(&path.to_string_lossy(), *size, mtime, *content);
    }
    let env = Environment::fake(dir.path());
    let opts = SummaryOptions::new(Arc::new(ExcludeSet::builtin(&env)));
    let go = |fs: &dyn FsScanner| summarize(fs, &base, &env, &opts, &CancellationToken::new());
    let real = go(&RealFs::new(&env)).unwrap();
    let from_mem = go(&mem).unwrap();
    assert_eq!(real, from_mem);
    assert_eq!(real.file_count, 9);
    assert!(real.markers.contains(&Marker::SqliteFiles));
}
