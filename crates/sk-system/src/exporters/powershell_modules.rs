//! The `powershell-modules` exporter (SPEC-06 §4.3): the list of the PowerShell modules
//! installed for the user (`{DOCUMENTS}\PowerShell\Modules` and
//! `{DOCUMENTS}\WindowsPowerShell\Modules`) with their versions, and optionally copies
//! of the modules.
//!
//! Whether a module can be reinstalled from the PowerShell Gallery cannot be told, so
//! with `params.copy_files` every module is copied, up to 200 MB in total. Files that
//! are only in the cloud are never opened, manifests included (SPEC-10 FR-10-15).

mod manifest;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use sk_core::env::{Environment, KnownFolder};
use sk_core::model::{Category, Finding, Sensitivity};

use super::partial::{Partial, ISSUE_FILE_COPY_FAILED, ISSUE_FOLDER_READ_FAILED};
use super::source::{self, SourceFs};
use super::{copy_file, plan_finding, unavailable, warning, write_file, Plan, NOT_WINDOWS};
use crate::error::ExportError;
use crate::exporter::{Availability, ExportContext, ExportResult, RestoreHint, SystemExporter};

/// Exporter identifier.
pub(crate) const ID: &str = "powershell-modules";

/// Parameter: copy the module files, not only list them (SPEC-06 §4.3).
pub(crate) const COPY_FILES: &str = "copy_files";

/// The module list in `system/powershell-modules/`.
pub(crate) const MODULES_JSON: &str = "modules.json";

/// Folder of the copied modules: `Modules/<edition>/<module>/**`.
pub(crate) const MODULES_DIR: &str = "Modules";

/// Most bytes of module files copied by one export (SPEC-06 §4.3).
pub(crate) const COPY_LIMIT: u64 = 200 * 1024 * 1024;

/// PowerShell editions: name in `modules.json` and in [`MODULES_DIR`], and the folder
/// under `{DOCUMENTS}` whose `Modules` holds the user's modules.
pub(crate) const EDITIONS: [&str; 2] = ["PowerShell", "WindowsPowerShell"];

/// Finding title (i18n key).
const TITLE: &str = "system.title.powershell_modules";

/// `reason_key`: both module folders are missing or empty.
const NONE: &str = "system.powershell_modules.none";

/// Restore instructions (i18n key): copy `Modules/<edition>` back or reinstall the
/// modules of `modules.json`.
const RESTORE_KEY: &str = "system.restore.powershell_modules";

/// Warning: a module was not copied because the copy limit would be exceeded; it is only
/// listed. Args: `module`, `edition`, `bytes`.
pub(crate) const ISSUE_NOT_COPIED: &str = "issue.system.ps_module_not_copied";

/// PowerShell modules of the user.
pub(crate) struct PowershellModulesExporter {
    /// Most bytes copied by one export.
    copy_limit: u64,
    /// The module files; cloud-only ones are not opened (SPEC-10 FR-10-15).
    source: Arc<dyn SourceFs>,
}

impl PowershellModulesExporter {
    /// The exporter with the copy limit [`COPY_LIMIT`].
    pub(crate) fn new() -> Self {
        Self {
            copy_limit: COPY_LIMIT,
            source: source::real(),
        }
    }

    /// The exporter with the given copy limit (tests).
    #[cfg(test)]
    pub(crate) fn with_copy_limit(copy_limit: u64) -> Self {
        Self {
            copy_limit,
            ..Self::new()
        }
    }

    /// The exporter reading the module files through `source` (tests).
    #[cfg(test)]
    pub(crate) fn with_source(self, source: Arc<dyn SourceFs>) -> Self {
        Self { source, ..self }
    }

