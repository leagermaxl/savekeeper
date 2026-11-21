//! Preliminary check of a backup: size, free space, file system limits (FR-10-03, §4.6).
//!
//! [`estimate`] inspects the target folder once ([`TargetProbe`]) and then applies the
//! rules of §4.6 in [`evaluate`], which is pure and tested without a real disk.

use std::collections::BTreeSet;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use sk_core::config::BackupFormat;
use sk_core::fs::{volume_info, VolumeInfo};
use sk_core::model::{Category, Finding, FindingId, Sensitivity, Target};
use sk_core::path::{starts_with_ci, to_extended};

use crate::error::BackupError;
use crate::types::{
    BackupEstimate, BackupRequest, BackupTarget, EstimateBlocker, EstimateWarning, FsKind,
    FsLimitSubject,
};

const MIB: u64 = 1 << 20;
const GIB: u64 = 1 << 30;
/// Space reserved on top of the backup itself (§4.6).
const SPACE_RESERVE: u64 = 50 * MIB;
/// Largest file FAT32 accepts: 4 GiB − 1 (FR-10-04).
pub(crate) const FAT32_MAX_FILE: u64 = 4 * GIB - 1;
/// A file above this size needs explicit confirmation (§5).
pub(crate) const HUGE_FILE_BYTES: u64 = 50 * GIB;
/// A selection of at least this size gets [`EstimateWarning::LargeSelection`].
pub(crate) const LARGE_SELECTION_BYTES: u64 = 100 * GIB;
/// Assumed size of a registry or system export without stats (FR-10-13: unknown until run).
pub(crate) const EXPORT_ESTIMATE_BYTES: u64 = MIB;

/// Preliminary check of a backup before it starts (FR-10-03, §4.6).
///
/// Reads the target folder (volume, write access, stale `.partial`) and the stats of the
/// selected findings; selected ids that are not in the report are ignored. A missing
/// target folder is the blocker [`EstimateBlocker::NotWritable`]; a folder whose volume
/// cannot be queried is [`BackupError::Target`].
pub fn estimate(req: &BackupRequest) -> Result<BackupEstimate, BackupError> {
    let probe = TargetProbe::inspect(&req.target)?;
    Ok(evaluate(req, &probe))
}

/// What [`estimate`] learns about the target folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TargetProbe {
    /// Volume of the folder; `None` if the folder does not exist.
    pub volume: Option<VolumeInfo>,
    /// A file can be created in the folder.
    pub writable: bool,
    /// `.partial` backups left in the folder (§5).
    pub stale_partials: Vec<PathBuf>,
}

impl TargetProbe {
    fn inspect(target: &BackupTarget) -> Result<Self, BackupError> {
        let dir = &target.parent_dir;
        let volume = match volume_info(dir) {
            Ok(volume) => Some(volume),
            Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => None,
            Err(source) => {
                return Err(BackupError::Target {
                    path: dir.clone(),
                    source,
                })
            }
        };
        let writable = volume.is_some() && can_create_file(dir);
        let stale_partials = if volume.is_some() {
            stale_partials(dir)
        } else {
            Vec::new()
        };
        Ok(Self {
            volume,
            writable,
            stale_partials,
        })
    }
}

/// Creates and removes an empty probe file in `dir`.
fn can_create_file(dir: &Path) -> bool {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let probe = to_extended(&dir.join(format!(
        ".savekeeper-write-test-{}-{n}.tmp",
        std::process::id()
    )));
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(file) => {
            drop(file);
            // Only our own empty probe file; a leftover is harmless.
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// `.partial` backups in `dir`: files `*.zip.partial` and `*.zip.age.partial`, and
/// folders `*.partial` that contain `manifest.json`, `entries.jsonl` or `files/`.
fn stale_partials(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(to_extended(dir)) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if !name.ends_with(".partial") {
                return None;
            }
            let kind = entry.file_type().ok()?;
            let is_backup = if kind.is_file() {
                name.ends_with(".zip.partial") || name.ends_with(".zip.age.partial")
            } else if kind.is_dir() {
                ["manifest.json", "entries.jsonl", "files"]
                    .iter()
                    .any(|marker| entry.path().join(marker).exists())
            } else {
                false
            };
            is_backup.then(|| dir.join(entry.file_name()))
        })
        .collect();
    found.sort();
    found
}

