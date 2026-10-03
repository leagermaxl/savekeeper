//! Battle.net detector (SPEC-05 §4.4), MVP: the `HKLM` Uninstall entries
//! with publisher "Blizzard Entertainment". `product.db` is not read.

use std::fmt;
use std::sync::Arc;

use sk_core::env::{Environment, InstalledGame, LauncherInfo};
use sk_core::fs::FsScanner;
use sk_core::model::RegHive;
use sk_core::registry::{RegistryReader, SystemRegistry};

use super::{install_path, non_empty, LauncherDetector};

const BATTLENET: &str = "battlenet";
/// Uninstall keys read: 32-bit programs first (Blizzard writes there), then 64-bit.
pub(crate) const UNINSTALL_KEYS: [&str; 2] = [
    r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
    r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
];
const PUBLISHER: &str = "Blizzard Entertainment";
/// Uninstall entry of the launcher itself.
const LAUNCHER_ENTRY: &str = "Battle.net";

/// Detects Battle.net (SPEC-05 §4.4) by Blizzard's Uninstall entries.
#[derive(Clone)]
pub struct BattleNetDetector {
    registry: Arc<dyn RegistryReader>,
}

impl BattleNetDetector {
    /// A detector reading the registry of this machine.
    pub fn new() -> Self {
        Self::with_registry(Arc::new(SystemRegistry))
    }

    /// A detector reading `registry`.
    pub fn with_registry(registry: Arc<dyn RegistryReader>) -> Self {
        Self { registry }
    }
}

impl Default for BattleNetDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for BattleNetDetector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BattleNetDetector").finish_non_exhaustive()
    }
}

impl LauncherDetector for BattleNetDetector {
    fn id(&self) -> &'static str {
        BATTLENET
    }

    /// Present if an Uninstall entry has a `Publisher` starting with
    /// "Blizzard Entertainment" (without case). The entry `Battle.net` gives
    /// the root (`InstallLocation`); every other one with an absolute
    /// `InstallLocation` is a game: id = entry name, name = `DisplayName`,
    /// size = `EstimatedSize` (KiB). An entry name seen in the first key is
    /// not read again from the second.
    fn detect(&self, _fs: &dyn FsScanner, _env: &Environment) -> Option<LauncherInfo> {
        let mut found = false;
        let mut root = None;
        let mut seen: Vec<String> = Vec::new();
        let mut games: Vec<InstalledGame> = Vec::new();
        for uninstall in UNINSTALL_KEYS {
            let mut entries = self.registry.subkeys(RegHive::Hklm, uninstall);
            entries.sort_by_key(|e| e.to_lowercase());
            for entry in entries {
                if seen.iter().any(|s| s.eq_ignore_ascii_case(&entry)) {
                    continue;
                }
                seen.push(entry.clone());
                let key = format!(r"{uninstall}\{entry}");
                let value = |name| self.registry.string_value(RegHive::Hklm, &key, name);
                let blizzard = value("Publisher").is_some_and(|p| {
                    p.trim()
                        .get(..PUBLISHER.len())
                        .is_some_and(|head| head.eq_ignore_ascii_case(PUBLISHER))
                });
                if !blizzard {
                    continue;
                }
                found = true;
                let location = value("InstallLocation").as_deref().and_then(install_path);
                if entry.eq_ignore_ascii_case(LAUNCHER_ENTRY) {
                    root = root.or(location);
                    continue;
                }
                let Some(install_dir) = location else {
                    continue;
                };
                let name = value("DisplayName")
                    .as_deref()
                    .and_then(non_empty)
                    .map_or_else(|| entry.clone(), str::to_owned);
                let size_bytes = self
                    .registry
                    .dword_value(RegHive::Hklm, &key, "EstimatedSize")
                    .map(|kib| u64::from(kib) * 1024);
                games.push(InstalledGame {
                    store_game_id: entry,
                    name,
                    install_dir,
                    size_bytes,
                    manifest_key: None,
                });
            }
        }
        found.then(|| LauncherInfo {
            id: BATTLENET.to_owned(),
            root,
            user_ids: Vec::new(),
            games,
        })
    }
}
