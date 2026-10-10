//! The `windows-info` exporter (SPEC-06 §4.3): version, edition, language, computer
//! name and the activation status of Windows as `windows.json`, for the checklist (§4.7)
//! and for choosing the installation media.
//!
//! No product key is read (FR-06-05): the activation status comes from
//! `SoftwareLicensingProduct.LicenseStatus` (the key columns are neither selected nor
//! returned), and the OEM key in the firmware is only tested for presence by the WQL
//! filter `OA3xOriginalProductKey <> ''`, so the key itself never reaches this process.

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use sk_core::env::Environment;
use sk_core::model::{Category, Finding, RegHive, ScanIssue, Sensitivity};
use sk_core::registry::{RegistryReader, SystemRegistry};

use super::{plan_finding, unavailable, warning, write_file, Plan, NOT_WINDOWS};
use crate::error::ExportError;
use crate::exporter::{Availability, ExportContext, ExportResult, RestoreHint, SystemExporter};
use crate::wql::{int, query_async, text, SystemWmi, WmiError, WmiRow, WmiSource, CIMV2};

/// Exporter identifier.
pub(crate) const ID: &str = "windows-info";

/// The exported file, in `system/windows-info/`.
pub(crate) const FILE: &str = "windows.json";

/// The license of Windows itself (`ApplicationID` of Windows in `SoftwareLicensingProduct`)
/// that has a key installed. Only the status and the channel are selected.
pub(crate) const ACTIVATION_QUERY: &str = "SELECT LicenseStatus, ProductKeyChannel \
     FROM SoftwareLicensingProduct \
     WHERE ApplicationID = '55c92734-d682-4d71-983e-d6ec3f16059f' AND PartialProductKey IS NOT NULL";

/// One row when the firmware holds an OEM key; the key is filtered by WMI, not selected.
pub(crate) const OEM_KEY_QUERY: &str =
    "SELECT Version FROM SoftwareLicensingService WHERE OA3xOriginalProductKey <> ''";

/// Key and value of the edition (`Professional`, `Core`, …).
const CURRENT_VERSION_KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
const EDITION_VALUE: &str = "EditionID";

/// Finding title (i18n key).
const TITLE: &str = "system.title.windows_info";

/// Warning: the activation status could not be read. Args: `error`.
pub(crate) const ISSUE_ACTIVATION_FAILED: &str = "issue.system.activation_query_failed";

/// Timeout of each WMI query (`SoftwareLicensingProduct` takes about a second).
const RUN_TIMEOUT: Duration = Duration::from_secs(60);

/// Size estimate of `windows.json`.
const ESTIMATE_BYTES: u64 = 1024;

/// Facts about the installed Windows.
pub(crate) struct WindowsInfoExporter {
    wmi: Arc<dyn WmiSource>,
    registry: Arc<dyn RegistryReader>,
}

/// The activation of Windows as written to `windows.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Activation {
    /// `LicenseStatus` of the Windows license, `None` when unknown.
    pub license_status: Option<i64>,
    /// `ProductKeyChannel`: `Retail`, `OEM:DM`, `Volume:GVLK`, …
    pub channel: Option<String>,
}

impl WindowsInfoExporter {
    /// The exporter with the WMI and the registry of this machine.
    pub(crate) fn new() -> Self {
        Self {
            wmi: Arc::new(SystemWmi),
            registry: Arc::new(SystemRegistry),
        }
    }

    /// The exporter with the given WMI source and registry (tests).
    #[cfg(test)]
    pub(crate) fn with(wmi: Arc<dyn WmiSource>, registry: Arc<dyn RegistryReader>) -> Self {
        Self { wmi, registry }
    }

    /// `detect` without the platform check: always available, no WMI query.
    pub(crate) fn probe() -> Availability {
        Availability::Available {
            estimated_bytes: Some(ESTIMATE_BYTES),
            details: BTreeMap::new(),
        }
    }

    /// The activation, or unknown with the warning `issue.system.activation_query_failed`.
    async fn activation(
        &self,
        ctx: &ExportContext<'_>,
    ) -> Result<(Activation, Option<ScanIssue>), ExportError> {
        let query = query_async(
            self.wmi.clone(),
            CIMV2,
            ACTIVATION_QUERY,
            RUN_TIMEOUT,
            ctx.cancel,
        );
        match query.await {
            Ok(rows) => Ok((activation(&rows), None)),
            Err(WmiError::Cancelled) => Err(ExportError::Cancelled),
            Err(err) => {
                tracing::debug!(error = %err, "activation status not read");
                let args = BTreeMap::from([("error".to_owned(), err.to_string())]);
                let unknown = Activation {
                    license_status: None,
                    channel: None,
                };
                Ok((unknown, Some(warning(ISSUE_ACTIVATION_FAILED, args))))
            }
        }
    }