/// Applies the rules of §4.6 to the request and the inspected target.
pub(crate) fn evaluate(req: &BackupRequest, probe: &TargetProbe) -> BackupEstimate {
    let selected = selected_findings(req);
    let counted = without_nested(&selected);
    let sizes = Sizes::of(&counted, req.options.compression_level);
    let encrypted = req.options.encryption.is_some();
    let zip = req.options.format == BackupFormat::Zip;

    let mut blockers = Vec::new();
    if !zip && encrypted {
        blockers.push(EstimateBlocker::EncryptWithDir);
    }
    if !probe.writable {
        blockers.push(EstimateBlocker::NotWritable);
    }
    let parent = std::path::absolute(&req.target.parent_dir)
        .unwrap_or_else(|_| req.target.parent_dir.clone());
    for finding in &counted {
        if let Target::FileSet { resolved, .. } = &finding.target {
            if starts_with_ci(&parent, resolved) {
                blockers.push(EstimateBlocker::TargetInsideSource {
                    finding: finding.id.clone(),
                });
            }
        }
    }

    let fs_kind = probe.volume.as_ref().map_or_else(unknown_fs, fs_kind);
    let free_bytes = probe.volume.as_ref().map_or(0, |v| v.free_bytes);
    if probe.volume.is_some() {
        let required = required_bytes(&sizes, zip, encrypted);
        if free_bytes < required {
            blockers.push(EstimateBlocker::InsufficientSpace {
                required_bytes: required,
            });
        }
        if fs_kind == FsKind::Fat32 {
            if let Some(largest) = sizes.largest_file.filter(|&b| b > FAT32_MAX_FILE) {
                blockers.push(EstimateBlocker::FsLimit {
                    subject: FsLimitSubject::File,
                    bytes: largest,
                    max_bytes: FAT32_MAX_FILE,
                });
            }
            if zip && sizes.archive_bytes > FAT32_MAX_FILE {
                blockers.push(EstimateBlocker::FsLimit {
                    subject: FsLimitSubject::Archive,
                    bytes: sizes.archive_bytes,
                    max_bytes: FAT32_MAX_FILE,
                });
            }
        }
    }

    let mut warnings = Vec::new();
    if probe.volume.as_ref().is_some_and(|v| v.system) {
        warnings.push(EstimateWarning::TargetOnSystemDrive);
    }
    warnings.extend(
        probe
            .stale_partials
            .iter()
            .map(|path| EstimateWarning::StalePartial { path: path.clone() }),
    );
    let sensitive = selected
        .iter()
        .filter(|f| f.sensitivity == Sensitivity::High || f.category == Category::Credentials)
        .count();
    if sensitive > 0 && !encrypted {
        warnings.push(EstimateWarning::Sensitive {
            count: u32::try_from(sensitive).unwrap_or(u32::MAX),
        });
    }
    if !req.report.environment.is_elevated {
        let ids: Vec<FindingId> = selected
            .iter()
            .filter(|f| f.requires_elevation)
            .map(|f| f.id.clone())
            .collect();
        if !ids.is_empty() {
            warnings.push(EstimateWarning::RequiresElevation { ids });
        }
    }
    if !sizes.unknown.is_empty() {
        warnings.push(EstimateWarning::UnknownSize {
            ids: sizes.unknown.clone(),
        });
    }
    warnings.extend(
        sizes
            .huge
            .iter()
            .map(|(finding, bytes)| EstimateWarning::HugeFile {
                finding: finding.clone(),
                bytes: *bytes,
            }),
    );
    if sizes.total_bytes >= LARGE_SELECTION_BYTES {
        warnings.push(EstimateWarning::LargeSelection);
    }

    BackupEstimate {
        total_bytes: sizes.total_bytes,
        file_count: sizes.file_count,
        estimated_archive_bytes: sizes.archive_bytes,
        free_bytes,
        fs_kind,
        largest_file_bytes: sizes.largest_file,
        blockers,
        warnings,
    }
}

/// Selected findings in report order.
fn selected_findings(req: &BackupRequest) -> Vec<&Finding> {
    req.report
        .findings
        .iter()
        .filter(|f| req.selection.contains(&f.id))
        .collect()
}