    /// `detect` without the platform check (SPEC-06 §4.3): Available when a module folder
    /// holds a module (a subfolder), with `details.modules` = their number. Sizes are
    /// not measured here, modules can hold many files.
    pub(crate) fn probe(env: &Environment) -> Availability {
        let modules: usize = EDITIONS
            .iter()
            .filter_map(|edition| modules_root(env, edition))
            .map(|root| match module_dirs(&root) {
                Ok(dirs) => dirs.len(),
                Err(err) => {
                    if err.kind() != io::ErrorKind::NotFound {
                        tracing::debug!(error = %err, "PowerShell module folder not read");
                    }
                    0
                }
            })
            .sum();
        if modules == 0 {
            return unavailable(NONE);
        }
        Availability::Available {
            estimated_bytes: None,
            details: BTreeMap::from([("modules".to_owned(), modules.to_string())]),
        }
    }
}

#[async_trait]
impl SystemExporter for PowershellModulesExporter {
    fn id(&self) -> &'static str {
        ID
    }

    /// See [`PowershellModulesExporter::probe`]. Reasons:
    /// `system.powershell_modules.none`, `system.not_windows`.
    fn detect(&self, env: &Environment) -> Availability {
        if cfg!(windows) {
            Self::probe(env)
        } else {
            unavailable(NOT_WINDOWS)
        }
    }

    /// Category `dev_environment`; the files are copied by default (`copy_files: true`).
    fn plan(&self, _env: &Environment, avail: &Availability) -> Option<Finding> {
        let plan = Plan {
            exporter_id: ID,
            title: TITLE,
            category: Category::DevEnvironment,
            sensitivity: Sensitivity::None,
            params: default_params(),
        };
        plan_finding(plan, avail)
    }

    /// Writes `modules.json` (every installed version of every module: name,
    /// `ModuleVersion` of its manifest, edition, folder) and, unless `params.copy_files`
    /// is `false`, copies the module folders to `Modules/<edition>/<module>/**`.
    ///
    /// Copied are whole modules, smallest first, while their total stays within
    /// [`COPY_LIMIT`]; a module that does not fit is only listed (warning
    /// `issue.system.ps_module_not_copied`). Links are not followed. A file or folder
    /// that cannot be read is a warning. A file that is only in the cloud is never
    /// opened (SPEC-10 FR-10-15), warning `issue.system.cloud_only` {path}: a module is
    /// copied without such files (`copied: true`), and the version of a cloud-only
    /// manifest is the name of its version folder. No module at all is
    /// [`ExportError::NotAvailable`]. Cancellation stops the export.
    async fn run(
        &self,
        ctx: &ExportContext<'_>,
        params: &serde_json::Value,
    ) -> Result<ExportResult, ExportError> {
        if ctx.cancel.is_cancelled() {
            return Err(ExportError::Cancelled);
        }
        let copy = copy_files(params);
        let mut failures = Partial::default();
        let mut modules = Vec::new();
        for edition in EDITIONS {
            let Some(root) = modules_root(ctx.env, edition) else {
                continue;
            };
            let dirs = match module_dirs(&root) {
                Ok(dirs) => dirs,
                Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
                Err(err) => {
                    failures.add_path(ISSUE_FOLDER_READ_FAILED, &root, err.into())?;
                    continue;
                }
            };
            for name in dirs {
                if ctx.cancel.is_cancelled() {
                    return Err(ExportError::Cancelled);
                }
                let dir = root.join(&name);
                let files = if copy {
                    list_files(&dir, &mut failures)?
                } else {
                    Vec::new()
                };
                modules.push(Module {
                    edition,
                    versions: versions(&dir, &name, self.source.as_ref(), &mut failures),
                    name,
                    dir,
                    files,
                    copied: false,
                });
            }
        }
        if modules.is_empty() {
            return failures.finish(Vec::new(), restore_hint());
        }
        if copy {
            select_for_copy(&mut modules, self.copy_limit, &mut failures);
        }

        let json = render_json(&modules);
        let mut files = vec![write_file(ctx.target_dir, MODULES_JSON, json.as_bytes())?];
        for module in modules.iter().filter(|m| m.copied) {
            let dest = Path::new(MODULES_DIR)
                .join(module.edition)
                .join(&module.name);
            for file in &module.files {
                if ctx.cancel.is_cancelled() {
                    return Err(ExportError::Cancelled);
                }
                let src = module.dir.join(&file.rel_path);
                let rel_path = dest.join(&file.rel_path);
                match copy_file(self.source.as_ref(), &src, ctx.target_dir, rel_path) {
                    Ok(Some(copied)) => files.push(copied),
                    Ok(None) => failures.cloud_only(&src),
                    Err(err) => failures.add_path(ISSUE_FILE_COPY_FAILED, &src, err.into())?,
                }
            }
        }
        failures.finish(files, restore_hint())
    }
}

