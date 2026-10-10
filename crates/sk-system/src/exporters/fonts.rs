//! The `fonts` exporter (SPEC-06 §4.3): copies of the fonts installed for the current
//! user, `reg export` of their registration in `HKCU`, and a list of the fonts in
//! `{WINDIR}\Fonts` that do not ship with Windows (only listed: they belong to programs
//! or were installed for all users and are usually reinstalled with them).

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use sk_core::env::{Environment, KnownFolder};
use sk_core::model::{Category, Finding, RegHive, Sensitivity};
use sk_core::registry::{RegistryReader, SystemRegistry};

use super::partial::{
    Partial, ISSUE_EXPORT_FAILED, ISSUE_FILE_COPY_FAILED, ISSUE_FOLDER_READ_FAILED,
};
use super::source::{self, SourceFs};
use super::{copy_file, plan_finding, unavailable, write_file, Plan, NOT_WINDOWS};
use crate::error::ExportError;
use crate::exporter::{
    Availability, ExportContext, ExportResult, ExportedFile, RestoreHint, SystemExporter,
};
use crate::reg_export::{windir, RegExport};

/// Exporter identifier.
pub(crate) const ID: &str = "fonts";

/// Folder of the copied user fonts in `system/fonts/`.
pub(crate) const USER_DIR: &str = "user";

/// `reg export` of [`FONTS_KEY`].
pub(crate) const REG_FILE: &str = "fonts.reg";

/// List of the fonts in `{WINDIR}\Fonts` that do not ship with Windows.
pub(crate) const NONSTANDARD_FILE: &str = "system-fonts-nonstandard.txt";

/// Registration of the user's fonts (`HKCU`): font name → file path.
pub(crate) const FONTS_KEY: &str = r"Software\Microsoft\Windows NT\CurrentVersion\Fonts";

/// Finding title (i18n key).
const TITLE: &str = "system.title.fonts";

/// `reason_key`: no user fonts, neither files nor registrations.
const NONE: &str = "system.fonts.none";

/// Restore instructions (i18n key): install the fonts of `user/` for the user.
const RESTORE_KEY: &str = "system.restore.fonts";

/// Extensions of font files (lowercase).
const FONT_EXTENSIONS: [&str; 8] = ["ttf", "ttc", "otf", "otc", "fon", "fnt", "pfb", "pfm"];

/// Names of the font files shipped with Windows: one lowercase pattern per line, `*`
/// matches any run of characters, `#` starts a comment.
const STANDARD_FONTS: &str = include_str!("fonts/windows-fonts.txt");

/// Fonts of the user and the list of non-standard system fonts.
pub(crate) struct FontsExporter {
    reg: RegExport,
    /// Registry read by `detect` (registrations of user fonts).
    registry: Arc<dyn RegistryReader>,
    /// The user's font files; cloud-only ones are not opened (SPEC-10 FR-10-15).
    source: Arc<dyn SourceFs>,
}

impl FontsExporter {
    /// The exporter with `reg.exe` and the registry of this machine.
    pub(crate) fn new() -> Self {
        Self {
            reg: RegExport::system(),
            registry: Arc::new(SystemRegistry),
            source: source::real(),
        }
    }

    /// The exporter with the given `reg export` and registry of `detect` (tests).
    #[cfg(test)]
    pub(crate) fn with(reg: RegExport, registry: Arc<dyn RegistryReader>) -> Self {
        Self {
            reg,
            registry,
            source: source::real(),
        }
    }

    /// The exporter reading the font files through `source` (tests).
    #[cfg(test)]
    pub(crate) fn with_source(self, source: Arc<dyn SourceFs>) -> Self {
        Self { source, ..self }
    }

