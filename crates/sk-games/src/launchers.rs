//! Launcher detection (SPEC-05 §4.4): what is installed, where, and for
//! which store accounts. The results fill `Environment.launchers`.

mod battlenet;
mod ea;
mod enrich;
mod epic;
mod findings;
mod gog;
mod steam;
mod ubisoft;
mod vdf;
mod xbox;

#[cfg(test)]
mod ea_tests;
#[cfg(test)]
mod enrich_tests;
#[cfg(test)]
mod epic_tests;
#[cfg(test)]
mod registry_tests;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod xbox_tests;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf, MAIN_SEPARATOR_STR};

use sk_core::env::{Environment, LauncherInfo};
use sk_core::fs::{EntryKind, EntryMeta, FsScanner, ReparseKind};
use sk_core::model::{IssueSeverity, ScanIssue};
use sk_core::template::PathTemplate;

pub use battlenet::BattleNetDetector;
pub use ea::EaDetector;
pub use enrich::{enrich, enrich_with_registry};
pub use epic::EpicDetector;
pub use gog::GogDetector;
pub use steam::{SteamDetector, STEAM_ID64_BASE};
pub use ubisoft::UbisoftDetector;
pub use xbox::XboxDetector;

/// Detects one launcher and its installed games (SPEC-05 §4.1).
pub trait LauncherDetector: Send + Sync {
    /// Launcher id: "steam", "epic", "gog", "ubisoft", "ea", "battlenet", "xbox".
    fn id(&self) -> &'static str;

    /// The launcher, or `None` if it is not installed. Only reads.
    fn detect(&self, fs: &dyn FsScanner, env: &Environment) -> Option<LauncherInfo>;

    /// [`detect`](Self::detect) with the problems met on the way, as
    /// `ScanIssue::Info` with `source: "games.<launcher>"` (SPEC-05 §4.4).
    /// The default reports no issues.
    fn detect_with_issues(
        &self,
        fs: &dyn FsScanner,
        env: &Environment,
    ) -> (Option<LauncherInfo>, Vec<ScanIssue>) {
        (self.detect(fs, env), Vec::new())
    }
}

/// A launcher file or folder cannot be read or is not valid: what it lists
/// is skipped (every launcher except Steam, SPEC-05 §5).
pub(crate) const ISSUE_LAUNCHER_FILE: &str = "issue.games.launcher_file_unreadable";

/// Size limit of the JSON files of Epic and EA; real ones are a few KiB.
const MAX_JSON: usize = 1 << 20;

/// `FILE_ATTRIBUTE_DIRECTORY`.
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;

/// An `Info` issue of launcher `launcher` (SPEC-05 §4.4).
fn info_issue(launcher: &str, key: &str, path: Option<String>, args: &[(&str, &str)]) -> ScanIssue {
    ScanIssue {
        severity: IssueSeverity::Info,
        source: format!("games.{launcher}"),
        path,
        message_key: key.to_owned(),
        message_args: args
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect::<BTreeMap<_, _>>(),
    }
}

/// [`ISSUE_LAUNCHER_FILE`] of `launcher` for `path` (as a template).
fn file_issue(launcher: &str, path: &Path, env: &Environment, reason: &str) -> ScanIssue {
    let template = PathTemplate::from_path(path, env).to_string();
    info_issue(
        launcher,
        ISSUE_LAUNCHER_FILE,
        Some(template),
        &[("reason", reason)],
    )
}

/// A path from launcher files or the registry in native form: `/` becomes the
/// platform separator (`SteamPath` is written as `c:/program files (x86)/steam`),
/// trailing separators are dropped except after a drive (`C:\`).
fn native_path(s: &str) -> PathBuf {
    let s = s.trim().replace('/', MAIN_SEPARATOR_STR);
    let trimmed = s.trim_end_matches(['\\', '/']);
    if trimmed.len() == 2 && trimmed.ends_with(':') {
        return PathBuf::from(format!("{trimmed}{MAIN_SEPARATOR_STR}"));
    }
    PathBuf::from(trimmed)
}

/// An install folder written by a launcher: [`native_path`], if absolute.
fn install_path(s: &str) -> Option<PathBuf> {
    Some(native_path(s)).filter(|p| p.is_absolute())
}

/// A folder, or a link to one (a launcher folder may be moved by a junction).
fn is_folder(meta: &EntryMeta) -> bool {
    match meta.kind {
        EntryKind::Dir => true,
        EntryKind::Reparse(ReparseKind::Junction | ReparseKind::Symlink) => {
            meta.attrs & FILE_ATTRIBUTE_DIRECTORY != 0
        }
        EntryKind::File | EntryKind::Reparse(_) => false,
    }
}

/// Whether `path` is a folder (or a link to one) on `fs`.
fn folder_exists(fs: &dyn FsScanner, path: &Path) -> bool {
    fs.metadata(path).is_ok_and(|m| is_folder(&m))
}

/// The trimmed string, if not empty.
fn non_empty(s: &str) -> Option<&str> {
    Some(s.trim()).filter(|s| !s.is_empty())
}

/// The last component of a path as text (the folder name of an install dir).
fn file_name(path: &Path) -> Option<String> {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
}
