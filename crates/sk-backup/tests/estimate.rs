//! `estimate` on a real target folder (SPEC-10 §4.6, T-10-03): volume, write access,
//! stale `.partial` backups. Folders are created under `target/tmp`, never in the system
//! temp folder.

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sk_backup::{
    estimate, BackupFormat, BackupOptions, BackupRequest, BackupTarget, EstimateBlocker,
    EstimateWarning, FsKind, RetryPolicy,
};
use sk_core::env::Environment;
use sk_core::model::{
    Category, CollectorToggles, EnvironmentSnapshot, Finding, FindingId, LlmMode,
    ScanOptionsSnapshot, ScanReport, Sensitivity, Target, TargetStats, Totals,
};
use sk_core::template::PathTemplate;
use tempfile::TempDir;
use time::OffsetDateTime;
use uuid::Uuid;

fn temp_dir() -> TempDir {
    let base = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::create_dir_all(base).unwrap();
    tempfile::Builder::new()
        .prefix("sk-backup-estimate-")
        .tempdir_in(base)
        .unwrap()
}

fn file_set(resolved: &Path, total_bytes: u64) -> Finding {
    let target = Target::FileSet {
        root: PathTemplate::parse(r"{APPDATA}\Game").unwrap(),
        resolved: resolved.to_path_buf(),
        include: vec![],
        exclude: vec![],
    };
    Finding {
        id: FindingId::for_target(&target),
        target,
        category: Category::GameSave,
        app: None,
        title: "Game".to_owned(),
        evidence: vec![],
        stats: Some(TargetStats {
            total_bytes,
            file_count: 1,
            dir_count: 0,
            newest_mtime: None,
            oldest_mtime: None,
            locked_files: 0,
            cloud_only_bytes: 0,
            cloud_only_files: 0,
            largest_file_bytes: Some(total_bytes),
            truncated: false,
        }),
        sensitivity: Sensitivity::None,
        score: None,
        default_selected: true,
        requires_elevation: false,
        tags: vec![],
        children: vec![],
        notes_key: None,
    }
}

fn request(findings: Vec<Finding>, parent_dir: &Path) -> BackupRequest {
    let env = Environment::fake(parent_dir);
    let selection: BTreeSet<FindingId> = findings.iter().map(|f| f.id.clone()).collect();
    let report = ScanReport {
        schema_version: ScanReport::SCHEMA_VERSION,
        scan_id: Uuid::nil(),
        app_version: "0.1.0".to_owned(),
        started_at: OffsetDateTime::UNIX_EPOCH,
        finished_at: OffsetDateTime::UNIX_EPOCH,
        environment: EnvironmentSnapshot::from_env(&env),
        options: ScanOptionsSnapshot {
            roots: vec![],
            collectors: CollectorToggles::default(),
            llm: LlmMode::Off,
            max_depth: None,
        },
        findings,
        unknown_summaries: vec![],
        issues: vec![],
        totals: Totals::default(),
    };
    BackupRequest {
        report: Arc::new(report),
        selection,
        target: BackupTarget {
            parent_dir: parent_dir.to_path_buf(),
            name: "backup".to_owned(),
        },
        options: BackupOptions {
            format: BackupFormat::Zip,
            compression_level: 6,
            encryption: None,
            verify: true,
            keep_partial_on_cancel: false,
            retry_locked: RetryPolicy::default(),
        },
    }
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn writable_folder_has_no_blockers_and_no_leftovers() {
    let dir = temp_dir();
    let source = dir.path().join("source");
    let target = dir.path().join("target");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&target).unwrap();

    let est = estimate(&request(vec![file_set(&source, 1000)], &target)).unwrap();
    assert!(est.blockers.is_empty(), "{:?}", est.blockers);
    assert_eq!(est.total_bytes, 1000);
    assert_eq!(est.estimated_archive_bytes, 600);
    assert!(est.free_bytes > 0);
    assert!(
        names(&target).is_empty(),
        "probe file left: {:?}",
        names(&target)
    );
}

/// Free space and file system come from `GetDiskFreeSpaceExW` / `GetVolumeInformationW`.
#[cfg(windows)]
#[test]
fn windows_free_space_and_file_system_of_the_target() {
    let dir = temp_dir();
    let est = estimate(&request(vec![], dir.path())).unwrap();
    assert!(est.free_bytes > 0 && est.free_bytes < u64::MAX, "{est:?}");
    assert_ne!(est.fs_kind, FsKind::Other(String::new()), "{est:?}");
    assert!(est.blockers.is_empty(), "{:?}", est.blockers);

    // Not enough space: require more than the volume has.
    let free = est.free_bytes;
    let huge = file_set(&dir.path().join("src"), free.saturating_mul(4));
    let est = estimate(&request(vec![huge], dir.path())).unwrap();
    assert!(
        est.blockers
            .iter()
            .any(|b| matches!(b, EstimateBlocker::InsufficientSpace { required_bytes } if *required_bytes > free)),
        "{:?}",
        est.blockers
    );
}

#[test]
fn missing_folder_is_not_writable() {
    let dir = temp_dir();
    let est = estimate(&request(vec![], &dir.path().join("missing"))).unwrap();
    assert_eq!(est.blockers, [EstimateBlocker::NotWritable]);
    assert_eq!(est.free_bytes, 0);
}

#[test]
fn target_inside_a_selected_source() {
    let dir = temp_dir();
    let source = dir.path().join("source");
    let target = source.join("backups");
    fs::create_dir_all(&target).unwrap();
    let finding = file_set(&source, 10);
    let est = estimate(&request(vec![finding.clone()], &target)).unwrap();
    assert_eq!(
        est.blockers,
        [EstimateBlocker::TargetInsideSource {
            finding: finding.id
        }]
    );
}

#[test]
fn stale_partial_backups_are_reported() {
    let dir = temp_dir();
    let target = dir.path();
    fs::write(target.join("SaveKeeper-PC-20260101-1200.zip.partial"), b"x").unwrap();
    fs::write(target.join("other.zip.age.partial"), b"x").unwrap();
    fs::create_dir(target.join("folder.partial")).unwrap();
    fs::write(target.join("folder.partial").join("entries.jsonl"), b"").unwrap();
    // Not backups: an empty `.partial` folder and an unrelated `.partial` file.
    fs::create_dir(target.join("empty.partial")).unwrap();
    fs::write(target.join("download.partial"), b"x").unwrap();

    let est = estimate(&request(vec![], target)).unwrap();
    let stale: Vec<PathBuf> = est
        .warnings
        .iter()
        .filter_map(|w| match w {
            EstimateWarning::StalePartial { path } => Some(path.clone()),
            _ => None,
        })
        .collect();
    let mut expected = vec![
        target.join("SaveKeeper-PC-20260101-1200.zip.partial"),
        target.join("folder.partial"),
        target.join("other.zip.age.partial"),
    ];
    expected.sort();
    assert_eq!(stale, expected);
}