    /// `detect` without the platform check (SPEC-06 §4.3): Available when
    /// `{LOCALAPPDATA}\Microsoft\Windows\Fonts` holds font files or
    /// `HKCU\Software\Microsoft\Windows NT\CurrentVersion\Fonts` has values. Details:
    /// `user_fonts` (files), `registered_fonts` (values); the estimate is the size of the
    /// files. `{WINDIR}\Fonts` is only read by `run`.
    pub(crate) fn probe(&self, env: &Environment) -> Availability {
        let fonts = user_font_dir(env)
            .map(|dir| match list_fonts(&dir) {
                Ok(fonts) => fonts,
                Err(err) => {
                    tracing::debug!(error = %err, "user font folder not read");
                    Vec::new()
                }
            })
            .unwrap_or_default();
        let registered = self
            .registry
            .values(RegHive::Hkcu, FONTS_KEY)
            .map_or(0, |values| values.len());
        if fonts.is_empty() && registered == 0 {
            return unavailable(NONE);
        }
        Availability::Available {
            estimated_bytes: Some(fonts.iter().map(|f| f.bytes).sum()),
            details: BTreeMap::from([
                ("user_fonts".to_owned(), fonts.len().to_string()),
                ("registered_fonts".to_owned(), registered.to_string()),
            ]),
        }
    }

    /// Copies the user fonts to `user/<name>`; a file or folder that fails is a warning,
    /// a cloud-only font is not opened (warning `issue.system.cloud_only`).
    fn copy_user_fonts(
        &self,
        ctx: &ExportContext<'_>,
        files: &mut Vec<ExportedFile>,
        failures: &mut Partial,
    ) -> Result<(), ExportError> {
        let Some(dir) = user_font_dir(ctx.env) else {
            return Ok(());
        };
        let fonts = match list_fonts(&dir) {
            Ok(fonts) => fonts,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return failures.add_path(ISSUE_FOLDER_READ_FAILED, &dir, err.into()),
        };
        for font in fonts {
            if ctx.cancel.is_cancelled() {
                return Err(ExportError::Cancelled);
            }
            let src = dir.join(&font.name);
            let rel_path = Path::new(USER_DIR).join(&font.name);
            match copy_file(self.source.as_ref(), &src, ctx.target_dir, rel_path) {
                Ok(Some(file)) => files.push(file),
                Ok(None) => failures.cloud_only(&src),
                Err(err) => failures.add_path(ISSUE_FILE_COPY_FAILED, &src, err.into())?,
            }
        }
        Ok(())
    }
}

#[async_trait]
impl SystemExporter for FontsExporter {
    fn id(&self) -> &'static str {
        ID
    }

    /// See [`FontsExporter::probe`]. Reasons: `system.fonts.none`, `system.not_windows`.
    fn detect(&self, env: &Environment) -> Availability {
        if cfg!(windows) {
            self.probe(env)
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

    /// Writes `user/<font file>` for every font file of the user's font folder,
    /// `fonts.reg` (`reg export` of the `HKCU` font registrations) and
    /// `system-fonts-nonstandard.txt` (names of the font files in `{WINDIR}\Fonts` not in
    /// the built-in list of Windows fonts, one per line; only when there are any).
    ///
    /// A missing user font folder or registry key is skipped silently; a file or folder
    /// that cannot be read and a failed `reg export` are warnings. A user font that is
    /// only in the cloud is not opened: warning `issue.system.cloud_only` {path} (SPEC-10
    /// FR-10-15); the system fonts are only listed, never opened. When nothing is
    /// written, the first error is returned (with no error: a result without files when
    /// cloud-only fonts were skipped, else [`ExportError::NotAvailable`]). Cancellation
    /// stops the export.
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
        self.copy_user_fonts(ctx, &mut files, &mut failures)?;

        let out_file = ctx.target_dir.join(REG_FILE);
        let result = self
            .reg
            .export(
                windir(ctx.env),
                RegHive::Hkcu,
                FONTS_KEY,
                &out_file,
                ctx.cancel,
            )
            .await;
        match result {
            Ok(file) => files.push(file),
            // No registrations of user fonts: nothing to export.
            Err(ExportError::NotAvailable) => tracing::debug!("no HKCU font registrations"),
            Err(err) => failures.add(RegHive::Hkcu, FONTS_KEY, ISSUE_EXPORT_FAILED, err)?,
        }

        if ctx.cancel.is_cancelled() {
            return Err(ExportError::Cancelled);
        }
        if let Some(dir) = windir(ctx.env).map(|windir| windir.join("Fonts")) {
            match list_fonts(&dir) {
                Ok(fonts) => {
                    let names = nonstandard(fonts.iter().map(|f| f.name.as_os_str()));
                    if !names.is_empty() {
                        let text: String = names.iter().map(|n| format!("{n}\r\n")).collect();
                        files.push(write_file(
                            ctx.target_dir,
                            NONSTANDARD_FILE,
                            text.as_bytes(),
                        )?);
                    }
                }
                Err(err) => failures.add_path(ISSUE_FOLDER_READ_FAILED, &dir, err.into())?,
            }
        }

        failures.finish(
            files,
            RestoreHint::Manual {
                key: RESTORE_KEY.to_owned(),
            },
        )
    }
}

