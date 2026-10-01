//! The machine and user the scan runs on (SPEC-02 §3.3).
//!
//! `Environment::detect()` reads the real system (Windows; a minimal stub on
//! other platforms). `Environment::fake()` builds a Windows-like layout under a
//! root folder for cross-platform tests. `launchers`, `installed_programs` and
//! non-OneDrive `cloud_roots` are filled later by feature crates.

mod fake;
#[cfg(not(windows))]
mod other;
mod types;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use specta::Type;

pub use types::{
    CloudProvider, CloudRoot, DriveInfo, DriveKind, DriveMedia, InstalledGame, InstalledProgram,
    KnownFolder, LauncherInfo, OsInfo, ProgramSource, StoreUser,
};

/// The machine and user the scan runs on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub struct Environment {
    /// Operating system version.
    pub os: OsInfo,
    /// Computer name.
    pub machine_name: String,
    /// Login name of the current user.
    pub user_name: String,
    /// Security identifier of the current user, "S-1-5-21-...".
    pub user_sid: Option<String>,
    /// The process runs with administrator rights.
    pub is_elevated: bool,
    /// Known folders that exist on this machine.
    #[serde(with = "crate::serde_util::lossy_path_map")]
    #[specta(type = BTreeMap<KnownFolder, String>)]
    pub known_folders: BTreeMap<KnownFolder, PathBuf>,
    /// Logical drives.
    pub drives: Vec<DriveInfo>,
    /// Cloud sync roots: OneDrive from `detect()`, others from SPEC-03.
    pub cloud_roots: Vec<CloudRoot>,
    /// Game launchers, filled by SPEC-05.
    pub launchers: Vec<LauncherInfo>,
    /// Installed programs, filled by SPEC-06 §4.4.
    pub installed_programs: Vec<InstalledProgram>,
    /// Lowercase executable names of running processes, sorted, no duplicates.
    pub running_processes: Vec<String>,
}

/// Failure to read data the environment cannot do without.
#[derive(Debug, thiserror::Error)]
pub enum EnvError {
    /// An OS call for required data (profile, user name) failed.
    #[error("{api} failed with code {code:#010x}")]
    Os {
        /// Name of the failed API.
        api: &'static str,
        /// Error code (HRESULT or Win32 error).
        code: i32,
    },
    /// The `Home` known folder is not available.
    #[error("the user profile folder is not available")]
    MissingHome,
}

impl Environment {
    /// Reads the current machine and user.
    ///
    /// Missing optional folders, OneDrive, drive or process information leave
    /// the corresponding fields empty; only missing required data is an error.
    pub fn detect() -> Result<Self, EnvError> {
        #[cfg(windows)]
        let env = crate::win::env::detect()?;
        #[cfg(not(windows))]
        let env = other::detect()?;
        if !env.known_folders.contains_key(&KnownFolder::Home) {
            return Err(EnvError::MissingHome);
        }
        Ok(env)
    }

    /// A Windows-like environment with all known folders under `root`
    /// (SPEC-02 §3.3). Folders are not created on disk.
    pub fn fake(root: &Path) -> Self {
        fake::fake(root)
    }

    /// Path of a known folder, if it exists on this machine.
    pub fn known_folder(&self, folder: KnownFolder) -> Option<&Path> {
        self.known_folders.get(&folder).map(PathBuf::as_path)
    }
}

#[cfg(test)]
mod tests;