/// Drops nested findings (tag `nested`, SPEC-09 FR-09-02) whose parent is also selected:
/// the parent's stats already include their files. Alternatives stay.
fn without_nested<'a>(selected: &[&'a Finding]) -> Vec<&'a Finding> {
    let ids: BTreeSet<&FindingId> = selected.iter().map(|f| &f.id).collect();
    let covered: BTreeSet<&FindingId> = selected
        .iter()
        .flat_map(|f| f.children.iter())
        .filter(|child| ids.contains(child))
        .collect();
    selected
        .iter()
        .filter(|f| !(covered.contains(&f.id) && f.tags.iter().any(|t| t == "nested")))
        .copied()
        .collect()
}

/// Sizes of the counted selection.
#[derive(Debug, Default)]
struct Sizes {
    total_bytes: u64,
    file_count: u64,
    archive_bytes: u64,
    largest_file: Option<u64>,
    unknown: Vec<FindingId>,
    huge: Vec<(FindingId, u64)>,
}

impl Sizes {
    fn of(findings: &[&Finding], compression_level: u8) -> Self {
        let mut sizes = Self::default();
        for finding in findings {
            let (bytes, files, largest) = match (&finding.stats, &finding.target) {
                (Some(stats), target) => {
                    let largest = stats.largest_file_bytes.or_else(|| {
                        matches!(target, Target::File { .. }).then_some(stats.total_bytes)
                    });
                    (stats.total_bytes, stats.file_count, largest)
                }
                (None, Target::Registry { .. } | Target::SystemExport { .. }) => {
                    (EXPORT_ESTIMATE_BYTES, 1, None)
                }
                (None, Target::FileSet { .. } | Target::File { .. }) => {
                    sizes.unknown.push(finding.id.clone());
                    continue;
                }
            };
            sizes.total_bytes = sizes.total_bytes.saturating_add(bytes);
            sizes.file_count = sizes.file_count.saturating_add(files);
            let ratio = compression_permille(finding.category, compression_level);
            sizes.archive_bytes = sizes
                .archive_bytes
                .saturating_add(scale(bytes, ratio, 1000));
            if let Some(largest) = largest {
                sizes.largest_file = Some(sizes.largest_file.map_or(largest, |l| l.max(largest)));
                if largest > HUGE_FILE_BYTES {
                    sizes.huge.push((finding.id.clone(), largest));
                }
            }
        }
        sizes
    }
}

/// Compression ratio of a category in thousandths (§4.6); level 0 stores, ratio 1.
pub(crate) fn compression_permille(category: Category, compression_level: u8) -> u64 {
    if compression_level == 0 {
        return 1000;
    }
    match category {
        Category::GameSave => 600,
        Category::AppConfig => 300,
        Category::AppData => 700,
        Category::UserFiles => 950,
        Category::DevEnvironment => 500,
        _ => 800,
    }
}

/// Free space the backup needs (§4.6): zip `archive × 1.1 + 50 MiB`, dir
/// `total × 1.05 + 50 MiB`; with encryption the zip and the `.age` coexist, `× 2`.
fn required_bytes(sizes: &Sizes, zip: bool, encrypted: bool) -> u64 {
    let base = if zip {
        scale(sizes.archive_bytes, 11, 10)
    } else {
        scale(sizes.total_bytes, 105, 100)
    }
    .saturating_add(SPACE_RESERVE);
    if zip && encrypted {
        base.saturating_mul(2)
    } else {
        base
    }
}

/// `value × num / den`, rounded up, saturating at `u64::MAX`.
fn scale(value: u64, num: u64, den: u64) -> u64 {
    let scaled = (u128::from(value) * u128::from(num)).div_ceil(u128::from(den));
    u64::try_from(scaled).unwrap_or(u64::MAX)
}

fn fs_kind(volume: &VolumeInfo) -> FsKind {
    if volume.remote {
        return FsKind::Network;
    }
    let Some(name) = volume.fs_name.as_deref() else {
        return unknown_fs();
    };
    match name.to_ascii_uppercase().as_str() {
        "NTFS" => FsKind::Ntfs,
        "EXFAT" => FsKind::ExFat,
        "FAT32" => FsKind::Fat32,
        "REFS" => FsKind::ReFs,
        _ => FsKind::Other(name.to_owned()),
    }
}

/// File system that could not be determined.
fn unknown_fs() -> FsKind {
    FsKind::Other(String::new())
}

#[cfg(test)]
#[path = "estimate_tests.rs"]
mod tests;