/// `{LOCALAPPDATA}\Microsoft\Windows\Fonts`, where Windows installs fonts "for the
/// current user".
fn user_font_dir(env: &Environment) -> Option<PathBuf> {
    env.known_folder(KnownFolder::LocalAppData)
        .map(|dir| dir.join("Microsoft").join("Windows").join("Fonts"))
}

/// A font file of a folder.
#[derive(Debug)]
struct FontFile {
    /// File name.
    name: OsString,
    /// Size in bytes.
    bytes: u64,
}

/// The font files (by [`FONT_EXTENSIONS`]) directly in `dir`, sorted by name; links and
/// folders are skipped.
fn list_fonts(dir: &Path) -> io::Result<Vec<FontFile>> {
    let mut fonts = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.is_file() && is_font(Path::new(&entry.file_name())) {
            fonts.push(FontFile {
                name: entry.file_name(),
                bytes: meta.len(),
            });
        }
    }
    fonts.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(fonts)
}

/// Whether `path` has a font extension (any case).
fn is_font(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            FONT_EXTENSIONS
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known))
        })
}

/// The names among `names` that are not Windows fonts ([`is_standard`]), sorted
/// case-insensitively.
pub(crate) fn nonstandard<'a>(names: impl IntoIterator<Item = &'a std::ffi::OsStr>) -> Vec<String> {
    let mut out: Vec<String> = names
        .into_iter()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !is_standard(name))
        .collect();
    out.sort_by_cached_key(|name| (name.to_lowercase(), name.clone()));
    out
}

/// Whether the font file `name` ships with Windows: it matches a pattern of
/// [`STANDARD_FONTS`], ignoring case.
pub(crate) fn is_standard(name: &str) -> bool {
    let name = name.to_lowercase();
    standard_patterns().any(|pattern| glob_match(pattern, &name))
}

/// The patterns of [`STANDARD_FONTS`].
pub(crate) fn standard_patterns() -> impl Iterator<Item = &'static str> {
    STANDARD_FONTS
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
}

/// Whether `name` matches `pattern`, where `*` matches any run of characters (also an
/// empty one) and every other character itself.
pub(crate) fn glob_match(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    let (mut p, mut n) = (0, 0);
    // Position after the last `*` and the name position it was tried at.
    let mut backtrack: Option<(usize, usize)> = None;
    while n < name.len() {
        if p < pattern.len() && pattern[p] == '*' {
            p += 1;
            backtrack = Some((p, n));
        } else if p < pattern.len() && pattern[p] == name[n] {
            p += 1;
            n += 1;
        } else if let Some((star_p, star_n)) = backtrack {
            p = star_p;
            n = star_n + 1;
            backtrack = Some((star_p, n));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}
