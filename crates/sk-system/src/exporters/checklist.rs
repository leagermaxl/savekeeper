//! The `checklist` exporter (SPEC-06 §4.7, FR-06-08): `checklist.md`, what has to be
//! done by hand before reinstalling Windows because it is not saved as files —
//! two-factor codes, BitLocker recovery keys, program licenses, Windows activation.
//!
//! The facts come from three places:
//! - `Environment.installed_programs`: programs from the built-in list
//!   `licensing-hints.yaml` and authenticator apps;
//! - the files of the exporters that ran before it in the same backup (sibling folders
//!   of the target folder, only read): `windows-info/windows.json`,
//!   `disks-info/disks.json`, `drivers/`, `winget/winget.json`;
//! - the parameters given by the backup for facts of the scan findings:
//!   `{ "credentials": bool, "browsers": bool, "unpushed_repos": n, "cloud_only_files": n }`
//!   (missing ones are `false`/`0`).

mod hints;
mod render;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::path::Path;

use async_trait::async_trait;
use serde_json::Value;
use sk_core::env::{Environment, InstalledProgram};
use sk_core::model::{Category, Finding, Sensitivity};

use self::render::{Activation, BitLockerFacts, Facts, WindowsFacts};
use super::programs::Lang;
use super::{disks_info, drivers, plan_finding, unavailable, windows_info, winget, write_file};
use super::{Plan, NOT_WINDOWS};
use crate::error::ExportError;
use crate::exporter::{Availability, ExportContext, ExportResult, RestoreHint, SystemExporter};

/// Exporter identifier.
pub(crate) const ID: &str = "checklist";

/// The generated file, in `system/checklist/` (SPEC-06 §4.3).
pub(crate) const FILE: &str = "checklist.md";

/// Finding title (i18n key).
const TITLE: &str = "system.title.checklist";

/// Size estimate of `checklist.md`.
const ESTIMATE_BYTES: u64 = 4 * 1024;

/// Parameters with facts of the scan findings (see the module documentation).
pub(crate) const PARAM_CREDENTIALS: &str = "credentials";
pub(crate) const PARAM_BROWSERS: &str = "browsers";
pub(crate) const PARAM_UNPUSHED_REPOS: &str = "unpushed_repos";
pub(crate) const PARAM_CLOUD_ONLY_FILES: &str = "cloud_only_files";

/// Authenticator apps for Windows (patterns of the program name, `*` is any text): their
/// codes are lost with the app data unless they are moved (SPEC-06 §4.7).
const AUTHENTICATORS: &[&str] = &["Authy*", "Twilio Authy*", "WinAuth*", "Ente Auth*"];

/// The checklist of manual actions.
pub(crate) struct ChecklistExporter;

