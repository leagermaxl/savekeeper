use std::ffi::OsStr;
use std::time::{Duration, Instant};

use sk_core::fs::{DirEntryInfo, EntryMeta, Exclusion, PathFilter, Readability};
use sk_core::model::Marker;
use sk_scan::{MemFs, WalkControl, WalkOptions, WalkStats};

use super::*;

fn root() -> &'static str {
    if cfg!(windows) {
        r"C:\fake"
    } else {
        "/fake"
    }
}

fn env() -> Environment {
    Environment::fake(Path::new(root()))
}

fn opts() -> SummaryOptions {
    SummaryOptions::new(Arc::new(ExcludeSet::builtin(&env())))
}

fn proj() -> PathBuf {
    Path::new(root()).join("proj")
}

fn mem() -> MemFs {
    let r = root();
    let mut fs = MemFs::new();
    fs.add_file(&format!("{r}/proj/a.txt"), 5, "-1d", None)
        .add_file(&format!("{r}/proj/.git/HEAD"), 10, "-1d", None)
        .add_file(&format!("{r}/proj/node_modules/x.js"), 100, "-1d", None);
    fs
}

/// A folder whose walk lasts until it is cancelled.
struct Endless(MemFs);

impl FsScanner for Endless {
    fn metadata(&self, path: &Path) -> Result<EntryMeta, FsError> {
        self.0.metadata(path)
    }
    fn exists(&self, path: &Path) -> bool {
        self.0.exists(path)
    }
    fn read_dir(&self, path: &Path) -> Result<Vec<DirEntryInfo>, FsError> {
        self.0.read_dir(path)
    }
    fn walk(
        &self,
        _root: &Path,
        _opts: &WalkOptions,
        _visit: &mut dyn FnMut(&DirEntryInfo) -> WalkControl,
        cancel: &CancellationToken,
    ) -> Result<WalkStats, FsError> {
        let start = Instant::now();
        while !cancel.is_cancelled() {
            if start.elapsed() > Duration::from_secs(10) {
                return Ok(WalkStats::default());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Err(FsError::Cancelled)
    }
    fn read_head(&self, path: &Path, max: usize) -> Result<Vec<u8>, FsError> {
        self.0.read_head(path, max)
    }
    fn read_small(&self, path: &Path, max: usize) -> Result<Vec<u8>, FsError> {
        self.0.read_small(path, max)
    }
    fn probe_readable(&self, path: &Path) -> Readability {
        self.0.probe_readable(path)
    }
}

#[tokio::test]
async fn summarizes_the_folder() {
    let summary = execute(
        Arc::new(mem()),
        proj(),
        env(),
        opts(),
        CancellationToken::new(),
        std::future::pending(),
    )
    .await
    .unwrap()
    .unwrap();
    // `node_modules` is a built-in exclusion.
    assert_eq!(summary.file_count, 2);
    assert_eq!(summary.dir_count, 1);
    assert_eq!(summary.total_bytes, 15);
    assert!(summary.markers.contains(&Marker::GitRepo));
    assert_eq!(
        finish(Ok(summary), &proj(), false).unwrap(),
        Status::Success
    );
}

#[tokio::test]
async fn interrupt_cancels_quickly() {
    let start = Instant::now();
    let result = execute(
        Arc::new(Endless(mem())),
        proj(),
        env(),
        opts(),
        CancellationToken::new(),
        tokio::time::sleep(Duration::from_millis(50)),
    )
    .await
    .unwrap();
    assert!(matches!(result, Err(FsError::Cancelled)), "{result:?}");
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(finish(result, &proj(), false).unwrap(), Status::Cancelled);
}

#[tokio::test]
async fn cancelled_token_is_cancelled() {
    let cancel = CancellationToken::new();
    cancel.cancel();
    let result = execute(
        Arc::new(mem()),
        proj(),
        env(),
        opts(),
        cancel,
        std::future::pending(),
    )
    .await
    .unwrap();
    assert!(matches!(result, Err(FsError::Cancelled)), "{result:?}");
}

#[tokio::test]
async fn missing_folder_and_file_are_errors() {
    for (dir, text) in [
        (Path::new(root()).join("missing"), "not found"),
        (proj().join("a.txt"), "is a file"),
    ] {
        let result = execute(
            Arc::new(mem()),
            dir.clone(),
            env(),
            opts(),
            CancellationToken::new(),
            std::future::pending(),
        )
        .await
        .unwrap();
        let error = finish(result, &dir, false).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains(&dir.display().to_string()), "{message}");
        assert!(message.contains(text), "{message}");
    }
}

#[tokio::test]
async fn render_compact_and_pretty() {
    let summary = execute(
        Arc::new(mem()),
        proj(),
        env(),
        opts(),
        CancellationToken::new(),
        std::future::pending(),
    )
    .await
    .unwrap()
    .unwrap();
    let compact = render(&summary, false).unwrap();
    assert!(compact.starts_with("{\"path\":"));
    assert_eq!(compact.matches('\n').count(), 1);
    let pretty = render(&summary, true).unwrap();
    assert!(pretty.starts_with("{\n  \"path\":"));
    assert!(pretty.ends_with("}\n"));
    for json in [&compact, &pretty] {
        let parsed: FolderSummary = serde_json::from_str(json).unwrap();
        assert_eq!(parsed, summary);
    }
}

#[test]
fn bad_exclude_globs_fall_back_to_builtin() {
    let env = env();
    let set = excludes(&env, &["[".to_owned()]);
    assert!(matches!(
        set.check(
            &proj().join("node_modules"),
            OsStr::new("node_modules"),
            true
        ),
        Exclusion::Exclude
    ));
    let set = excludes(&env, &["*.tmp".to_owned()]);
    assert!(matches!(
        set.check(&proj().join("a.tmp"), OsStr::new("a.tmp"), false),
        Exclusion::Exclude
    ));
}