/// Parameters of a planned finding: copy the files.
pub(crate) fn default_params() -> serde_json::Value {
    json!({ COPY_FILES: true })
}

/// Whether `params` ask for copies of the files: yes unless `copy_files` is `false`.
pub(crate) fn copy_files(params: &serde_json::Value) -> bool {
    params.get(COPY_FILES).and_then(serde_json::Value::as_bool) != Some(false)
}

fn restore_hint() -> RestoreHint {
    RestoreHint::Manual {
        key: RESTORE_KEY.to_owned(),
    }
}

/// `{DOCUMENTS}\<edition>\Modules`.
fn modules_root(env: &Environment, edition: &str) -> Option<PathBuf> {
    env.known_folder(KnownFolder::Documents)
        .map(|docs| docs.join(edition).join("Modules"))
}

/// A module folder (all its installed versions).
#[derive(Debug)]
pub(crate) struct Module {
    /// Edition, one of [`EDITIONS`].
    pub edition: &'static str,
    /// Folder name, the module name.
    pub name: String,
    /// The module folder.
    pub dir: PathBuf,
    /// Installed versions.
    pub versions: Vec<ModuleVersion>,
    /// Files of the folder (only listed when copying).
    pub files: Vec<ModuleFile>,
    /// Whether the files are copied.
    pub copied: bool,
}

impl Module {
    /// Size of the files of the module.
    pub(crate) fn bytes(&self) -> u64 {
        self.files.iter().map(|f| f.bytes).sum()
    }
}

/// One installed version of a module.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ModuleVersion {
    /// `ModuleVersion` of the manifest, else the name of the version folder.
    pub version: Option<String>,
    /// Folder relative to the edition's `Modules`: `<module>/<version>` or `<module>`.
    pub path: String,
}

/// A file of a module.
#[derive(Debug)]
pub(crate) struct ModuleFile {
    /// Path relative to the module folder.
    pub rel_path: PathBuf,
    /// Size in bytes.
    pub bytes: u64,
}

/// Names of the subfolders of `root` (links skipped), sorted case-insensitively.
fn module_dirs(root: &Path) -> io::Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort_by_cached_key(|name| (name.to_lowercase(), name.clone()));
    Ok(names)
}

/// Installed versions of the module `name` in `dir`: one per version folder
/// (`<dir>/1.2.3/<name>.psd1`), or the module itself when it has none
/// (`<dir>/<name>.psd1`). Manifests are read through `source`; a cloud-only one is not
/// opened and is recorded in `skipped` ([`Partial::cloud_only`]).
pub(crate) fn versions(
    dir: &Path,
    name: &str,
    source: &dyn SourceFs,
    skipped: &mut Partial,
) -> Vec<ModuleVersion> {
    let manifest = format!("{name}.psd1");
    let mut version_dirs: Vec<String> = module_dirs(dir)
        .unwrap_or_default()
        .into_iter()
        .filter(|sub| version_key(sub).is_some())
        .collect();
    if version_dirs.is_empty() {
        return vec![ModuleVersion {
            version: manifest_version(&dir.join(&manifest), source, skipped),
            path: name.to_owned(),
        }];
    }
    version_dirs.sort_by_key(|sub| version_key(sub));
    version_dirs
        .into_iter()
        .map(|sub| ModuleVersion {
            version: manifest_version(&dir.join(&sub).join(&manifest), source, skipped)
                .or_else(|| Some(sub.clone())),
            path: format!("{name}/{sub}"),
        })
        .collect()
}

