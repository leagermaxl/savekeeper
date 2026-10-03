//! EA app detector (SPEC-05 §4.4), best effort: games from the JSON files
//! `{PROGRAMDATA}\EA Desktop\InstallData\*\*.json` and from the registry
//! `HKLM\SOFTWARE\WOW6432Node\Electronic Arts\<Game>\Install Dir`.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Map, Value};
use sk_core::env::{Environment, InstalledGame, KnownFolder, LauncherInfo};
use sk_core::fs::{EntryKind, FsError, FsScanner};
use sk_core::model::{RegHive, ScanIssue};
use sk_core::path::eq_ci;
use sk_core::registry::{RegistryReader, SystemRegistry};

use super::vdf::{fs_reason, INVALID};
use super::{file_issue, file_name, folder_exists, install_path, is_folder, non_empty};
use super::{LauncherDetector, MAX_JSON};

const EA: &str = "ea";
pub(crate) const GAMES_KEY: &str = r"SOFTWARE\WOW6432Node\Electronic Arts";
const INSTALL_DIR: &str = "Install Dir";
/// JSON fields read, compared without ASCII case, in this order.
const DIR_FIELDS: [&str; 4] = [
    "installLocation",
    "installPath",
    "baseInstallPath",
    "installDir",
];
const NAME_FIELDS: [&str; 3] = ["displayName", "gameName", "title"];
const ID_FIELDS: [&str; 4] = ["softwareId", "contentId", "offerId", "productId"];

/// Detects the EA app (SPEC-05 §4.4).
#[derive(Clone)]
pub struct EaDetector {
    registry: Arc<dyn RegistryReader>,
}

impl EaDetector {
    /// A detector reading the registry of this machine.
    pub fn new() -> Self {
        Self::with_registry(Arc::new(SystemRegistry))
    }

    /// A detector reading `registry`.
    pub fn with_registry(registry: Arc<dyn RegistryReader>) -> Self {
        Self { registry }
    }

    /// Subkeys of `Electronic Arts` with an absolute `Install Dir`, by name;
    /// the name is `DisplayName`, else the subkey name, which is also the id.
    fn registry_games(&self) -> Vec<InstalledGame> {
        let mut keys = self.registry.subkeys(RegHive::Hklm, GAMES_KEY);
        keys.sort_by_key(|k| k.to_lowercase());
        keys.iter()
            .filter_map(|sub| {
                let sub = non_empty(sub)?;
                let key = format!(r"{GAMES_KEY}\{sub}");
                let value = |name| self.registry.string_value(RegHive::Hklm, &key, name);
                let install_dir = value(INSTALL_DIR).as_deref().and_then(install_path)?;
                let name = value("DisplayName")
                    .as_deref()
                    .and_then(non_empty)
                    .unwrap_or(sub)
                    .to_owned();
                Some(InstalledGame {
                    store_game_id: sub.to_owned(),
                    name,
                    install_dir,
                    size_bytes: None,
                    manifest_key: None,
                })
            })
            .collect()
    }
}

impl Default for EaDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for EaDetector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EaDetector").finish_non_exhaustive()
    }
}

impl LauncherDetector for EaDetector {
    fn id(&self) -> &'static str {
        EA
    }

    fn detect(&self, fs: &dyn FsScanner, env: &Environment) -> Option<LauncherInfo> {
        self.detect_with_issues(fs, env).0
    }

    /// Present if `{PROGRAMDATA}\EA Desktop` is a folder (the root) or the
    /// registry lists a game. Games of the JSON files come first; a registry
    /// game with the install folder of a known one is dropped.
    fn detect_with_issues(
        &self,
        fs: &dyn FsScanner,
        env: &Environment,
    ) -> (Option<LauncherInfo>, Vec<ScanIssue>) {
        let root = env
            .known_folder(KnownFolder::ProgramData)
            .map(|p| p.join("EA Desktop"))
            .filter(|p| folder_exists(fs, p));
        let mut issues = Vec::new();
        let mut games = root
            .as_deref()
            .map(|r| json_games(fs, env, r, &mut issues))
            .unwrap_or_default();
        for game in self.registry_games() {
            if !games
                .iter()
                .any(|g| eq_ci(&g.install_dir, &game.install_dir))
            {
                games.push(game);
            }
        }
        if root.is_none() && games.is_empty() {
            return (None, issues);
        }
        let launcher = LauncherInfo {
            id: EA.to_owned(),
            root,
            user_ids: Vec::new(),
            games,
        };
        (Some(launcher), issues)
    }
}

