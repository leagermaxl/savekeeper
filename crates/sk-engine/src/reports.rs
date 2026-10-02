//! Saving reports to `scans/` and keeping the newest ones (SPEC-01 §4.4, §4.8.1).

use std::path::{Path, PathBuf};

use sk_core::model::ScanReport;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// Number of reports kept in `scans/`.
pub(crate) const KEEP_REPORTS: usize = 10;

/// Writes `<dir>/<scan_id>.json` atomically and removes all but the newest
/// [`KEEP_REPORTS`] reports by `finished_at`.
pub(crate) fn save(dir: &Path, report: &ScanReport) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.json", report.scan_id));
    let tmp = dir.join(format!("{}.json.tmp", report.scan_id));
    let json = serde_json::to_vec_pretty(report).map_err(std::io::Error::other)?;
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, &path)?;
    prune(dir)?;
    Ok(path)
}

/// `finished_at` of a saved report; `None` if the file is not a report.
fn finished_at(path: &Path) -> Option<OffsetDateTime> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    OffsetDateTime::parse(value.get("finished_at")?.as_str()?, &Rfc3339).ok()
}

fn prune(dir: &Path) -> std::io::Result<()> {
    let mut reports: Vec<(OffsetDateTime, PathBuf)> = std::fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .filter_map(|p| finished_at(&p).map(|t| (t, p)))
        .collect();
    reports.sort_by_key(|(finished, _)| std::cmp::Reverse(*finished));
    for (_, path) in reports.into_iter().skip(KEEP_REPORTS) {
        std::fs::remove_file(path)?;
    }
    Ok(())
}
