//! The `disks-info` exporter (SPEC-06 §4.3): the logical drives of `Environment.drives`
//! with their BitLocker status as `disks.json`, for the checklist (§4.7) and for
//! finding the drives again after the reinstall (SPEC-13).
//!
//! The BitLocker status is read from WMI `Win32_EncryptableVolume`, which needs
//! administrator rights: without them every drive is `"unknown"` and the result carries
//! a warning (the export degrades instead of asking for UAC, SPEC-06 T-06-12).

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use sk_core::env::{DriveInfo, Environment};
use sk_core::model::{Category, Finding, ScanIssue, Sensitivity};

use super::{plan_finding, unavailable, warning, write_file, Plan, NOT_WINDOWS};
use crate::error::ExportError;
use crate::exporter::{Availability, ExportContext, ExportResult, RestoreHint, SystemExporter};
use crate::wql::{int, query_async, text, SystemWmi, WmiError, WmiRow, WmiSource};

/// Exporter identifier.
pub(crate) const ID: &str = "disks-info";

/// The exported file, in `system/disks-info/`.
pub(crate) const FILE: &str = "disks.json";

/// WMI namespace of the BitLocker provider.
pub(crate) const BITLOCKER_NAMESPACE: &str = r"ROOT\CIMV2\Security\MicrosoftVolumeEncryption";

/// The BitLocker query of `run` (administrator rights only).
pub(crate) const BITLOCKER_QUERY: &str =
    "SELECT DriveLetter, ProtectionStatus, ConversionStatus FROM Win32_EncryptableVolume";

/// Finding title (i18n key).
const TITLE: &str = "system.title.disks_info";

/// Warning: without administrator rights the BitLocker status is unknown.
pub(crate) const ISSUE_BITLOCKER_NEEDS_ADMIN: &str = "issue.system.bitlocker_needs_admin";

/// Warning: the BitLocker status could not be read. Args: `error`.
pub(crate) const ISSUE_BITLOCKER_FAILED: &str = "issue.system.bitlocker_query_failed";

/// Timeout of the BitLocker query.
const RUN_TIMEOUT: Duration = Duration::from_secs(60);

/// Size estimate of one drive in `disks.json`.
const ESTIMATE_BYTES_PER_DRIVE: u64 = 512;

/// BitLocker status of a drive in `disks.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BitLocker {
    /// Encrypted and protected (`ProtectionStatus = 1`).
    On,
    /// Encrypted (fully or partly) but not protected: suspended, waiting for activation
    /// of device encryption, conversion in progress (`ProtectionStatus = 0`,
    /// `ConversionStatus ≠ 0`).
    Suspended,
    /// Not encrypted (`ProtectionStatus = 0`, `ConversionStatus = 0`).
    Off,
    /// Not known: no administrator rights, a failed query, a locked volume, a drive the
    /// provider does not list.
    Unknown,
}

impl BitLocker {
    /// The value written to `disks.json`.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            BitLocker::On => "on",
            BitLocker::Suspended => "suspended",
            BitLocker::Off => "off",
            BitLocker::Unknown => "unknown",
        }
    }

    /// The status of one `Win32_EncryptableVolume` object.
    pub(crate) fn of(row: &WmiRow) -> Self {
        match (int(row, "ProtectionStatus"), int(row, "ConversionStatus")) {
            (Some(1), _) => BitLocker::On,
            (Some(0), Some(0)) => BitLocker::Off,
            (Some(0), Some(_)) => BitLocker::Suspended,
            _ => BitLocker::Unknown,
        }
    }
}

/// The drives with their BitLocker status.
pub(crate) struct DisksInfoExporter {
    wmi: Arc<dyn WmiSource>,
}

impl DisksInfoExporter {
    /// The exporter with the WMI of this machine.
    pub(crate) fn new() -> Self {
        Self {
            wmi: Arc::new(SystemWmi),
        }
    }

    /// The exporter with the given WMI source (tests).
    #[cfg(test)]
    pub(crate) fn with_wmi(wmi: Arc<dyn WmiSource>) -> Self {
        Self { wmi }
    }

    /// `detect` without the platform check: always available, `details.drives` = the
    /// number of drives. No WMI query: BitLocker is read only by `run`.
    pub(crate) fn probe(env: &Environment) -> Availability {
        let drives = env.drives.len();
        Availability::Available {
            estimated_bytes: Some((drives as u64).max(1) * ESTIMATE_BYTES_PER_DRIVE),
            details: BTreeMap::from([("drives".to_owned(), drives.to_string())]),
        }
    }

