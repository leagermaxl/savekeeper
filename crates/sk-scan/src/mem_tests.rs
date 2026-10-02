use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::Arc;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

use super::*;
use crate::{Exclusion, PathFilter};

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

/// Excludes one absolute path.
#[derive(Debug)]
struct PathExclude(PathBuf);

impl PathFilter for PathExclude {
    fn check(&self, abs: &Path, _: &OsStr, _: bool) -> Exclusion {
        if abs == self.0 {
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
        threads: 1,
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

/// `root/a/x.txt, root/a/b/y.dat, root/a/b/c/z.dat, root/top.sav`.
fn sample() -> MemFs {
    let mut fs = MemFs::new();
    fs.add_file("/r/a/x.txt", 3, "-1d", Some(b"abc"))
        .add_file("/r/a/b/y.dat", 10, "-2d", None)
        .add_file("/r/a/b/c/z.dat", 20, "", None)
        .add_file("/r/top.sav", 5, "2020-01-02T03:04:05Z", None);
    fs
}

/// Relative paths (`/`-separated) visited by a walk, and its stats.
fn walk_with(
    fs: &MemFs,
    opts: &WalkOptions,
    mut control: impl FnMut(&DirEntryInfo) -> WalkControl,
) -> (Vec<String>, WalkStats) {
    let mut seen = Vec::new();
    let stats = fs
        .walk(
            Path::new("/r"),
            opts,
            &mut |e| {
                let parts: Vec<_> = e.rel.iter().map(|c| c.to_string_lossy()).collect();
                seen.push(parts.join("/"));
                control(e)
            },
            &CancellationToken::new(),
        )
        .unwrap();
    (seen, stats)
}

#[test]
fn paths_are_case_insensitive_and_split_at_both_separators() {
    let fs = sample();
    assert!(fs.exists(Path::new(r"\r\A\B\Y.DAT")));
    assert!(fs.exists(Path::new(r"\\?\r\a\x.txt")));
    assert!(!fs.exists(Path::new("/r/a/missing")));
    assert!(!fs.exists(Path::new("")));
    assert_eq!(fs.calls().exists, 4);
}

#[test]
fn parents_are_created_implicitly() {
    let fs = sample();
    let meta = fs.metadata(Path::new("/r/a/b/c")).unwrap();
    assert_eq!(meta.kind, EntryKind::Dir);
    assert_eq!(
        meta.attrs & FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_DIRECTORY
    );
    let names: Vec<_> = fs
        .read_dir(Path::new("/r/a"))
        .unwrap()
        .into_iter()
        .map(|e| (e.rel, e.depth, e.meta.kind))
        .collect();
    assert_eq!(
        names,
        [
            (PathBuf::from("b"), 1, EntryKind::Dir),
            (PathBuf::from("x.txt"), 1, EntryKind::File)
        ]
    );
    assert!(matches!(
        fs.read_dir(Path::new("/r/a/x.txt")),
        Err(FsError::Io(_))
    ));
    assert!(matches!(
        fs.read_dir(Path::new("/r/none")),
        Err(FsError::NotFound)
    ));
    assert_eq!(fs.calls().read_dir, 3);
}

#[test]
fn a_file_on_the_way_becomes_a_folder() {
    let mut fs = MemFs::new();
    fs.add_file("/r/a", 1, "", None)
        .add_file("/r/a/b", 2, "", None);
    assert_eq!(fs.metadata(Path::new("/r/a")).unwrap().kind, EntryKind::Dir);
    fs.add_dir("/r/a/b");
    assert_eq!(
        fs.metadata(Path::new("/r/a/b")).unwrap().kind,
        EntryKind::Dir
    );
}

#[test]
fn mtimes_use_the_fixture_syntax() {
    let fs = sample();
    let meta = |p: &str| fs.metadata(Path::new(p)).unwrap();
    let now = OffsetDateTime::now_utc();
    let day_ago = meta("/r/a/x.txt").mtime.unwrap();
    assert!((now - day_ago - Duration::DAY).abs() < Duration::MINUTE);
    assert_eq!(meta("/r/a/b/c/z.dat").mtime, None);
    assert_eq!(
        meta("/r/top.sav").mtime,
        OffsetDateTime::parse("2020-01-02T03:04:05Z", &Rfc3339).ok()
    );
    let t = OffsetDateTime::UNIX_EPOCH;
    assert_eq!(parse_mtime("+2h", t), Some(t + Duration::HOUR * 2));
    assert_eq!(parse_mtime("-1w", t), Some(t - Duration::WEEK));
    assert_eq!(parse_mtime("-1y", t), None);
    assert_eq!(parse_mtime("-5", t), None);
    assert_eq!(parse_mtime("yesterday", t), None);
}

#[test]
fn reads_return_content_or_zeros() {
    let fs = sample();
    let p = |s: &str| PathBuf::from(s);
    assert_eq!(fs.read_head(&p("/r/a/x.txt"), 2).unwrap(), b"ab");
    assert_eq!(fs.read_small(&p("/r/a/x.txt"), 3).unwrap(), b"abc");
    assert_eq!(fs.read_head(&p("/r/a/b/y.dat"), 4).unwrap(), [0; 4]);
    assert_eq!(fs.read_small(&p("/r/a/b/y.dat"), 10).unwrap(), [0; 10]);
    assert!(matches!(
        fs.read_small(&p("/r/a/b/y.dat"), 9),
        Err(FsError::TooLarge)
    ));
    assert!(matches!(fs.read_head(&p("/r/a"), 4), Err(FsError::Io(_))));
    assert!(matches!(
        fs.read_head(&p("/r/none"), 4),
        Err(FsError::NotFound)
    ));
    assert_eq!(fs.calls().read_head, 4);
}

#[test]
fn locked_cloud_only_and_reparse_entries_are_not_read() {
    let mut fs = sample();
    fs.set_locked("/r/a/x.txt")
        .set_cloud_only("/r/top.sav")
        .add_reparse("/r/apps/tool.exe", ReparseKind::AppExecLink)
        .set_locked("/r/new.lock");
    let p = Path::new;
    assert!(matches!(
        fs.read_head(p("/r/a/x.txt"), 4),
        Err(FsError::SharingViolation)
    ));
    assert!(matches!(
        fs.read_small(p("/r/top.sav"), 100),
        Err(FsError::CloudOnly)
    ));
    assert!(matches!(
        fs.read_head(p("/r/apps/tool.exe"), 4),
        Err(FsError::Io(_))
    ));
    assert_eq!(fs.probe_readable(p("/r/a/x.txt")), Readability::Locked);
    assert_eq!(fs.probe_readable(p("/r/top.sav")), Readability::CloudOnly);
    assert_eq!(fs.probe_readable(p("/r/a/b/y.dat")), Readability::Ok);
    assert_eq!(fs.probe_readable(p("/r/none")), Readability::Missing);
    assert_eq!(fs.probe_readable(p("/r/new.lock")), Readability::Locked);
    let cloud = fs.metadata(p("/r/top.sav")).unwrap();
    assert_eq!(cloud.cloud, CloudState::CloudOnly);
    assert_eq!(cloud.size, 5);
    assert_ne!(cloud.attrs & FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS, 0);
}

/// The read rules of SPEC-03 §4.1, the same as in `RealFs`.
#[test]
fn reads_follow_the_spec_rules() {
    let mut fs = sample();
    fs.add_file("/r/odd*name", 1, "", None)
        .add_file("/r/short.txt", 10, "", Some(b"abc"))
        .add_dir("/r/cloud")
        .set_cloud_only("/r/cloud")
        .add_file("/r/hydrated.txt", 2, "", Some(b"ok"))
        .add_reparse("/r/hydrated.txt", ReparseKind::CloudPlaceholder)
        .add_reparse("/r/link.txt", ReparseKind::Symlink)
        .add_reparse("/r/a/b", ReparseKind::Junction);
    let p = Path::new;
    let io = |r: Result<Vec<u8>, FsError>| matches!(r, Err(FsError::Io(_)));
    // Wildcards name nothing, even a node added with one.
    for path in ["/r/odd*name", "/r/top.*", r"\\?\r\top.sa?"] {
        assert!(matches!(fs.read_head(p(path), 4), Err(FsError::NotFound)));
        assert!(matches!(fs.read_small(p(path), 4), Err(FsError::NotFound)));
    }
    // Cloud-only goes before the folder check.
    assert!(matches!(
        fs.read_head(p("/r/cloud"), 4),
        Err(FsError::CloudOnly)
    ));
    assert!(io(fs.read_small(p("/r/a"), 4)));
    assert!(io(fs.read_head(p("/r/link.txt"), 4)));
    assert!(io(fs.read_small(p("/r/a/b"), 4)));
    assert_eq!(fs.read_small(p("/r/hydrated.txt"), 2).unwrap(), b"ok");
    // Too large by metadata size, even if the content is shorter.
    assert!(matches!(
        fs.read_small(p("/r/short.txt"), 9),
        Err(FsError::TooLarge)
    ));
    assert_eq!(fs.read_small(p("/r/short.txt"), 10).unwrap(), b"abc");
    assert_eq!(fs.read_head(p("/r/short.txt"), 2).unwrap(), b"ab");
}

#[test]
fn walk_visits_all_entries_in_order() {
    let fs = sample();
    let (seen, stats) = walk_with(&fs, &opts(), |_| WalkControl::Continue);
    assert_eq!(
        seen,
        [
            "a",
            "a/b",
            "a/b/c",
            "a/b/c/z.dat",
            "a/b/y.dat",
            "a/x.txt",
            "top.sav"
        ]
    );
    assert_eq!(
        stats,
        WalkStats {
            entries: 7,
            ..WalkStats::default()
        }
    );
    // One listing per folder: r, a, b, c.
    assert_eq!(fs.calls().read_dir, 4);
}

#[test]
fn walk_reports_depth_and_absolute_paths() {
    let fs = sample();
    let mut found = Vec::new();
    fs.walk(
        Path::new("/R/A"),
        &opts(),
        &mut |e| {
            found.push((e.path.clone(), e.depth));
            WalkControl::Continue
        },
        &CancellationToken::new(),
    )
    .unwrap();
    assert!(found.contains(&(Path::new("/R/A").join("b").join("c"), 2)));
    assert!(found.contains(&(Path::new("/R/A").join("x.txt"), 1)));
}

#[test]
fn walk_skips_excluded_names_and_paths() {
    let fs = sample();
    let mut o = opts();
    o.excludes = Arc::new(NameFilter(vec!["B"]));
    let (seen, stats) = walk_with(&fs, &o, |_| WalkControl::Continue);
    assert_eq!(seen, ["a", "a/x.txt", "top.sav"]);
    assert_eq!(stats.skipped_excluded, 1);

    o.excludes = Arc::new(PathExclude(Path::new("/r").join("top.sav")));
    let (seen, _) = walk_with(&fs, &o, |_| WalkControl::Continue);
    assert!(!seen.contains(&"top.sav".to_owned()));
    assert_eq!(seen.len(), 6);
}

#[test]
fn walk_applies_include_and_exclude_globs() {
    let fs = sample();
    let mut o = opts();
    o.include = Some(globs(&["**/*.DAT"]));
    let (seen, _) = walk_with(&fs, &o, |_| WalkControl::Continue);
    assert_eq!(seen, ["a", "a/b", "a/b/c", "a/b/c/z.dat", "a/b/y.dat"]);

    o.include = None;
    o.exclude = Some(globs(&["a/b"]));
    let (seen, stats) = walk_with(&fs, &o, |_| WalkControl::Continue);
    assert_eq!(seen, ["a", "a/x.txt", "top.sav"]);
    assert_eq!(stats.skipped_excluded, 1);
}

#[test]
fn walk_skip_dir_and_stop() {
    let fs = sample();
    let (seen, _) = walk_with(&fs, &opts(), |e| {
        if e.rel == Path::new("a").join("b") {
            WalkControl::SkipDir
        } else {
            WalkControl::Continue
        }
    });
    assert_eq!(seen, ["a", "a/b", "a/x.txt", "top.sav"]);

    let (seen, stats) = walk_with(&fs, &opts(), |e| {
        if e.depth == 3 {
            WalkControl::Stop
        } else {
            WalkControl::Continue
        }
    });
    assert_eq!(seen, ["a", "a/b", "a/b/c"]);
    assert_eq!(stats.entries, 3);
    assert!(!stats.truncated);
}

#[test]
fn walk_limits_depth_and_entries() {
    let fs = sample();
    let mut o = opts();
    o.max_depth = 2;
    let (seen, _) = walk_with(&fs, &o, |_| WalkControl::Continue);
    assert_eq!(seen, ["a", "a/b", "a/x.txt", "top.sav"]);

    o.max_depth = 32;
    o.max_entries = 7;
    let (_, stats) = walk_with(&fs, &o, |_| WalkControl::Continue);
    assert!(!stats.truncated, "exactly at the limit");

    o.max_entries = 3;
    let (seen, stats) = walk_with(&fs, &o, |_| WalkControl::Continue);
    assert_eq!(seen.len(), 3);
    assert_eq!(stats.entries, 3);
    assert!(stats.truncated);
}

#[test]
fn walk_is_cancelled() {
    let fs = sample();
    let cancel = CancellationToken::new();
    let mut visited = 0;
    let result = fs.walk(
        Path::new("/r"),
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
    let again = fs.walk(
        Path::new("/r"),
        &opts(),
        &mut |_| WalkControl::Continue,
        &cancel,
    );
    assert!(matches!(again, Err(FsError::Cancelled)));
}

#[test]
fn walk_does_not_enter_reparse_points() {
    let mut fs = sample();
    fs.add_reparse("/r/a/b", ReparseKind::Junction)
        .add_dir("/r/od")
        .add_file("/r/od/doc.txt", 1, "", None)
        .add_reparse("/r/od", ReparseKind::CloudPlaceholder);
    let (seen, _) = walk_with(&fs, &opts(), |_| WalkControl::Continue);
    assert_eq!(seen, ["a", "a/b", "a/x.txt", "od", "od/doc.txt", "top.sav"]);

    // A reparse root is not entered.
    let stats = fs
        .walk(
            Path::new("/r/a/b"),
            &opts(),
            &mut |_| WalkControl::Continue,
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(stats, WalkStats::default());
    let junction = fs.metadata(Path::new("/r/a/b")).unwrap();
    assert_eq!(junction.kind, EntryKind::Reparse(ReparseKind::Junction));
    assert_ne!(junction.attrs & FILE_ATTRIBUTE_REPARSE_POINT, 0);
}

#[test]
fn walk_root_errors() {
    let fs = sample();
    let mut visit = |_: &DirEntryInfo| WalkControl::Continue;
    let cancel = CancellationToken::new();
    assert!(matches!(
        fs.walk(Path::new("/r/none"), &opts(), &mut visit, &cancel),
        Err(FsError::NotFound)
    ));
    assert!(matches!(
        fs.walk(Path::new("/r/top.sav"), &opts(), &mut visit, &cancel),
        Err(FsError::Io(_))
    ));
}

#[test]
fn a_junction_is_always_a_folder() {
    let mut fs = sample();
    fs.add_reparse("/r/top.sav", ReparseKind::Junction)
        .add_reparse("/r/new", ReparseKind::Junction)
        .add_reparse("/r/link.txt", ReparseKind::Symlink);
    for p in ["/r/top.sav", "/r/new"] {
        let meta = fs.metadata(Path::new(p)).unwrap();
        assert_eq!(meta.kind, EntryKind::Reparse(ReparseKind::Junction), "{p}");
        assert_ne!(meta.attrs & FILE_ATTRIBUTE_DIRECTORY, 0, "{p}");
        assert_ne!(meta.attrs & FILE_ATTRIBUTE_REPARSE_POINT, 0, "{p}");
    }
    let link = fs.metadata(Path::new("/r/link.txt")).unwrap();
    assert_eq!(link.attrs & FILE_ATTRIBUTE_DIRECTORY, 0);
}

/// Cloud placeholders are `Reparse(CloudPlaceholder)` for files and folders;
/// only folders have `FILE_ATTRIBUTE_DIRECTORY` and are walked (SPEC-03 §4.2).
#[test]
fn cloud_placeholders_are_told_apart_by_the_directory_attribute() {
    let mut fs = MemFs::new();
    fs.add_file("/r/od/doc.txt", 7, "", None)
        .add_reparse("/r/od", ReparseKind::CloudPlaceholder)
        .add_reparse("/r/od/doc.txt", ReparseKind::CloudPlaceholder)
        .set_cloud_only("/r/od/doc.txt")
        .add_reparse("/r/new.txt", ReparseKind::CloudPlaceholder);
    let kind = EntryKind::Reparse(ReparseKind::CloudPlaceholder);

    let folder = fs.metadata(Path::new("/r/od")).unwrap();
    assert_eq!(folder.kind, kind);
    assert_ne!(folder.attrs & FILE_ATTRIBUTE_DIRECTORY, 0);
    assert_ne!(folder.attrs & FILE_ATTRIBUTE_REPARSE_POINT, 0);
    for file in ["/r/od/doc.txt", "/r/new.txt"] {
        let meta = fs.metadata(Path::new(file)).unwrap();
        assert_eq!(meta.kind, kind, "{file}");
        assert_eq!(meta.attrs & FILE_ATTRIBUTE_DIRECTORY, 0, "{file}");
        assert_ne!(meta.attrs & FILE_ATTRIBUTE_REPARSE_POINT, 0, "{file}");
    }
    let doc = fs.metadata(Path::new("/r/od/doc.txt")).unwrap();
    assert_eq!((doc.size, doc.cloud), (7, CloudState::CloudOnly));

    let (seen, _) = walk_with(&fs, &opts(), |_| WalkControl::Continue);
    assert_eq!(seen, ["new.txt", "od", "od/doc.txt"]);
}
