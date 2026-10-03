//! Ubisoft Connect detector (SPEC-05 §4.4): launcher folder from
//! `HKLM\SOFTWARE\WOW6432Node\Ubisoft\Launcher\InstallDir`, games from
//! `…\Launcher\Installs\<id>\InstallDir`, accounts from `<root>\savegames`.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sk_core::env::{Environment, InstalledGame, KnownFolder, LauncherInfo, StoreUser};
use sk_core::fs::FsScanner;
use sk_core::model::RegHive;
use sk_core::registry::{KeyState, RegistryReader, SystemRegistry};

use super::LauncherDetector;
use super::{file_name, folder_exists, install_path, is_folder, native_path, non_empty};

pub(crate) const UBISOFT: &str = "ubisoft";
pub(crate) const LAUNCHER_KEY: &str = r"SOFTWARE\WOW6432Node\Ubisoft\Launcher";
const INSTALL_DIR: &str = "InstallDir";
/// Saves of all Ubisoft Connect games: `<root>\savegames\<userid>\<gameid>`.
pub(crate) const SAVEGAMES: &str = "savegames";

/// Detects Ubisoft Connect (SPEC-05 §4.4).
#[derive(Clone)]
pub struct UbisoftDetector {
    registry: Arc<dyn RegistryReader>,
}

impl UbisoftDetector {
    /// A detector reading the registry of this machine.
    pub fn new() -> Self {
        Self::with_registry(Arc::new(SystemRegistry))
    }

    /// A detector reading `registry`.
    pub fn with_registry(registry: Arc<dyn RegistryReader>) -> Self {
        Self { registry }
    }

    /// The launcher folder: `InstallDir` of the `Launcher` key, else
    /// `{PROGRAMFILES_X86}\Ubisoft\Ubisoft Game Launcher`; the first that is a folder.
    fn root(&self, fs: &dyn FsScanner, env: &Environment) -> Option<PathBuf> {
        let registered = self
            .registry
            .string_value(RegHive::Hklm, LAUNCHER_KEY, INSTALL_DIR)
            .map(|s| native_path(&s))
            .filter(|p| p.is_absolute());
        let default = env
            .known_folder(KnownFolder::ProgramFilesX86)
            .map(|p| p.join("Ubisoft").join("Ubisoft Game Launcher"));
        registered
            .into_iter()
            .chain(default)
            .find(|p| folder_exists(fs, p))
    }

    /// Subkeys of `Installs` with an absolute `InstallDir`, by id; the name
    /// is the folder name (the registry has no game names).
    fn games(&self) -> Vec<InstalledGame> {
        let installs = format!(r"{LAUNCHER_KEY}\Installs");
        let mut ids = self.registry.subkeys(RegHive::Hklm, &installs);
        ids.sort_by_key(|id| id.to_lowercase());
        ids.iter()
            .filter_map(|id| {
                let id = non_empty(id)?;
                let key = format!(r"{installs}\{id}");
                let install_dir = self
                    .registry
                    .string_value(RegHive::Hklm, &key, INSTALL_DIR)
                    .as_deref()
                    .and_then(install_path)?;
                Some(InstalledGame {
                    store_game_id: id.to_owned(),
                    name: file_name(&install_dir).unwrap_or_else(|| id.to_owned()),
                    install_dir,
                    size_bytes: None,
                    manifest_key: None,
                })
            })
            .collect()
    }
}

impl Default for UbisoftDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for UbisoftDetector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UbisoftDetector").finish_non_exhaustive()
    }
}

impl LauncherDetector for UbisoftDetector {
    fn id(&self) -> &'static str {
        UBISOFT
    }

    /// Present if the launcher folder or the `Installs` key exists.
    fn detect(&self, fs: &dyn FsScanner, env: &Environment) -> Option<LauncherInfo> {
        let root = self.root(fs, env);
        let installs = format!(r"{LAUNCHER_KEY}\Installs");
        if root.is_none() && self.registry.key_state(RegHive::Hklm, &installs) != KeyState::Present
        {
            return None;
        }
        Some(LauncherInfo {
            id: UBISOFT.to_owned(),
            user_ids: root.as_deref().map(|r| users(fs, r)).unwrap_or_default(),
            games: self.games(),
            root,
        })
    }
}

/// Accounts: the folders of `<root>\savegames`, by name.
fn users(fs: &dyn FsScanner, root: &Path) -> Vec<StoreUser> {
    let Ok(entries) = fs.read_dir(&root.join(SAVEGAMES)) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = entries
        .iter()
        .filter(|e| is_folder(&e.meta))
        .filter_map(|e| file_name(&e.path))
        .collect();
    ids.sort_by_key(|id| id.to_lowercase());
    ids.into_iter()
        .map(|id| StoreUser {
            id,
            alt_id: None,
            name: None,
        })
        .collect()
}
