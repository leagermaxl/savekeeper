use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use sk_core::env::Environment;
use sk_core::model::{RegHive, Target};
use sk_core::template::PathTemplate;

use super::*;
use crate::{DirEntryInfo, MemFs, Readability, ReparseKind, WalkStats};

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

pub(super) fn opts() -> MeasureOptions {
    MeasureOptions::new(Arc::new(ExcludeSet::builtin(&env())), 32)
}

pub(super) fn set(rel: &str, include: &[&str], exclude: &[&str]) -> Target {
    let resolved = p(rel);
    Target::FileSet {
        root: PathTemplate::from_path(&resolved, &env()),
        resolved,
        include: include.iter().map(|g| (*g).to_owned()).collect(),
        exclude: exclude.iter().map(|g| (*g).to_owned()).collect(),
    }
}

pub(super) fn file(rel: &str) -> Target {
    let resolved = p(rel);
    Target::File {
        path: PathTemplate::from_path(&resolved, &env()),
        resolved,
    }
}

/// A `MemFs` that records lock probes and can report walk errors.
#[derive(Debug, Default)]
pub(super) struct Wrap {
    pub(super) fs: MemFs,
    pub(super) probes: Mutex<Vec<PathBuf>>,
    pub(super) walk_errors: AtomicU64,
}

impl Wrap {
    pub(super) fn new(fs: MemFs) -> Self {
        Self {
            fs,
            ..Self::default()
        }
    }

    fn probed(&self) -> Vec<PathBuf> {
        self.probes.lock().unwrap().clone()
    }
}

impl FsScanner for Wrap {
    fn metadata(&self, path: &Path) -> Result<EntryMeta, FsError> {
        self.fs.metadata(path)
    }
    fn exists(&self, path: &Path) -> bool {
        self.fs.exists(path)
    }
    fn read_dir(&self, path: &Path) -> Result<Vec<DirEntryInfo>, FsError> {
        self.fs.read_dir(path)
    }
    fn walk(
        &self,
        root: &Path,
        opts: &WalkOptions,
        visit: &mut dyn FnMut(&DirEntryInfo) -> WalkControl,
        cancel: &CancellationToken,
    ) -> Result<WalkStats, FsError> {
        let mut stats = self.fs.walk(root, opts, visit, cancel)?;
        stats.errors += self.walk_errors.load(Ordering::Relaxed);
        Ok(stats)
    }
    fn read_head(&self, path: &Path, max: usize) -> Result<Vec<u8>, FsError> {
        self.fs.read_head(path, max)
    }
    fn read_small(&self, path: &Path, max: usize) -> Result<Vec<u8>, FsError> {
        self.fs.read_small(path, max)
    }
    fn probe_readable(&self, path: &Path) -> Readability {
        self.probes.lock().unwrap().push(path.to_path_buf());
        self.fs.probe_readable(path)
    }
}

fn run(fs: &dyn FsScanner, target: &Target, opts: &MeasureOptions) -> TargetStats {
    measure(
        fs,
        target,
        &DirStatsCache::new(),
        opts,
        &CancellationToken::new(),
    )
    .unwrap()
    .unwrap()
}

#[test]
fn counts_files_folders_times_and_largest() {
    let mut fs = MemFs::new();
    fs.add_file(&s("app/a.cfg"), 10, "2024-01-01T00:00:00Z", None)
        .add_file(&s("app/sub/b.sav"), 300, "2025-06-01T00:00:00Z", None)
        .add_file(&s("app/sub/deep/c.txt"), 5, "not a time", None)
        .add_dir(&s("app/empty"));
    let stats = run(&fs, &set("app", &[], &[]), &opts());
    assert_eq!(stats.total_bytes, 315);
    assert_eq!(stats.file_count, 3);
    assert_eq!(stats.dir_count, 3);
    assert_eq!(stats.largest_file_bytes, Some(300));
    let t = |s: &str| {
        time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).unwrap()
    };
    assert_eq!(stats.newest_mtime, Some(t("2025-06-01T00:00:00Z")));
    assert_eq!(stats.oldest_mtime, Some(t("2024-01-01T00:00:00Z")));
    assert!(!stats.truncated);
    assert_eq!(stats.locked_files, 0);

    let mut empty = MemFs::new();
    empty.add_dir(&s("void"));
    let stats = run(&empty, &set("void", &[], &[]), &opts());
    assert_eq!((stats.file_count, stats.largest_file_bytes), (0, None));
}

