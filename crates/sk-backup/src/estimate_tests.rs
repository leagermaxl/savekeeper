//! Rules of `evaluate` (SPEC-10 §4.6, §6): blockers, warnings and sizes on a fake target.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sk_core::config::BackupFormat;
use sk_core::env::Environment;
use sk_core::fs::VolumeInfo;
use sk_core::model::{
    Category, CollectorToggles, EnvironmentSnapshot, Finding, FindingId, LlmMode, RegHive,
    ScanOptionsSnapshot, ScanReport, Sensitivity, Target, TargetStats, Totals,
};
use sk_core::template::PathTemplate;
use time::OffsetDateTime;
use uuid::Uuid;

use super::*;
use crate::types::{BackupOptions, BackupRequest, BackupTarget, Passphrase, RetryPolicy};

fn root() -> PathBuf {
    PathBuf::from(if cfg!(windows) { r"D:\fake" } else { "/fake" })
}

fn stats(total_bytes: u64, file_count: u64, largest: Option<u64>) -> TargetStats {
    TargetStats {
        total_bytes,
        file_count,
        dir_count: 0,
        newest_mtime: None,
        oldest_mtime: None,
        locked_files: 0,
        cloud_only_bytes: 0,
        cloud_only_files: 0,
        largest_file_bytes: largest,
        truncated: false,
    }
}

fn finding(target: Target, category: Category, stats: Option<TargetStats>) -> Finding {
    Finding {
        id: FindingId::for_target(&target),
        target,
        category,
        app: None,
        title: "test".to_owned(),
        evidence: vec![],
        stats,
        sensitivity: Sensitivity::None,
        score: None,
        default_selected: true,
        requires_elevation: false,
        tags: vec![],
        children: vec![],
        notes_key: None,
    }
}

fn file_set(rel: &str, category: Category, stats: Option<TargetStats>) -> Finding {
    let target = Target::FileSet {
        root: PathTemplate::parse(&format!("{{DRIVE:D}}\\{rel}")).unwrap(),
        resolved: root().join(rel),
        include: vec![],
        exclude: vec![],
    };
    finding(target, category, stats)
}

fn registry(key: &str) -> Finding {
    let target = Target::Registry {
        hive: RegHive::Hkcu,
        key: key.to_owned(),
        recursive: true,
    };
    finding(target, Category::AppConfig, None)
}

fn request(findings: Vec<Finding>, format: BackupFormat, parent_dir: &Path) -> BackupRequest {
    let env = Environment::fake(&root());
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
    let selection: BTreeSet<FindingId> = report.findings.iter().map(|f| f.id.clone()).collect();
    BackupRequest {
        report: Arc::new(report),
        selection,
        target: BackupTarget {
            parent_dir: parent_dir.to_path_buf(),
            name: "backup".to_owned(),
        },
        options: BackupOptions {
            format,
            compression_level: 6,
            encryption: None,
            verify: true,
            keep_partial_on_cancel: false,
            retry_locked: RetryPolicy::default(),
        },
    }
}