    /// Whether the firmware holds an OEM key; `None` when WMI does not tell.
    async fn oem_key_in_firmware(
        &self,
        ctx: &ExportContext<'_>,
    ) -> Result<Option<bool>, ExportError> {
        let query = query_async(
            self.wmi.clone(),
            CIMV2,
            OEM_KEY_QUERY,
            RUN_TIMEOUT,
            ctx.cancel,
        );
        match query.await {
            Ok(rows) => Ok(Some(!rows.is_empty())),
            Err(WmiError::Cancelled) => Err(ExportError::Cancelled),
            Err(err) => {
                tracing::debug!(error = %err, "OEM key presence not read");
                Ok(None)
            }
        }
    }
}

#[async_trait]
impl SystemExporter for WindowsInfoExporter {
    fn id(&self) -> &'static str {
        ID
    }

    /// Always available on Windows; `system.not_windows` elsewhere.
    fn detect(&self, _env: &Environment) -> Availability {
        if cfg!(windows) {
            Self::probe()
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

    /// Writes `windows.json` (see [`render`]). The version, language and computer name
    /// come from the environment, the edition from `EditionID` in the registry, the
    /// activation from WMI. A failed activation query gives `"status": "unknown"` and the
    /// warning `issue.system.activation_query_failed`; an unknown OEM key presence is
    /// `null`. Informational: nothing is restored from it.
    async fn run(
        &self,
        ctx: &ExportContext<'_>,
        _params: &serde_json::Value,
    ) -> Result<ExportResult, ExportError> {
        if ctx.cancel.is_cancelled() {
            return Err(ExportError::Cancelled);
        }
        let (activation, issue) = self.activation(ctx).await?;
        let oem_key = self.oem_key_in_firmware(ctx).await?;
        let edition = self
            .registry
            .string_value(RegHive::Hklm, CURRENT_VERSION_KEY, EDITION_VALUE);
        let text = render(ctx.env, edition.as_deref(), &activation, oem_key);
        let file = write_file(ctx.target_dir, FILE, text.as_bytes())?;
        Ok(ExportResult {
            files: vec![file],
            warnings: issue.into_iter().collect(),
            restore_hint: RestoreHint::None,
        })
    }
}

/// The activation of the rows of [`ACTIVATION_QUERY`]: the licensed row if there is one
/// (several keys may be installed), else the first; no row is unknown.
pub(crate) fn activation(rows: &[WmiRow]) -> Activation {
    let chosen = rows
        .iter()
        .find(|row| int(row, "LicenseStatus") == Some(1))
        .or_else(|| rows.first());
    Activation {
        license_status: chosen.and_then(|row| int(row, "LicenseStatus")),
        channel: chosen.and_then(|row| text(row, "ProductKeyChannel").map(str::to_owned)),
    }
}

/// Name of a `LicenseStatus` value (`SoftwareLicensingProduct`).
pub(crate) fn status_name(license_status: Option<i64>) -> &'static str {
    match license_status {
        Some(0) => "unlicensed",
        Some(1) => "licensed",
        Some(2) => "oob_grace",
        Some(3) => "oot_grace",
        Some(4) => "non_genuine_grace",
        Some(5) => "notification",
        Some(6) => "extended_grace",
        _ => "unknown",
    }
}

/// `windows.json`: pretty-printed JSON with a final newline:
/// `{ product, edition, display_version, build, arch, ui_language, machine_name,
/// activation: { status, license_status, channel }, oem_key_in_firmware }`.
pub(crate) fn render(
    env: &Environment,
    edition: Option<&str>,
    activation: &Activation,
    oem_key_in_firmware: Option<bool>,
) -> String {
    let doc = json!({
        "product": env.os.product,
        "edition": edition,
        "display_version": env.os.display_version,
        "build": env.os.build,
        "arch": env.os.arch,
        "ui_language": env.os.ui_language,
        "machine_name": env.machine_name,
        "activation": {
            "status": status_name(activation.license_status),
            "license_status": activation.license_status,
            "channel": activation.channel,
        },
        "oem_key_in_firmware": oem_key_in_firmware,
    });
    let mut text = serde_json::to_string_pretty(&doc).unwrap_or_default();
    text.push('\n');
    text
}