#[test]
fn include_and_exclude_globs_ignore_case() {
    let mut fs = MemFs::new();
    fs.add_file(&s("g/One.SAV"), 1, "-1d", None)
        .add_file(&s("g/two.txt"), 2, "-1d", None)
        .add_file(&s("g/Saves/x.bin"), 4, "-1d", None)
        .add_file(&s("g/saves/skip/y.bin"), 8, "-1d", None)
        .add_file(&s("g/deep/z.sav"), 16, "-1d", None);
    let target = set("g", &["*.sav", "SAVES/**"], &["saves/SKIP/**"]);
    let stats = run(&fs, &target, &opts());
    // `*` does not cross `/`, so deep/z.sav is left out.
    assert_eq!((stats.total_bytes, stats.file_count), (5, 2));
}

#[test]
fn cloud_placeholders_count_as_files_and_folders() {
    let mut fs = MemFs::new();
    fs.add_file(&s("od/hydrated.docx"), 100, "-1d", None)
        .add_reparse(&s("od/hydrated.docx"), ReparseKind::CloudPlaceholder)
        .add_file(&s("od/online.mp4"), 1000, "-1d", None)
        .add_reparse(&s("od/online.mp4"), ReparseKind::CloudPlaceholder)
        .set_cloud_only(&s("od/online.mp4"))
        .add_dir(&s("od/Folder"))
        .add_reparse(&s("od/Folder"), ReparseKind::CloudPlaceholder)
        .add_file(&s("od/Folder/in.txt"), 10, "-1d", None);
    let fs = Wrap::new(fs);
    let stats = run(&fs, &set("od", &[], &[]), &opts());
    assert_eq!(stats.file_count, 3);
    assert_eq!(stats.dir_count, 1);
    assert_eq!(stats.total_bytes, 1110);
    assert_eq!((stats.cloud_only_bytes, stats.cloud_only_files), (1000, 1));
    assert_eq!(stats.largest_file_bytes, Some(1000));
}

#[test]
fn cloud_only_files_are_never_probed() {
    let mut fs = MemFs::new();
    fs.add_file(&s("d/cloud.db"), 7, "-1d", None)
        .set_cloud_only(&s("d/cloud.db"))
        .add_file(&s("d/local.db"), 1, "-1d", None);
    let fs = Wrap::new(fs);
    let stats = run(&fs, &set("d", &[], &[]), &opts());
    assert_eq!(fs.probed(), [p("d/local.db")]);
    assert_eq!(stats.cloud_only_files, 1);

    let fs = Wrap::new({
        let mut m = MemFs::new();
        m.add_file(&s("c.db"), 3, "-1d", None)
            .set_cloud_only(&s("c.db"));
        m
    });
    let stats = run(&fs, &file("c.db"), &opts());
    assert!(fs.probed().is_empty());
    assert_eq!((stats.file_count, stats.cloud_only_bytes), (1, 3));
}

#[test]
fn locks_are_probed_for_lockable_extensions_only() {
    let mut fs = MemFs::new();
    fs.add_file(&s("p/places.SQLITE"), 1, "-1d", None)
        .set_locked(&s("p/places.SQLITE"))
        .add_file(&s("p/app.log"), 1, "-1d", None)
        .add_file(&s("p/notes.txt"), 1, "-1d", None)
        .set_locked(&s("p/notes.txt"));
    let fs = Wrap::new(fs);
    let stats = run(&fs, &set("p", &[], &[]), &opts());
    assert_eq!(stats.locked_files, 1);
    let mut probed = fs.probed();
    probed.sort();
    assert_eq!(probed, [p("p/app.log"), p("p/places.SQLITE")]);

    let mut plain = MemFs::new();
    plain
        .add_file(&s("p/x.db"), 1, "-1d", None)
        .set_locked(&s("p/x.db"));
    let fs = Wrap::new(plain);
    let mut no_probe = opts();
    no_probe.probe_locks = false;
    assert_eq!(run(&fs, &set("p", &[], &[]), &no_probe).locked_files, 0);
    assert!(fs.probed().is_empty());
}