#[async_trait]
impl SystemExporter for ChecklistExporter {
    fn id(&self) -> &'static str {
        ID
    }

    /// Always available on Windows; `system.not_windows` elsewhere.
    fn detect(&self, _env: &Environment) -> Availability {
        if cfg!(windows) {
            Availability::Available {
                estimated_bytes: Some(ESTIMATE_BYTES),
                details: BTreeMap::new(),
            }
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

    /// Writes `checklist.md` in the Windows UI language (Russian for `ru-*`, English
    /// otherwise). The files of other exporters are read from the sibling folders of the
    /// target folder; a missing or unreadable one counts as "not checked".
    async fn run(
        &self,
        ctx: &ExportContext<'_>,
        params: &Value,
    ) -> Result<ExportResult, ExportError> {
        if ctx.cancel.is_cancelled() {
            return Err(ExportError::Cancelled);
        }
        let facts = collect_facts(ctx.env, ctx.target_dir, params);
        let text = render::render(&facts);
        let file = write_file(ctx.target_dir, FILE, text.as_bytes())?;
        Ok(ExportResult {
            files: vec![file],
            warnings: Vec::new(),
            restore_hint: RestoreHint::None,
        })
    }
}

/// The facts of the checklist (see the module documentation).
pub(crate) fn collect_facts(env: &Environment, target_dir: &Path, params: &Value) -> Facts {
    let backup_system = target_dir.parent();
    let sibling = |exporter: &str| backup_system.map(|dir| dir.join(exporter));
    Facts {
        lang: Lang::for_ui_language(&env.os.ui_language),
        machine_name: env.machine_name.clone(),
        credentials: flag(params, PARAM_CREDENTIALS),
        authenticators: authenticators(&env.installed_programs),
        browsers: flag(params, PARAM_BROWSERS),
        bitlocker: sibling(disks_info::ID)
            .and_then(|dir| read_json(&dir.join(disks_info::FILE)))
            .map_or(BitLockerFacts::NotChecked, |json| bitlocker_facts(&json)),
        licenses: hints::match_programs(&hints::builtin(), &env.installed_programs),
        windows: sibling(windows_info::ID)
            .and_then(|dir| read_json(&dir.join(windows_info::FILE)))
            .map(|json| windows_facts(&json)),
        unpushed_repos: count(params, PARAM_UNPUSHED_REPOS),
        cloud_only_files: count(params, PARAM_CLOUD_ONLY_FILES),
        drivers_exported: sibling(drivers::ID).is_some_and(|dir| has_entries(&dir)),
        winget_exported: sibling(winget::ID)
            .is_some_and(|dir| dir.join(winget::EXPORT_FILE).is_file()),
    }
}

/// A boolean parameter; missing or not a boolean is `false`.
fn flag(params: &Value, name: &str) -> bool {
    params.get(name).and_then(Value::as_bool).unwrap_or(false)
}

/// A count parameter; missing or not a non-negative integer is `0`.
fn count(params: &Value, name: &str) -> u64 {
    params.get(name).and_then(Value::as_u64).unwrap_or(0)
}

/// Names of the installed authenticator apps, sorted.
fn authenticators(programs: &[InstalledProgram]) -> Vec<String> {
    let names = programs
        .iter()
        .filter(|p| {
            AUTHENTICATORS
                .iter()
                .any(|pattern| hints::wildcard_match(pattern, &p.name))
        })
        .map(|p| p.name.as_str())
        .collect();
    hints::sorted_unique(names)
}

/// JSON written by another exporter of the backup; `None` when it is missing or invalid
/// (the checklist then treats the fact as not checked).
fn read_json(path: &Path) -> Option<Value> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) => {
            if err.kind() != std::io::ErrorKind::NotFound {
                tracing::debug!(error = %err, file = %path.display(), "checklist input not read");
            }
            return None;
        }
    };
    serde_json::from_slice(&bytes)
        .map_err(
            |err| tracing::debug!(error = %err, file = %path.display(), "checklist input invalid"),
        )
        .ok()
}

/// Whether the folder exists and has any entry (the `drivers` export wrote files).
fn has_entries(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_some())
}

/// BitLocker facts of `disks.json` (`{ "drives": [{ "letter", "kind", "bitlocker" }] }`):
/// `on` and `suspended` are encrypted; `unknown` counts only for fixed and removable
/// drives (network and optical drives are not encrypted by BitLocker).
fn bitlocker_facts(json: &Value) -> BitLockerFacts {
    let mut encrypted = Vec::new();
    let mut unknown = Vec::new();
    let drives = json.get("drives").and_then(Value::as_array);
    for drive in drives.into_iter().flatten() {
        let Some(letter) = drive
            .get("letter")
            .and_then(Value::as_str)
            .and_then(|s| s.chars().next())
        else {
            continue;
        };
        let letter = letter.to_ascii_uppercase();
        let local = matches!(
            drive.get("kind").and_then(Value::as_str),
            Some("fixed" | "removable")
        );
        match drive.get("bitlocker").and_then(Value::as_str) {
            Some("on" | "suspended") => encrypted.push(letter),
            Some("off") => {}
            _ if local => unknown.push(letter),
            _ => {}
        }
    }
    encrypted.sort_unstable();
    encrypted.dedup();
    unknown.sort_unstable();
    unknown.dedup();
    BitLockerFacts::Checked { encrypted, unknown }
}

/// Windows facts of `windows.json` (`activation.status`, `activation.channel`,
/// `oem_key_in_firmware`): a licensed volume channel (`Volume:GVLK`, `Volume:MAK`) is an
/// organization's activation; any other known status than `licensed` is not activated.
fn windows_facts(json: &Value) -> WindowsFacts {
    let activation = json.get("activation");
    let field = |name: &str| activation.and_then(|a| a.get(name)).and_then(Value::as_str);
    let volume = field("channel").is_some_and(|c| c.to_ascii_lowercase().starts_with("volume"));
    let activation = match field("status") {
        Some("licensed") if volume => Activation::Volume,
        Some("licensed") => Activation::Licensed,
        None | Some("unknown") => Activation::Unknown,
        Some(_) => Activation::NotActivated,
    };
    WindowsFacts {
        activation,
        oem_key_in_firmware: json
            .get("oem_key_in_firmware")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}