    /// BitLocker status by uppercase drive letter, and the warning when it is unknown.
    /// Without administrator rights WMI is not asked at all.
    async fn bitlocker(
        &self,
        ctx: &ExportContext<'_>,
    ) -> Result<(BTreeMap<char, BitLocker>, Option<ScanIssue>), ExportError> {
        if !(ctx.elevated || ctx.env.is_elevated) {
            let issue = warning(ISSUE_BITLOCKER_NEEDS_ADMIN, BTreeMap::new());
            return Ok((BTreeMap::new(), Some(issue)));
        }
        let query = query_async(
            self.wmi.clone(),
            BITLOCKER_NAMESPACE,
            BITLOCKER_QUERY,
            RUN_TIMEOUT,
            ctx.cancel,
        );
        match query.await {
            Ok(rows) => Ok((statuses(&rows), None)),
            Err(WmiError::Cancelled) => Err(ExportError::Cancelled),
            Err(err) => {
                tracing::debug!(error = %err, "BitLocker status not read");
                let args = BTreeMap::from([("error".to_owned(), err.to_string())]);
                Ok((BTreeMap::new(), Some(warning(ISSUE_BITLOCKER_FAILED, args))))
            }
        }
    }
}

#[async_trait]
impl SystemExporter for DisksInfoExporter {
    fn id(&self) -> &'static str {
        ID
    }

    /// Always available on Windows; `system.not_windows` elsewhere.
    fn detect(&self, env: &Environment) -> Availability {
        if cfg!(windows) {
            Self::probe(env)
        } else {
            unavailable(NOT_WINDOWS)
        }
    }

    fn plan(&self, _env: &Environment, avail: &Availability) -> Option<Finding> {
        let plan = Plan {
            exporter_id: ID,
            title: TITLE,
            category: Category::SystemSettings,
            sensitivity: Sensitivity::None,
            params: serde_json::json!({}),
        };
        plan_finding(plan, avail)
    }

    /// Writes `disks.json`: `{ "drives": [{ <DriveInfo fields>, "bitlocker" }] }` in the
    /// order of `Environment.drives`, `bitlocker` ∈ `on`, `suspended`, `off`, `unknown`.
    ///
    /// With administrator rights (`ctx.elevated` or `env.is_elevated`) the status comes
    /// from `Win32_EncryptableVolume`; a failed query is the warning
    /// `issue.system.bitlocker_query_failed`. Without them every status is `unknown` with
    /// the warning `issue.system.bitlocker_needs_admin`.
    async fn run(
        &self,
        ctx: &ExportContext<'_>,
        _params: &serde_json::Value,
    ) -> Result<ExportResult, ExportError> {
        if ctx.cancel.is_cancelled() {
            return Err(ExportError::Cancelled);
        }
        let (statuses, issue) = self.bitlocker(ctx).await?;
        let text = render(&ctx.env.drives, &statuses);
        let file = write_file(ctx.target_dir, FILE, text.as_bytes())?;
        Ok(ExportResult {
            files: vec![file],
            warnings: issue.into_iter().collect(),
            restore_hint: RestoreHint::None,
        })
    }
}

/// BitLocker status by uppercase drive letter (`DriveLetter` = `"C:"`); volumes without
/// a letter are skipped.
pub(crate) fn statuses(rows: &[WmiRow]) -> BTreeMap<char, BitLocker> {
    rows.iter()
        .filter_map(|row| {
            let letter = text(row, "DriveLetter")?.chars().next()?;
            Some((letter.to_ascii_uppercase(), BitLocker::of(row)))
        })
        .collect()
}

/// `disks.json`: pretty-printed JSON with a final newline.
pub(crate) fn render(drives: &[DriveInfo], statuses: &BTreeMap<char, BitLocker>) -> String {
    let items: Vec<_> = drives
        .iter()
        .map(|drive| {
            let mut item = serde_json::to_value(drive).unwrap_or_else(|_| json!({}));
            let status = statuses
                .get(&drive.letter.to_ascii_uppercase())
                .copied()
                .unwrap_or(BitLocker::Unknown);
            if let Some(map) = item.as_object_mut() {
                map.insert("bitlocker".to_owned(), json!(status.as_str()));
            }
            item
        })
        .collect();
    let mut text = serde_json::to_string_pretty(&json!({ "drives": items })).unwrap_or_default();
    text.push('\n');
    text
}