#[test]
fn at_most_500_probes_per_root() {
    let mut fs = MemFs::new();
    for i in 0..510 {
        fs.add_file(&s(&format!("many/{i}.dat")), 1, "-1d", None);
    }
    let fs = Wrap::new(fs);
    let stats = run(&fs, &set("many", &[], &[]), &opts());
    assert_eq!(stats.file_count, 510);
    assert_eq!(fs.probed().len(), 500);
}

#[test]
fn links_count_by_kind() {
    let mut fs = MemFs::new();
    fs.add_file(&s("l/real.txt"), 5, "-1d", None)
        .add_file(&s("l/alias.exe"), 999, "-1d", None)
        .add_reparse(&s("l/alias.exe"), ReparseKind::AppExecLink)
        .add_file(&s("l/link.txt"), 50, "-1d", None)
        .add_reparse(&s("l/link.txt"), ReparseKind::Symlink)
        .add_reparse(&s("l/junction"), ReparseKind::Junction)
        .add_reparse(&s("l/other"), ReparseKind::Other(7));
    let stats = run(&fs, &set("l", &[], &[]), &opts());
    assert_eq!(
        (stats.file_count, stats.dir_count, stats.total_bytes),
        (2, 0, 5)
    );
    assert_eq!(stats.largest_file_bytes, Some(5));
}

#[test]
fn explicit_root_uses_name_exclusions_only() {
    let local = "Users/user/AppData/Local";
    let mut fs = MemFs::new();
    fs.add_file(&s(&format!("{local}/Temp/app/keep.cfg")), 4, "-1d", None)
        .add_file(
            &s(&format!("{local}/Temp/app/node_modules/m.js")),
            8,
            "-1d",
            None,
        )
        .add_file(&s(&format!("{local}/Mine/a.cfg")), 1, "-1d", None);
    let inside = set(&format!("{local}/Temp/app"), &[], &[]);
    assert_eq!(
        Mode::for_root(&opts(), &p(&format!("{local}/Temp/app"))),
        Mode::NamesOnly
    );
    let stats = run(&fs, &inside, &opts());
    assert_eq!((stats.file_count, stats.total_bytes), (1, 4));

    // Not an explicit root: the path exclusion below the root still applies.
    assert_eq!(Mode::for_root(&opts(), &p(local)), Mode::Full);
    let stats = run(&fs, &set(local, &[], &[]), &opts());
    assert_eq!((stats.file_count, stats.total_bytes), (1, 1));

    // A root named like a name exclusion is not an explicit root.
    assert_eq!(Mode::for_root(&opts(), &p("x/node_modules")), Mode::Full);
}

#[test]
fn include_excluded_switches_exclusions_off() {
    let mut fs = MemFs::new();
    fs.add_file(&s("w/node_modules/m.js"), 8, "-1d", None)
        .add_file(&s("w/a.txt"), 1, "-1d", None);
    let mut all = opts();
    all.include_excluded = true;
    assert_eq!(Mode::for_root(&all, &p("w")), Mode::Off);
    assert_eq!(run(&fs, &set("w", &[], &[]), &all).total_bytes, 9);
    assert_eq!(run(&fs, &set("w", &[], &[]), &opts()).total_bytes, 1);
}

