//! The `registry-user` exporter (SPEC-06 §4.3): `reg export` of the user's keyboard,
//! mouse, regional, Explorer, console and autostart settings into `*.reg` files.

#[cfg(test)]
mod tests;

use std::collections::BTreeSet;

use async_trait::async_trait;
use sk_core::env::Environment;
use sk_core::model::{Category, Finding, RegHive, Sensitivity};

use super::partial::{Partial, ISSUE_EXPORT_FAILED};
use super::{plan_finding, unavailable, Plan, NOT_WINDOWS};
use crate::error::ExportError;
use crate::exporter::{Availability, ExportContext, ExportResult, RestoreHint, SystemExporter};
use crate::reg_export::{reg_file_name, windir, RegExport};

/// Exporter identifier.
pub(crate) const ID: &str = "registry-user";

/// Exported `HKCU` branches, in this order (SPEC-06 §4.3).
pub(crate) const KEYS: [&str; 7] = [
    r"Control Panel\Keyboard",
    r"Control Panel\Mouse",
    r"Control Panel\International",
    r"Keyboard Layout",
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced",
    r"Console",
    r"Software\Microsoft\Windows\CurrentVersion\Run",
];

/// Finding title (i18n key).
const TITLE: &str = "system.title.registry_user";

/// Restore instructions (i18n key): import the `.reg` files.
const RESTORE_KEY: &str = "system.restore.registry_user";

/// User settings from `HKCU` as `.reg` files.
pub(crate) struct RegistryUserExporter {
    reg: RegExport,
}

impl RegistryUserExporter {
    /// The exporter with `reg.exe` and the registry of this machine.
    pub(crate) fn new() -> Self {
        Self {
            reg: RegExport::system(),
        }
    }

    /// The exporter with the given process runner and registry (tests).
    #[cfg(test)]
    pub(crate) fn with(reg: RegExport) -> Self {
        Self { reg }
    }

    /// `detect` without the platform check: always available.
    pub(crate) fn probe() -> Availability {
        Availability::Available {
            estimated_bytes: None,
            details: Default::default(),
        }
    }
}

#[async_trait]
impl SystemExporter for RegistryUserExporter {
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
            sensitivity: Sensitivity::Low,
            params: serde_json::json!({}),
        };
        plan_finding(plan, avail)
    }

    /// Exports every key of [`KEYS`] to `<slug(key)>.reg` in the target folder
    /// (SPEC-06 §4.5).
    ///
    /// A missing key or a failed `reg export` of one key is a warning and the other keys
    /// are still exported; cancellation stops the export. When no file at all is written,
    /// the first error is returned.
    async fn run(
        &self,
        ctx: &ExportContext<'_>,
        _params: &serde_json::Value,
    ) -> Result<ExportResult, ExportError> {
        let windir = windir(ctx.env);
        let mut taken = BTreeSet::new();
        let mut files = Vec::new();
        let mut failures = Partial::default();
        for key in KEYS {
            let out_file = ctx.target_dir.join(reg_file_name(key, &mut taken));
            let result = self
                .reg
                .export(windir, RegHive::Hkcu, key, &out_file, ctx.cancel)
                .await;
            match result {
                Ok(file) => files.push(file),
                Err(err) => failures.add(RegHive::Hkcu, key, ISSUE_EXPORT_FAILED, err)?,
            }
        }
        failures.finish(files, restore_hint())
    }
}

/// The `.reg` files are imported by the restore (SPEC-13 §4.4) or by a double click.
fn restore_hint() -> RestoreHint {
    RestoreHint::Manual {
        key: RESTORE_KEY.to_owned(),
    }
}