/// `ModuleVersion` of the manifest at `path`; `None` when it is missing, unreadable, has
/// no such key or is only in the cloud (then it is not opened and is recorded in
/// `skipped`).
fn manifest_version(path: &Path, source: &dyn SourceFs, skipped: &mut Partial) -> Option<String> {
    match source::open_local(source, path) {
        Ok(Some(file)) => manifest::read_version(file),
        Ok(None) => {
            skipped.cloud_only(path);
            None
        }
        Err(err) => {
            if err.kind() != io::ErrorKind::NotFound {
                tracing::debug!(error = %err, "module manifest not read");
            }
            None
        }
    }
}

/// Numeric parts of a version folder name (`1.2`, `1.2.3`, `1.2.3.4`); `None` for any
/// other name.
fn version_key(name: &str) -> Option<Vec<u64>> {
    let parts: Option<Vec<u64>> = name.split('.').map(|p| p.parse().ok()).collect();
    parts.filter(|parts| (2..=4).contains(&parts.len()))
}

/// Every file under `dir` with its path relative to `dir`, sorted; links are not
/// followed, a subfolder that cannot be read is a warning.
fn list_files(dir: &Path, failures: &mut Partial) -> Result<Vec<ModuleFile>, ExportError> {
    let mut files = Vec::new();
    let mut pending = vec![PathBuf::new()];
    while let Some(rel) = pending.pop() {
        let path = dir.join(&rel);
        let entries = match std::fs::read_dir(&path) {
            Ok(entries) => entries,
            Err(err) => {
                failures.add_path(ISSUE_FOLDER_READ_FAILED, &path, err.into())?;
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => {
                    failures.add_path(ISSUE_FOLDER_READ_FAILED, &path, err.into())?;
                    continue;
                }
            };
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            let rel_path = rel.join(entry.file_name());
            if meta.is_dir() {
                pending.push(rel_path);
            } else if meta.is_file() {
                files.push(ModuleFile {
                    rel_path,
                    bytes: meta.len(),
                });
            }
        }
    }
    files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(files)
}

/// Marks the modules to copy (and the others not to): smallest first (then by edition and name) while the total
/// stays within `limit`; every other module gets the warning [`ISSUE_NOT_COPIED`].
pub(crate) fn select_for_copy(modules: &mut [Module], limit: u64, failures: &mut Partial) {
    let mut order: Vec<usize> = (0..modules.len()).collect();
    order.sort_by_key(|&i| {
        (
            modules[i].bytes(),
            modules[i].edition,
            modules[i].name.to_lowercase(),
        )
    });
    let mut total = 0u64;
    for i in order {
        let module = &mut modules[i];
        let bytes = module.bytes();
        module.copied = total.saturating_add(bytes) <= limit;
        if module.copied {
            total += bytes;
        } else {
            let args = BTreeMap::from([
                ("module".to_owned(), module.name.clone()),
                ("edition".to_owned(), module.edition.to_owned()),
                ("bytes".to_owned(), bytes.to_string()),
            ]);
            failures.note(warning(ISSUE_NOT_COPIED, args));
        }
    }
}

/// `modules.json`: `{ "modules": [{ "name", "version", "edition", "path", "copied" }] }`,
/// one entry per installed version, in edition and module order, UTF-8, ending with a
/// newline. `version` is `null` when unknown; `path` is relative to the edition's
/// `Modules` folder.
pub(crate) fn render_json(modules: &[Module]) -> String {
    let entries: Vec<_> = modules
        .iter()
        .flat_map(|module| {
            module.versions.iter().map(|v| {
                json!({
                    "name": module.name,
                    "version": v.version,
                    "edition": module.edition,
                    "path": v.path,
                    "copied": module.copied,
                })
            })
        })
        .collect();
    let mut text = serde_json::to_string_pretty(&json!({ "modules": entries })).unwrap_or_default();
    text.push('\n');
    text
}
