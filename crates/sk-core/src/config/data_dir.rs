//! Where the config and program data live (SPEC-01 §4.8.1, FR-01-05).

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use super::ConfigError;

const CONFIG_FILE: &str = "savekeeper.config.json";
const DATA_DIR: &str = "savekeeper-data";
const FALLBACK_DIR: &str = "SaveKeeper";

/// The config file and the `savekeeper-data` folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataDir {
    /// `savekeeper-data`.
    pub root: PathBuf,
    /// `savekeeper.config.json`.
    pub config_path: PathBuf,
    /// Next to the program (portable mode), not in `%LOCALAPPDATA%`.
    pub portable: bool,
}

impl DataDir {
    /// Next to the program if its folder is writable, else in
    /// `<local_app_data>\SaveKeeper`.
    pub fn locate(exe_dir: &Path, local_app_data: Option<&Path>) -> Result<DataDir, ConfigError> {
        if is_writable(exe_dir) {
            return Ok(Self::at(exe_dir, true));
        }
        let base = local_app_data
            .ok_or(ConfigError::NoDataDir)?
            .join(FALLBACK_DIR);
        std::fs::create_dir_all(&base).map_err(|_| ConfigError::NoDataDir)?;
        if !is_writable(&base) {
            return Err(ConfigError::NoDataDir);
        }
        Ok(Self::at(&base, false))
    }

    fn at(base: &Path, portable: bool) -> Self {
        Self {
            root: base.join(DATA_DIR),
            config_path: base.join(CONFIG_FILE),
            portable,
        }
    }

    /// `logs/`.
    pub fn logs(&self) -> PathBuf {
        self.root.join("logs")
    }

    /// `cache/`.
    pub fn cache(&self) -> PathBuf {
        self.root.join("cache")
    }

    /// `rules.d/`: user rules (SPEC-04).
    pub fn rules(&self) -> PathBuf {
        self.root.join("rules.d")
    }

    /// `scans/`: saved reports.
    pub fn scans(&self) -> PathBuf {
        self.root.join("scans")
    }

    /// Creates `savekeeper-data` and its subfolders.
    pub fn create_dirs(&self) -> std::io::Result<()> {
        for dir in [self.logs(), self.cache(), self.rules(), self.scans()] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }
}

/// Whether a probe file can be created and removed in `dir`.
fn is_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".savekeeper-write-test-{}", std::process::id()));
    match OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(file) => {
            drop(file);
            std::fs::remove_file(&probe).is_ok()
        }
        Err(_) => false,
    }
}
