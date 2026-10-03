//! Epic Games Launcher detector (SPEC-05 §4.4): games from the JSON
//! `{PROGRAMDATA}\Epic\EpicGamesLauncher\Data\Manifests\*.item` files.

use std::path::{Path, PathBuf};

use serde_json::Value;
use sk_core::env::{Environment, InstalledGame, KnownFolder, LauncherInfo};
use sk_core::fs::{EntryKind, FsError, FsScanner};
use sk_core::model::ScanIssue;

use super::vdf::{fs_reason, INVALID};
use super::{file_issue, folder_exists, install_path, non_empty, LauncherDetector, MAX_JSON};

const EPIC: &str = "epic";

/// Detects the Epic Games Launcher (SPEC-05 §4.4) by its data folder
/// `{PROGRAMDATA}\Epic\EpicGamesLauncher`.
#[derive(Debug, Clone, Copy, Default)]
pub struct EpicDetector;

impl EpicDetector {
    /// A detector reading the file system it is given.
    pub fn new() -> Self {
        Self
    }
}

impl LauncherDetector for EpicDetector {
    fn id(&self) -> &'static str {
        EPIC
    }

    fn detect(&self, fs: &dyn FsScanner, env: &Environment) -> Option<LauncherInfo> {
        self.detect_with_issues(fs, env).0
    }

    fn detect_with_issues(
        &self,
        fs: &dyn FsScanner,
        env: &Environment,
    ) -> (Option<LauncherInfo>, Vec<ScanIssue>) {
        let Some(root) = env
            .known_folder(KnownFolder::ProgramData)
            .map(|p| p.join("Epic").join("EpicGamesLauncher"))
            .filter(|p| folder_exists(fs, p))
        else {
            return (None, Vec::new());
        };
        let mut issues = Vec::new();
        let games = games(fs, env, &root, &mut issues);
        let launcher = LauncherInfo {
            id: EPIC.to_owned(),
            root: Some(root),
            user_ids: Vec::new(),
            games,
        };
        (Some(launcher), issues)
    }
}

/// Games of the `Data\Manifests\*.item` files, by file name; one per
/// `AppName`. A missing `Manifests` folder means no games.
fn games(
    fs: &dyn FsScanner,
    env: &Environment,
    root: &Path,
    issues: &mut Vec<ScanIssue>,
) -> Vec<InstalledGame> {
    let manifests = root.join("Data").join("Manifests");
    let entries = match fs.read_dir(&manifests) {
        Ok(entries) => entries,
        Err(FsError::NotFound) => return Vec::new(),
        Err(e) => {
            issues.push(file_issue(EPIC, &manifests, env, fs_reason(&e)));
            return Vec::new();
        }
    };
    let mut items: Vec<PathBuf> = entries
        .into_iter()
        .filter(|e| e.meta.kind == EntryKind::File)
        .map(|e| e.path)
        .filter(|p| {
            p.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("item"))
        })
        .collect();
    items.sort_by_key(|p| p.to_string_lossy().to_lowercase());

    let mut games: Vec<InstalledGame> = Vec::new();
    for item in items {
        let parsed = fs
            .read_small(&item, MAX_JSON)
            .map_err(|e| fs_reason(&e))
            .and_then(|bytes| parse_item(&bytes).ok_or(INVALID));
        match parsed {
            Ok(None) => {}
            Ok(Some(game)) => {
                if !games.iter().any(|g| g.store_game_id == game.store_game_id) {
                    games.push(game);
                }
            }
            Err(reason) => issues.push(file_issue(EPIC, &item, env, reason)),
        }
    }
    games
}

/// The game of an `.item` file: `AppName`, `DisplayName` (else `AppName`),
/// `InstallLocation` (absolute) and `InstallSize`.
///
/// `Some(None)` for an add-on (`MainGameAppName` other than `AppName`): it
/// lives in the folder of its game. `None` if the file is not a JSON object
/// or lacks a valid `AppName` or `InstallLocation`.
pub(crate) fn parse_item(bytes: &[u8]) -> Option<Option<InstalledGame>> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let value: Value = serde_json::from_slice(bytes).ok()?;
    let obj = value.as_object()?;
    let text = |key: &str| obj.get(key).and_then(Value::as_str).and_then(non_empty);
    let app_name = text("AppName")?;
    let install_dir = text("InstallLocation").and_then(install_path)?;
    if text("MainGameAppName").is_some_and(|main| main != app_name) {
        return Some(None);
    }
    Some(Some(InstalledGame {
        store_game_id: app_name.to_owned(),
        name: text("DisplayName").unwrap_or(app_name).to_owned(),
        install_dir,
        size_bytes: obj.get("InstallSize").and_then(Value::as_u64),
        manifest_key: None,
    }))
}