#[test]
fn file_targets() {
    let mut fs = MemFs::new();
    fs.add_file(&s("cfg/settings.json"), 42, "2025-01-01T00:00:00Z", None)
        .add_file(&s("cfg/open.txt"), 1, "-1d", None)
        .set_locked(&s("cfg/open.txt"))
        .add_file(&s("cfg/alias.exe"), 9, "-1d", None)
        .add_reparse(&s("cfg/alias.exe"), ReparseKind::AppExecLink);
    let fs = Wrap::new(fs);
    let stats = run(&fs, &file("cfg/settings.json"), &opts());
    assert_eq!(
        (stats.file_count, stats.total_bytes, stats.dir_count),
        (1, 42, 0)
    );
    assert_eq!(stats.largest_file_bytes, Some(42));
    assert!(stats.newest_mtime.is_some() && stats.newest_mtime == stats.oldest_mtime);
    // Any extension is probed for a single file.
    assert_eq!(run(&fs, &file("cfg/open.txt"), &opts()).locked_files, 1);
    let alias = run(&fs, &file("cfg/alias.exe"), &opts());
    assert_eq!((alias.file_count, alias.total_bytes), (1, 0));

    let cache = DirStatsCache::new();
    let cancel = CancellationToken::new();
    let err = measure(&fs, &file("cfg"), &cache, &opts(), &cancel).unwrap_err();
    assert!(matches!(err, FsError::Io(_)), "{err:?}");
    let err = measure(&fs, &file("cfg/none"), &cache, &opts(), &cancel).unwrap_err();
    assert!(matches!(err, FsError::NotFound), "{err:?}");
    let err = measure(&fs, &set("none", &[], &[]), &cache, &opts(), &cancel).unwrap_err();
    assert!(matches!(err, FsError::NotFound), "{err:?}");
}

#[test]
fn registry_and_exports_have_no_stats() {
    let fs = MemFs::new();
    let cache = DirStatsCache::new();
    let cancel = CancellationToken::new();
    let reg = Target::Registry {
        hive: RegHive::Hkcu,
        key: r"Software\X".to_owned(),
        recursive: true,
    };
    assert_eq!(measure(&fs, &reg, &cache, &opts(), &cancel).unwrap(), None);
    let export = Target::SystemExport {
        exporter_id: "winget".to_owned(),
        params: Default::default(),
    };
    assert_eq!(
        measure(&fs, &export, &cache, &opts(), &cancel).unwrap(),
        None
    );
}

#[test]
fn invalid_glob_is_an_io_error() {
    let mut fs = MemFs::new();
    fs.add_dir(&s("g"));
    let err = measure(
        &fs,
        &set("g", &["a[b"], &[]),
        &DirStatsCache::new(),
        &opts(),
        &CancellationToken::new(),
    )
    .unwrap_err();
    assert!(matches!(err, FsError::Io(_)), "{err:?}");
}

/// SPEC-03 §6: a path of 400 characters is measured by `RealFs`.
#[cfg(windows)]
#[test]
fn long_path_is_measured_on_windows() {
    use sk_core::path::to_extended;

    let dir = tempfile::tempdir().unwrap();
    let mut deep = dir.path().to_path_buf();
    while deep.as_os_str().len() < 400 {
        deep.push("a_rather_long_directory_name");
    }
    std::fs::create_dir_all(to_extended(&deep)).unwrap();
    std::fs::write(to_extended(&deep.join("save.sav")), b"save").unwrap();
    let real = crate::RealFs::new(&env());
    let target = Target::FileSet {
        root: PathTemplate::from_path(dir.path(), &env()),
        resolved: dir.path().to_path_buf(),
        include: vec![],
        exclude: vec![],
    };
    let stats = run(&real, &target, &opts());
    assert_eq!((stats.file_count, stats.total_bytes), (1, 4));
    assert!(!stats.truncated);
    let file_target = Target::File {
        path: PathTemplate::from_path(&deep.join("save.sav"), &env()),
        resolved: deep.join("save.sav"),
    };
    assert_eq!(run(&real, &file_target, &opts()).total_bytes, 4);
}

#[path = "measure_cache_tests.rs"]
mod cache_tests;
