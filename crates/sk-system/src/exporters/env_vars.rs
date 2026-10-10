//! The `env-vars` exporter (SPEC-06 §4.3): the user's environment variables from
//! `HKCU\Environment` as JSON and as a `.reg` file, and the system variables as JSON for
//! reference only.

#[cfg(test)]
mod tests;

use std::io;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use sk_core::env::Environment;
use sk_core::model::{Category, Finding, RegHive, Sensitivity};
use sk_core::registry::{KeyState, RegData, RegValue, RegistryReader, SystemRegistry};

use super::partial::{Partial, ISSUE_EXPORT_FAILED, ISSUE_READ_FAILED};
use super::{plan_finding, unavailable, write_file, Plan, NOT_WINDOWS};
use crate::error::ExportError;
use crate::exporter::{
    Availability, ExportContext, ExportResult, ExportedFile, RestoreHint, SystemExporter,
};
use crate::reg_export::{full_key, windir, RegExport};

/// Exporter identifier.
pub(crate) const ID: &str = "env-vars";

/// Output files (SPEC-06 §4.3).
pub(crate) const USER_JSON: &str = "user-env.json";
pub(crate) const USER_REG: &str = "user-env.reg";
pub(crate) const SYSTEM_JSON: &str = "system-env.json";

/// Key of the user's variables (`HKCU`).
pub(crate) const USER_KEY: &str = "Environment";

/// Key of the system variables (`HKLM`), exported to JSON only.
pub(crate) const SYSTEM_KEY: &str = r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment";

/// Finding title (i18n key).
const TITLE: &str = "system.title.env_vars";

/// Restore instructions (i18n key): import `user-env.reg`.
const RESTORE_KEY: &str = "system.restore.env_vars";

/// Environment variables of the user and of the system.
pub(crate) struct EnvVarsExporter {
    reg: RegExport,
    /// Registry whose values are written to the JSON files.
    registry: Arc<dyn RegistryReader>,
}

impl EnvVarsExporter {
    /// The exporter with `reg.exe` and the registry of this machine.
    pub(crate) fn new() -> Self {
        Self {
            reg: RegExport::system(),
            registry: Arc::new(SystemRegistry),
        }
    }

    /// The exporter with the given `reg export` and registry of the JSON files (tests).
    #[cfg(test)]
    pub(crate) fn with(reg: RegExport, registry: Arc<dyn RegistryReader>) -> Self {
        Self { reg, registry }
    }

    /// `detect` without the platform check: always available.
    pub(crate) fn probe() -> Availability {
        Availability::Available {
            estimated_bytes: None,
            details: Default::default(),
        }
    }

    /// Writes the values of `hive\key` as JSON to `file_name`; a missing or unreadable
    /// key is recorded in `failures` (`NotAvailable` for a missing key, an access error
    /// for a key that cannot be read).
    fn export_json(
        &self,
        ctx: &ExportContext<'_>,
        hive: RegHive,
        key: &str,
        file_name: &str,
        failures: &mut Partial,
    ) -> Result<Option<ExportedFile>, ExportError> {
        let Some(values) = self.registry.values(hive, key) else {
            let err = match self.registry.key_state(hive, key) {
                KeyState::AccessDenied => ExportError::Io(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "access denied",
                )),
                // Missing, or deleted after `values`.
                KeyState::Missing | KeyState::Present => ExportError::NotAvailable,
            };
            failures.add(hive, key, ISSUE_READ_FAILED, err)?;
            return Ok(None);
        };
        let text = render_json(hive, key, values);
        Ok(Some(write_file(
            ctx.target_dir,
            file_name,
            text.as_bytes(),
        )?))
    }
}

#[async_trait]
impl SystemExporter for EnvVarsExporter {
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

