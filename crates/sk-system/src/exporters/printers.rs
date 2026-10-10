//! The `printers` exporter (SPEC-06 §4.3): the installed printers (name, driver, port)
//! from WMI `Win32_Printer` as `printers.json`, for reference when setting them up again.
//!
//! The virtual printers that come with Windows (`Microsoft Print to PDF`, `Microsoft XPS
//! Document Writer`, `Fax`, `OneNote`) are left out: they are not counted by `detect` and
//! not written by `run`.

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use sk_core::env::Environment;
use sk_core::model::{Category, Finding, Sensitivity};

use super::{plan_finding, unavailable, write_file, Plan, NOT_WINDOWS};
use crate::error::ExportError;
use crate::exporter::{Availability, ExportContext, ExportResult, RestoreHint, SystemExporter};
use crate::wql::{query_async, text, SystemWmi, WmiError, WmiRow, WmiSource, CIMV2};

/// Exporter identifier.
pub(crate) const ID: &str = "printers";

/// The exported file, in `system/printers/`.
pub(crate) const FILE: &str = "printers.json";

/// The WMI query of `detect` and `run`.
pub(crate) const QUERY: &str = "SELECT Name, DriverName, PortName FROM Win32_Printer";

/// Finding title (i18n key).
const TITLE: &str = "system.title.printers";

/// `reason_key`: only the printers that come with Windows.
const NONE: &str = "system.printers.none";

/// `reason_key`: the WMI query failed.
const QUERY_FAILED: &str = "system.printers.query_failed";

/// Timeout of the query in `detect` (FR-06-01: ≤ 500 ms; the query takes about 100 ms).
/// A timeout leaves the exporter available without a printer count and `run` decides.
const DETECT_TIMEOUT: Duration = Duration::from_millis(500);

/// Timeout of the query in `run` (a network printer may make the spooler slow).
const RUN_TIMEOUT: Duration = Duration::from_secs(60);

/// Size estimate of one printer in `printers.json`.
const ESTIMATE_BYTES_PER_PRINTER: u64 = 256;

/// The printers of `Win32_Printer`.
pub(crate) struct PrintersExporter {
    wmi: Arc<dyn WmiSource>,
}

/// One printer as written to `printers.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Printer {
    pub name: String,
    pub driver: Option<String>,
    pub port: Option<String>,
}

impl PrintersExporter {
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

    /// `detect` without the platform check: `Win32_Printer` with a 500 ms timeout.
    /// Printers other than those of Windows → Available with `details.printers`; none →
    /// `system.printers.none`; a failed query → `system.printers.query_failed`; a timeout
    /// → Available without details.
    pub(crate) fn probe(&self) -> Availability {
        match self.wmi.query(CIMV2, QUERY, DETECT_TIMEOUT) {
            Ok(rows) => {
                let printers = user_printers(&rows).len();
                if printers == 0 {
                    return unavailable(NONE);
                }
                Availability::Available {
                    estimated_bytes: Some(printers as u64 * ESTIMATE_BYTES_PER_PRINTER),
                    details: BTreeMap::from([("printers".to_owned(), printers.to_string())]),
                }
            }
            Err(WmiError::Timeout) => {
                tracing::debug!("Win32_Printer query timed out in detect");
                Availability::Available {
                    estimated_bytes: None,
                    details: BTreeMap::new(),
                }
            }
            Err(err) => {
                tracing::debug!(error = %err, "Win32_Printer query failed");
                unavailable(QUERY_FAILED)
            }
        }
    }
}

#[async_trait]
impl SystemExporter for PrintersExporter {
    fn id(&self) -> &'static str {
        ID
    }

    /// See [`PrintersExporter::probe`]. Reasons: `system.printers.none`,
    /// `system.printers.query_failed`, `system.not_windows`.
    fn detect(&self, _env: &Environment) -> Availability {
        if cfg!(windows) {
            self.probe()
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

    /// Writes `printers.json`: `{ "printers": [{ "name", "driver", "port" }] }`, sorted
    /// by name without case, without the printers of Windows. No other printer is
    /// [`ExportError::NotAvailable`]; a failed query is its error (timeout, access).
    /// Informational only: nothing is restored from it.
    async fn run(
        &self,
        ctx: &ExportContext<'_>,
        _params: &serde_json::Value,
    ) -> Result<ExportResult, ExportError> {
        let rows = query_async(self.wmi.clone(), CIMV2, QUERY, RUN_TIMEOUT, ctx.cancel).await?;
        let printers = user_printers(&rows);
        if printers.is_empty() {
            return Err(ExportError::NotAvailable);
        }
        let file = write_file(ctx.target_dir, FILE, render(&printers).as_bytes())?;
        Ok(ExportResult {
            files: vec![file],
            warnings: Vec::new(),
            restore_hint: RestoreHint::None,
        })
    }
}

/// The printers of `rows` other than those of Windows, sorted by name without case.
/// Rows without a name are skipped.
pub(crate) fn user_printers(rows: &[WmiRow]) -> Vec<Printer> {
    let mut printers: Vec<Printer> = rows
        .iter()
        .filter_map(|row| {
            let printer = Printer {
                name: text(row, "Name")?.to_owned(),
                driver: text(row, "DriverName").map(str::to_owned),
                port: text(row, "PortName").map(str::to_owned),
            };
            (!is_builtin(&printer)).then_some(printer)
        })
        .collect();
    printers.sort_by_cached_key(|p| (p.name.to_lowercase(), p.name.clone()));
    printers
}

/// Whether `printer` is a virtual printer of Windows: `Microsoft Print to PDF`,
/// `Microsoft XPS Document Writer`, the fax (`Fax`, localized names have the
/// `SHRFAX:` port and the shared fax driver) and the OneNote printers. Driver names are
/// not localized, so both the name and the driver are checked.
pub(crate) fn is_builtin(printer: &Printer) -> bool {
    const MARKERS: [&str; 4] = [
        "print to pdf",
        "xps document writer",
        "onenote",
        "microsoft shared fax",
    ];
    let mut texts = std::iter::once(printer.name.as_str()).chain(printer.driver.as_deref());
    texts.any(|t| {
        let t = t.to_lowercase();
        MARKERS.iter().any(|m| t.contains(m))
    }) || printer.name.eq_ignore_ascii_case("fax")
        || printer
            .port
            .as_deref()
            .is_some_and(|p| p.eq_ignore_ascii_case("SHRFAX:"))
}

/// `printers.json`: pretty-printed JSON with a final newline.
pub(crate) fn render(printers: &[Printer]) -> String {
    let items: Vec<_> = printers
        .iter()
        .map(|p| json!({ "name": p.name, "driver": p.driver, "port": p.port }))
        .collect();
    let mut text = serde_json::to_string_pretty(&json!({ "printers": items })).unwrap_or_default();
    text.push('\n');
    text
}
