//! GOG detector (SPEC-05 §4.4): games from the registry
//! `HKLM\SOFTWARE\WOW6432Node\GOG.com\Games\<id>` (`gameName`, `path`).
//! GOG Galaxy's database is not read (P2).

use std::fmt;
use std::sync::Arc;

use sk_core::env::{Environment, InstalledGame, LauncherInfo};
use sk_core::fs::FsScanner;
use sk_core::model::RegHive;
use sk_core::registry::{KeyState, RegistryReader, SystemRegistry};

use super::{file_name, install_path, non_empty, LauncherDetector};

const GOG: &str = "gog";
pub(crate) const GAMES_KEY: &str = r"SOFTWARE\WOW6432Node\GOG.com\Games";

/// Detects GOG (SPEC-05 §4.4) by the registry key of its games.
#[derive(Clone)]
pub struct GogDetector {
    registry: Arc<dyn RegistryReader>,
}

impl GogDetector {
    /// A detector reading the registry of this machine.
    pub fn new() -> Self {
        Self::with_registry(Arc::new(SystemRegistry))
    }

    /// A detector reading `registry`.
    pub fn with_registry(registry: Arc<dyn RegistryReader>) -> Self {
        Self { registry }
    }
}

impl Default for GogDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for GogDetector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GogDetector").finish_non_exhaustive()
    }
}

impl LauncherDetector for GogDetector {
    fn id(&self) -> &'static str {
        GOG
    }

    /// Present if the `Games` key exists. A game is a subkey with an absolute
    /// `path`; the subkey name is the product id, `gameName` the name (else
    /// the folder name). Subkeys without a valid `path` (left by an
    /// uninstall) are skipped.
    fn detect(&self, _fs: &dyn FsScanner, _env: &Environment) -> Option<LauncherInfo> {
        if self.registry.key_state(RegHive::Hklm, GAMES_KEY) != KeyState::Present {
            return None;
        }
        let mut ids = self.registry.subkeys(RegHive::Hklm, GAMES_KEY);
        ids.sort_by_key(|id| id.to_lowercase());
        let games = ids
            .iter()
            .filter_map(|id| {
                let id = non_empty(id)?;
                let key = format!(r"{GAMES_KEY}\{id}");
                let value = |name| self.registry.string_value(RegHive::Hklm, &key, name);
                let install_dir = value("path").as_deref().and_then(install_path)?;
                let name = value("gameName")
                    .as_deref()
                    .and_then(non_empty)
                    .map(str::to_owned)
                    .or_else(|| file_name(&install_dir))
                    .unwrap_or_else(|| id.to_owned());
                Some(InstalledGame {
                    store_game_id: id.to_owned(),
                    name,
                    install_dir,
                    size_bytes: None,
                    manifest_key: None,
                })
            })
            .collect();
        Some(LauncherInfo {
            id: GOG.to_owned(),
            root: None,
            user_ids: Vec::new(),
            games,
        })
    }
}