    /// Writes `user-env.json` (values of `HKCU\Environment`), `user-env.reg`
    /// (`reg export` of the same key) and `system-env.json` (values of the system
    /// `Environment` key, for reference; it is not restored).
    ///
    /// The JSON keeps the stored data: `REG_EXPAND_SZ` values are not expanded. A file
    /// that cannot be produced is a warning; when none is written, the first error is
    /// returned. Cancellation stops the export.
    async fn run(
        &self,
        ctx: &ExportContext<'_>,
        _params: &serde_json::Value,
    ) -> Result<ExportResult, ExportError> {
        if ctx.cancel.is_cancelled() {
            return Err(ExportError::Cancelled);
        }
        let mut files = Vec::new();
        let mut failures = Partial::default();

        let user = self.export_json(ctx, RegHive::Hkcu, USER_KEY, USER_JSON, &mut failures)?;
        files.extend(user);

        let out_file = ctx.target_dir.join(USER_REG);
        let result = self
            .reg
            .export(
                windir(ctx.env),
                RegHive::Hkcu,
                USER_KEY,
                &out_file,
                ctx.cancel,
            )
            .await;
        match result {
            Ok(file) => files.push(file),
            Err(err) => failures.add(RegHive::Hkcu, USER_KEY, ISSUE_EXPORT_FAILED, err)?,
        }

        if ctx.cancel.is_cancelled() {
            return Err(ExportError::Cancelled);
        }
        let system =
            self.export_json(ctx, RegHive::Hklm, SYSTEM_KEY, SYSTEM_JSON, &mut failures)?;
        files.extend(system);

        failures.finish(
            files,
            RestoreHint::Manual {
                key: RESTORE_KEY.to_owned(),
            },
        )
    }
}

/// The JSON of one key: `{ "key": "HKCU\\Environment", "variables": [{ "name",
/// "type", "value" }] }`, variables sorted by name (case-insensitive), UTF-8, ending with
/// a newline.
///
/// `value` is a string for `REG_SZ` and `REG_EXPAND_SZ`, an array of strings for
/// `REG_MULTI_SZ`, a number for `REG_DWORD` and `REG_QWORD` and `null` for other types
/// (also for a `REG_DWORD`/`REG_QWORD` of the wrong size). A value of a type without a
/// `REG_*` name (type number above `REG_QWORD`) is left out.
pub(crate) fn render_json(hive: RegHive, key: &str, mut values: Vec<RegValue>) -> String {
    values.sort_by_cached_key(|v| (v.name.to_lowercase(), v.name.clone()));
    let variables: Vec<_> = values
        .into_iter()
        .filter_map(|v| {
            let Some(type_name) = type_name(&v.data) else {
                tracing::debug!(name = %v.name, data = ?v.data, "registry value of unknown type skipped");
                return None;
            };
            let value = match &v.data {
                RegData::String(s) | RegData::ExpandString(s) => json!(s),
                RegData::MultiString(items) => json!(items),
                RegData::Dword(n) => json!(n),
                RegData::Qword(n) => json!(n),
                RegData::Binary(_) | RegData::Other { .. } => serde_json::Value::Null,
            };
            Some(json!({ "name": v.name, "type": type_name, "value": value }))
        })
        .collect();
    let doc = json!({ "key": full_key(hive, key), "variables": variables });
    let mut text = serde_json::to_string_pretty(&doc).unwrap_or_default();
    text.push('\n');
    text
}

/// Name of the registry type of `data`: `REG_SZ`, `REG_EXPAND_SZ`, …; `None` for a type
/// number without a name.
pub(crate) fn type_name(data: &RegData) -> Option<&'static str> {
    let name = match data {
        RegData::String(_) => "REG_SZ",
        RegData::ExpandString(_) => "REG_EXPAND_SZ",
        RegData::MultiString(_) => "REG_MULTI_SZ",
        RegData::Dword(_) => "REG_DWORD",
        RegData::Qword(_) => "REG_QWORD",
        RegData::Binary(_) => "REG_BINARY",
        RegData::Other { kind, .. } => match kind {
            0 => "REG_NONE",
            1 => "REG_SZ",
            2 => "REG_EXPAND_SZ",
            3 => "REG_BINARY",
            4 => "REG_DWORD",
            5 => "REG_DWORD_BIG_ENDIAN",
            6 => "REG_LINK",
            7 => "REG_MULTI_SZ",
            8 => "REG_RESOURCE_LIST",
            9 => "REG_FULL_RESOURCE_DESCRIPTOR",
            10 => "REG_RESOURCE_REQUIREMENTS_LIST",
            11 => "REG_QWORD",
            _ => return None,
        },
    };
    Some(name)
}
