//! Xbox / Microsoft Store detector (SPEC-05 §4.4): packages of
//! `{LOCALAPPDATA}\Packages\*` with a `SystemAppData\wgs` folder.
//!
//! The `wgs` saves are encrypted per container and normally synced through
//! Xbox Cloud: they become one `xbox.wgs` finding per package
//! ([`super::findings`]), never decrypted. The packages give no install
//! folders, so the launcher lists no games.

use std::path::{Path, PathBuf};

use sk_core::env::{Environment, KnownFolder, LauncherInfo};
use sk_core::fs::{FsError, FsScanner};
use sk_core::model::ScanIssue;

use super::vdf::fs_reason;
use super::{file_issue, file_name, folder_exists, is_folder, LauncherDetector};

pub(crate) const XBOX: &str = "xbox";

/// Detects Xbox / Microsoft Store games by their `wgs` save folders
/// (SPEC-05 §4.4).
#[derive(Debug, Clone, Copy, Default)]
pub struct XboxDetector;

impl XboxDetector {
    /// A detector reading the file system it is given.
    pub fn new() -> Self {
        Self
    }
}

impl LauncherDetector for XboxDetector {
    fn id(&self) -> &'static str {
        XBOX
    }

    fn detect(&self, fs: &dyn FsScanner, env: &Environment) -> Option<LauncherInfo> {
        self.detect_with_issues(fs, env).0
    }

    /// Present if at least one package has a `SystemAppData\wgs` folder.
    fn detect_with_issues(
        &self,
        fs: &dyn FsScanner,
        env: &Environment,
    ) -> (Option<LauncherInfo>, Vec<ScanIssue>) {
        let (folders, issues) = wgs_folders(fs, env);
        let launcher = (!folders.is_empty()).then(|| LauncherInfo {
            id: XBOX.to_owned(),
            root: None,
            user_ids: Vec::new(),
            games: Vec::new(),
        });
        (launcher, issues)
    }
}

/// The `wgs` save folder of one Store package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WgsFolder {
    /// Package family name: the folder name under `Packages`.
    pub(crate) package: String,
    /// `{LOCALAPPDATA}\Packages\<package>\SystemAppData\wgs`.
    pub(crate) path: PathBuf,
}

/// The `wgs` folders of `{LOCALAPPDATA}\Packages\*`, by package name
/// (without case). A missing `Packages` folder gives nothing; one that cannot
/// be listed gives an issue.
pub(crate) fn wgs_folders(
    fs: &dyn FsScanner,
    env: &Environment,
) -> (Vec<WgsFolder>, Vec<ScanIssue>) {
    let Some(packages) = env
        .known_folder(KnownFolder::LocalAppData)
        .map(|p| p.join("Packages"))
    else {
        return (Vec::new(), Vec::new());
    };
    let entries = match fs.read_dir(&packages) {
        Ok(entries) => entries,
        Err(FsError::NotFound) => return (Vec::new(), Vec::new()),
        Err(e) => {
            return (
                Vec::new(),
                vec![file_issue(XBOX, &packages, env, fs_reason(&e))],
            )
        }
    };
    let mut folders: Vec<WgsFolder> = entries
        .iter()
        .filter(|e| is_folder(&e.meta))
        .filter_map(|e| {
            let path = wgs_path(&e.path);
            if !folder_exists(fs, &path) {
                return None;
            }
            Some(WgsFolder {
                package: file_name(&e.path)?,
                path,
            })
        })
        .collect();
    folders.sort_by_key(|f| f.package.to_lowercase());
    (folders, Vec::new())
}

fn wgs_path(package: &Path) -> PathBuf {
    package.join("SystemAppData").join("wgs")
}