fn probe(fs: &str, free_bytes: u64) -> TargetProbe {
    TargetProbe {
        volume: Some(VolumeInfo {
            root: PathBuf::from(if cfg!(windows) { r"E:\" } else { "/" }),
            fs_name: Some(fs.to_owned()),
            remote: false,
            free_bytes,
            system: false,
        }),
        writable: true,
        stale_partials: vec![],
    }
}

fn target_dir() -> PathBuf {
    PathBuf::from(if cfg!(windows) {
        r"E:\backups"
    } else {
        "/backups"
    })
}

const PLENTY: u64 = 1 << 50;

#[test]
fn sizes_and_compression_ratios_by_category() {
    let findings = vec![
        file_set("saves", Category::GameSave, Some(stats(1000, 2, Some(600)))),
        file_set(
            "config",
            Category::AppConfig,
            Some(stats(1000, 3, Some(10))),
        ),
        file_set("user", Category::UserFiles, Some(stats(1000, 1, None))),
        file_set("other", Category::Unknown, Some(stats(1000, 1, None))),
    ];
    let req = request(findings, BackupFormat::Zip, &target_dir());
    let est = evaluate(&req, &probe("NTFS", PLENTY));
    assert_eq!(est.total_bytes, 4000);
    assert_eq!(est.file_count, 7);
    assert_eq!(est.estimated_archive_bytes, 600 + 300 + 950 + 800);
    assert_eq!(est.largest_file_bytes, Some(600));
    assert_eq!(est.fs_kind, FsKind::Ntfs);
    assert_eq!(est.free_bytes, PLENTY);
    assert!(est.blockers.is_empty(), "{:?}", est.blockers);
    assert!(est.warnings.is_empty(), "{:?}", est.warnings);
}

#[test]
fn level_zero_stores_without_compression() {
    let findings = vec![file_set(
        "config",
        Category::AppConfig,
        Some(stats(1000, 1, None)),
    )];
    let mut req = request(findings, BackupFormat::Zip, &target_dir());
    req.options.compression_level = 0;
    assert_eq!(
        evaluate(&req, &probe("NTFS", PLENTY)).estimated_archive_bytes,
        1000
    );
}

#[test]
fn insufficient_space_for_zip_dir_and_encryption() {
    let gib = 1u64 << 30;
    let reserve = 50 << 20;
    // AppData: archive = 0.7 × 10 GiB = 7 GiB.
    let findings = vec![file_set(
        "data",
        Category::AppData,
        Some(stats(10 * gib, 1, None)),
    )];
    let archive = (10 * gib * 7).div_ceil(10);
    let zip_required = (archive * 11).div_ceil(10) + reserve;
    let dir_required = (10 * gib * 105).div_ceil(100) + reserve;

    let mut req = request(findings, BackupFormat::Zip, &target_dir());
    let blocked = |req: &BackupRequest, free| {
        evaluate(req, &probe("NTFS", free))
            .blockers
            .into_iter()
            .find(|b| matches!(b, EstimateBlocker::InsufficientSpace { .. }))
    };
    assert_eq!(blocked(&req, zip_required), None);
    assert_eq!(
        blocked(&req, zip_required - 1),
        Some(EstimateBlocker::InsufficientSpace {
            required_bytes: zip_required
        })
    );

    req.options.encryption = Some(Passphrase::from("long enough"));
    assert_eq!(
        blocked(&req, zip_required),
        Some(EstimateBlocker::InsufficientSpace {
            required_bytes: zip_required * 2
        })
    );

    req.options.encryption = None;
    req.options.format = BackupFormat::Dir;
    assert_eq!(blocked(&req, dir_required), None);
    assert_eq!(
        blocked(&req, dir_required - 1),
        Some(EstimateBlocker::InsufficientSpace {
            required_bytes: dir_required
        })
    );
}

#[test]
fn fat32_limits_file_and_archive() {
    let gib = 1u64 << 30;
    let big_file = file_set(
        "video",
        Category::UserFiles,
        Some(stats(5 * gib, 1, Some(5 * gib))),
    );
    let req = request(vec![big_file], BackupFormat::Zip, &target_dir());
    let est = evaluate(&req, &probe("FAT32", PLENTY));
    assert_eq!(est.fs_kind, FsKind::Fat32);
    assert_eq!(
        est.blockers,
        [
            EstimateBlocker::FsLimit {
                subject: FsLimitSubject::File,
                bytes: 5 * gib,
                max_bytes: FAT32_MAX_FILE,
            },
            EstimateBlocker::FsLimit {
                subject: FsLimitSubject::Archive,
                bytes: (5 * gib * 95).div_ceil(100),
                max_bytes: FAT32_MAX_FILE,
            },
        ]
    );

    // Many small files: the archive is too large for zip, a folder is fine.
    let many = file_set(
        "photos",
        Category::UserFiles,
        Some(stats(6 * gib, 6000, Some(10 << 20))),
    );
    let mut req = request(vec![many], BackupFormat::Zip, &target_dir());
    let est = evaluate(&req, &probe("FAT32", PLENTY));
    assert!(matches!(
        est.blockers.as_slice(),
        [EstimateBlocker::FsLimit {
            subject: FsLimitSubject::Archive,
            ..
        }]
    ));
    req.options.format = BackupFormat::Dir;
    assert!(evaluate(&req, &probe("FAT32", PLENTY)).blockers.is_empty());
    // exFAT has no such limit.
    req.options.format = BackupFormat::Zip;
    let est = evaluate(&req, &probe("exFAT", PLENTY));
    assert_eq!(est.fs_kind, FsKind::ExFat);
    assert!(est.blockers.is_empty());
}

#[test]
fn target_inside_selected_file_set() {
    let saves = file_set("saves", Category::GameSave, Some(stats(10, 1, None)));
    let other = file_set("other", Category::GameSave, Some(stats(10, 1, None)));
    let inside = root().join("SAVES").join("backups");
    let req = request(vec![saves.clone(), other], BackupFormat::Zip, &inside);
    assert_eq!(
        evaluate(&req, &probe("NTFS", PLENTY)).blockers,
        [EstimateBlocker::TargetInsideSource {
            finding: saves.id.clone()
        }]
    );
    // The root itself counts; a sibling with a common prefix does not.
    let req = request(
        vec![saves.clone()],
        BackupFormat::Zip,
        &root().join("saves"),
    );
    assert_eq!(evaluate(&req, &probe("NTFS", PLENTY)).blockers.len(), 1);
    let req = request(vec![saves], BackupFormat::Zip, &root().join("saves2"));
    assert!(evaluate(&req, &probe("NTFS", PLENTY)).blockers.is_empty());
}

#[test]
fn encrypt_with_dir_and_not_writable() {
    let findings = vec![file_set("a", Category::AppData, Some(stats(1, 1, None)))];
    let mut req = request(findings, BackupFormat::Dir, &target_dir());
    req.options.encryption = Some(Passphrase::from("long enough"));
    let mut target = probe("NTFS", PLENTY);
    target.writable = false;
    assert_eq!(
        evaluate(&req, &target).blockers,
        [
            EstimateBlocker::EncryptWithDir,
            EstimateBlocker::NotWritable
        ]
    );

    // A missing folder: not writable, volume unknown, no space checks.
    let missing = TargetProbe {
        volume: None,
        writable: false,
        stale_partials: vec![],
    };
    let est = evaluate(&req, &missing);
    assert_eq!(est.free_bytes, 0);
    assert_eq!(est.fs_kind, FsKind::Other(String::new()));
    assert_eq!(
        est.blockers,
        [
            EstimateBlocker::EncryptWithDir,
            EstimateBlocker::NotWritable
        ]
    );
}

#[test]
fn nested_children_are_counted_once_alternatives_are_not_dropped() {
    let mut parent = file_set("dev", Category::DevEnvironment, Some(stats(1000, 10, None)));
    let mut nested = file_set(
        "dev/sub",
        Category::DevEnvironment,
        Some(stats(400, 4, None)),
    );
    nested.tags.push("nested".to_owned());
    let alternative = finding(
        Target::SystemExport {
            exporter_id: "git-bundle".to_owned(),
            params: serde_json::json!({ "repo": "x" }),
        },
        Category::DevEnvironment,
        Some(stats(200, 1, None)),
    );
    parent.children = vec![nested.id.clone(), alternative.id.clone()];
    let req = request(
        vec![parent.clone(), nested.clone(), alternative],
        BackupFormat::Zip,
        &target_dir(),
    );
    let est = evaluate(&req, &probe("NTFS", PLENTY));
    assert_eq!(est.total_bytes, 1200);
    assert_eq!(est.file_count, 11);

    // Without its parent the nested finding is counted.
    let req = request(vec![nested], BackupFormat::Zip, &target_dir());
    assert_eq!(evaluate(&req, &probe("NTFS", PLENTY)).total_bytes, 400);
}

#[test]
fn exports_without_stats_get_a_default_size_file_sets_are_unknown() {
    let reg = registry(r"Software\Test");
    let unknown = file_set("unknown", Category::AppData, None);
    let req = request(vec![reg, unknown.clone()], BackupFormat::Zip, &target_dir());
    let est = evaluate(&req, &probe("NTFS", PLENTY));
    assert_eq!(est.total_bytes, EXPORT_ESTIMATE_BYTES);
    assert_eq!(est.file_count, 1);
    assert_eq!(
        est.warnings,
        [EstimateWarning::UnknownSize {
            ids: vec![unknown.id]
        }]
    );
}

#[test]
fn warnings_sensitive_elevation_system_drive_stale_huge_large() {
    let gib = 1u64 << 30;
    let mut keys = file_set("keys", Category::Credentials, Some(stats(10, 1, None)));
    keys.requires_elevation = true;
    let mut high = file_set("high", Category::AppData, Some(stats(10, 1, None)));
    high.sensitivity = Sensitivity::High;
    let vhd = finding(
        Target::File {
            path: PathTemplate::parse(r"{DRIVE:D}\disk.vhdx").unwrap(),
            resolved: root().join("disk.vhdx"),
        },
        Category::UserFiles,
        Some(stats(120 * gib, 1, None)),
    );
    let mut req = request(
        vec![keys.clone(), high, vhd.clone()],
        BackupFormat::Dir,
        &target_dir(),
    );
    let mut target = probe("NTFS", PLENTY);
    target.volume.as_mut().unwrap().system = true;
    let stale = target_dir().join("old.zip.partial");
    target.stale_partials = vec![stale.clone()];

    let est = evaluate(&req, &target);
    assert_eq!(est.largest_file_bytes, Some(120 * gib));
    assert_eq!(
        est.warnings,
        [
            EstimateWarning::TargetOnSystemDrive,
            EstimateWarning::StalePartial { path: stale },
            EstimateWarning::Sensitive { count: 2 },
            EstimateWarning::RequiresElevation {
                ids: vec![keys.id.clone()]
            },
            EstimateWarning::HugeFile {
                finding: vhd.id,
                bytes: 120 * gib
            },
            EstimateWarning::LargeSelection,
        ]
    );

    // Encryption answers the sensitivity warning; an elevated scan needs no elevation.
    req.options.format = BackupFormat::Zip;
    req.options.encryption = Some(Passphrase::from("long enough"));
    let mut report = (*req.report).clone();
    report.environment.is_elevated = true;
    req.report = Arc::new(report);
    let est = evaluate(&req, &target);
    assert!(!est.warnings.iter().any(|w| matches!(
        w,
        EstimateWarning::Sensitive { .. } | EstimateWarning::RequiresElevation { .. }
    )));
}

#[test]
fn file_system_names_map_to_kinds() {
    let kind = |name: Option<&str>, remote| {
        fs_kind(&VolumeInfo {
            root: PathBuf::new(),
            fs_name: name.map(str::to_owned),
            remote,
            free_bytes: 0,
            system: false,
        })
    };
    assert_eq!(kind(Some("NTFS"), false), FsKind::Ntfs);
    assert_eq!(kind(Some("exFAT"), false), FsKind::ExFat);
    assert_eq!(kind(Some("FAT32"), false), FsKind::Fat32);
    assert_eq!(kind(Some("ReFS"), false), FsKind::ReFs);
    assert_eq!(kind(Some("NTFS"), true), FsKind::Network);
    assert_eq!(kind(Some("FAT"), false), FsKind::Other("FAT".to_owned()));
    assert_eq!(kind(None, false), FsKind::Other(String::new()));
}
