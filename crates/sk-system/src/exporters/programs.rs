//! The `programs` exporter (SPEC-06 §4.3, FR-06-07): the list of installed programs
//! from `Environment.installed_programs` as `programs.csv` and `programs.html`, the base
//! for reinstalling by hand. Works without winget.

mod render;
mod winget_ids;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use sk_core::env::Environment;
use sk_core::model::{Category, Finding, ScanIssue, Sensitivity};

pub(crate) use self::render::Lang;
use self::render::{Page, Row};
use self::winget_ids::WingetIndex;
use super::{plan_finding, unavailable, warning, winget, write_file, Plan, NOT_WINDOWS};
use crate::error::ExportError;
use crate::exporter::{Availability, ExportContext, ExportResult, RestoreHint, SystemExporter};

/// Exporter identifier.
pub(crate) const ID: &str = "programs";

/// Output files (SPEC-06 §4.3).
pub(crate) const CSV_FILE: &str = "programs.csv";
pub(crate) const HTML_FILE: &str = "programs.html";

/// Finding title (i18n key).
const TITLE: &str = "system.title.programs";

/// Restore instructions (i18n key): reinstall the programs by hand from the list.
const RESTORE_KEY: &str = "system.restore.programs";

/// Warning: `winget.json` exists but cannot be read or is not a winget export.
const ISSUE_WINGET_UNREADABLE: &str = "issue.system.winget_json_unreadable";

/// Size estimate of both files: fixed part plus a part per program.
const ESTIMATE_BASE_BYTES: u64 = 4 * 1024;
const ESTIMATE_BYTES_PER_PROGRAM: u64 = 400;

/// The list of installed programs.
pub(crate) struct ProgramsExporter;

impl ProgramsExporter {
    /// `detect` without the platform check: always available.
    pub(crate) fn probe(env: &Environment) -> Availability {
        let count = env.installed_programs.len();
        Availability::Available {
            estimated_bytes: Some(ESTIMATE_BASE_BYTES + count as u64 * ESTIMATE_BYTES_PER_PROGRAM),
            details: BTreeMap::from([("programs".to_owned(), count.to_string())]),
        }
    }
}

#[async_trait]
impl SystemExporter for ProgramsExporter {
    fn id(&self) -> &'static str {
        ID
    }

    /// Always available on Windows (the list may be empty); `system.not_windows` elsewhere.
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

    /// Writes `programs.csv` and `programs.html` from `ctx.env.installed_programs`,
    /// sorted by name.
    ///
    /// The "winget" column comes from `winget.json` of the `winget` exporter in the
    /// sibling folder `<backup>/system/winget/` (only read), so `winget` has to run first;
    /// without that file the column is empty, and an unreadable one gives a warning.
    async fn run(
        &self,
        ctx: &ExportContext<'_>,
        _params: &serde_json::Value,
    ) -> Result<ExportResult, ExportError> {
        if ctx.cancel.is_cancelled() {
            return Err(ExportError::Cancelled);
        }
        let mut warnings = Vec::new();
        let winget = load_winget(ctx.target_dir, &mut warnings);

        let mut programs: Vec<_> = ctx.env.installed_programs.iter().collect();
        programs.sort_by_cached_key(|p| (p.name.to_lowercase(), p.name.clone()));
        let rows: Vec<Row<'_>> = programs
            .into_iter()
            .map(|program| Row {
                program,
                winget_id: winget.as_ref().and_then(|index| index.find(&program.name)),
            })
            .collect();

        let page = Page {
            lang: Lang::for_ui_language(&ctx.env.os.ui_language),
            machine_name: &ctx.env.machine_name,
            winget_known: winget.is_some(),
        };
        let csv = render::csv(&rows, page.winget_known);
        let html = render::html(&rows, &page);
        let files = vec![
            write_file(ctx.target_dir, CSV_FILE, csv.as_bytes())?,
            write_file(ctx.target_dir, HTML_FILE, html.as_bytes())?,
        ];
        Ok(ExportResult {
            files,
            warnings,
            restore_hint: RestoreHint::Manual {
                key: RESTORE_KEY.to_owned(),
            },
        })
    }
}

/// `<target_dir>/../winget/winget.json`: the export of the `winget` exporter.
fn winget_file(target_dir: &Path) -> Option<PathBuf> {
    Some(
        target_dir
            .parent()?
            .join(winget::ID)
            .join(winget::EXPORT_FILE),
    )
}

/// Reads the winget index; `None` when there is no `winget.json` or it is unreadable
/// (then with a warning).
fn load_winget(target_dir: &Path, warnings: &mut Vec<ScanIssue>) -> Option<WingetIndex> {
    let path = winget_file(target_dir)?;
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
        Err(err) => {
            warnings.push(unreadable(err.to_string()));
            return None;
        }
    };
    let text = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    let index = serde_json::from_slice(text)
        .map_err(|err| err.to_string())
        .and_then(|json| {
            WingetIndex::from_json(&json).ok_or_else(|| "not a winget export".to_owned())
        });
    match index {
        Ok(index) => Some(index),
        Err(error) => {
            warnings.push(unreadable(error));
            None
        }
    }
}

fn unreadable(error: String) -> ScanIssue {
    warning(
        ISSUE_WINGET_UNREADABLE,
        BTreeMap::from([("error".to_owned(), error)]),
    )
}