/// Games of `<root>\InstallData\<folder>\*.json`, folders and files by name.
/// A missing `InstallData` means no games.
fn json_games(
    fs: &dyn FsScanner,
    env: &Environment,
    root: &Path,
    issues: &mut Vec<ScanIssue>,
) -> Vec<InstalledGame> {
    let install_data = root.join("InstallData");
    let folders = match list(fs, &install_data, true) {
        Ok(folders) => folders,
        Err(FsError::NotFound) => return Vec::new(),
        Err(e) => {
            issues.push(file_issue(EA, &install_data, env, fs_reason(&e)));
            return Vec::new();
        }
    };
    let mut games: Vec<InstalledGame> = Vec::new();
    for folder in folders {
        let files = match list(fs, &folder, false) {
            Ok(files) => files,
            Err(e) => {
                issues.push(file_issue(EA, &folder, env, fs_reason(&e)));
                continue;
            }
        };
        let folder_name = file_name(&folder).unwrap_or_default();
        for file in files {
            let is_json = file
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("json"));
            if !is_json {
                continue;
            }
            let parsed = fs
                .read_small(&file, MAX_JSON)
                .map_err(|e| fs_reason(&e))
                .and_then(|bytes| parse_install_data(&bytes, &folder_name).ok_or(INVALID));
            match parsed {
                Ok(Some(game)) => {
                    if !games
                        .iter()
                        .any(|g| eq_ci(&g.install_dir, &game.install_dir))
                    {
                        games.push(game);
                    }
                }
                Ok(None) => {}
                Err(reason) => issues.push(file_issue(EA, &file, env, reason)),
            }
        }
    }
    games
}

/// Folders (`folders`) or files of `path`, sorted by name.
fn list(fs: &dyn FsScanner, path: &Path, folders: bool) -> Result<Vec<PathBuf>, FsError> {
    let mut out: Vec<PathBuf> = fs
        .read_dir(path)?
        .into_iter()
        .filter(|e| {
            if folders {
                is_folder(&e.meta)
            } else {
                e.meta.kind == EntryKind::File
            }
        })
        .map(|e| e.path)
        .collect();
    out.sort_by_key(|p| p.to_string_lossy().to_lowercase());
    Ok(out)
}

/// The game of an `InstallData` JSON file found in the folder `folder_name`:
/// the install folder from the first non-empty install folder field (it must
/// be absolute, else the record is not a game), name and id from the first
/// present field (else the folder name).
///
/// `Some(None)` for an object without an install folder (not a game record);
/// `None` if the file is not a JSON object.
pub(crate) fn parse_install_data(bytes: &[u8], folder_name: &str) -> Option<Option<InstalledGame>> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let value: Value = serde_json::from_slice(bytes).ok()?;
    let obj = value.as_object()?;
    let Some(install_dir) = first_text(obj, &DIR_FIELDS).and_then(install_path) else {
        return Some(None);
    };
    let fallback = non_empty(folder_name)
        .map(str::to_owned)
        .or_else(|| file_name(&install_dir))
        .unwrap_or_default();
    let name = first_text(obj, &NAME_FIELDS).map_or_else(|| fallback.clone(), str::to_owned);
    let id = first_text(obj, &ID_FIELDS).map_or(fallback, str::to_owned);
    Some(Some(InstalledGame {
        store_game_id: id,
        name,
        install_dir,
        size_bytes: None,
        manifest_key: None,
    }))
}

/// The first non-empty string among `fields`, keys compared without ASCII case.
fn first_text<'a>(obj: &'a Map<String, Value>, fields: &[&str]) -> Option<&'a str> {
    fields.iter().find_map(|field| {
        obj.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(field))
            .and_then(|(_, v)| v.as_str())
            .and_then(non_empty)
    })
}
