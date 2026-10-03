//! Steam detector (SPEC-05 §4.4): root from the registry, libraries from
//! `libraryfolders.vdf`, games from `appmanifest_<appid>.acf`, accounts from
//! `userdata\<id3>` with names from `config\loginusers.vdf`.

use std::fmt;
use std::path::{Path, PathBuf, MAIN_SEPARATOR_STR};
use std::sync::Arc;

use keyvalues_parser::Value;
use sk_core::env::{Environment, InstalledGame, KnownFolder, LauncherInfo, StoreUser};
use sk_core::fs::{EntryKind, EntryMeta, FsError, FsScanner, ReparseKind};
use sk_core::model::{RegHive, ScanIssue};
use sk_core::path::eq_ci;
use sk_core::registry::{RegistryReader, SystemRegistry};
use sk_core::template::PathTemplate;

use super::vdf::{self, Reason, INVALID};
use super::{info_issue, LauncherDetector};

/// Steam id64 of the account with id3 0: id3 = id64 − base.
pub const STEAM_ID64_BASE: u64 = 76_561_197_960_265_728;

const STEAM: &str = "steam";
const REGISTRY_KEY: &str = r"Software\Valve\Steam";
const REGISTRY_VALUE: &str = "SteamPath";
/// Size limits of the files read; real ones are a few KiB.
const MAX_VDF: usize = 4 << 20;
const MAX_ACF: usize = 1 << 20;
/// `FILE_ATTRIBUTE_DIRECTORY`.
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;

/// `libraryfolders.vdf` is missing or broken: only the main library is used.
pub(crate) const ISSUE_LIBRARYFOLDERS: &str = "issue.games.steam_libraryfolders_unreadable";
/// A library cannot be listed (disconnected drive and the like): skipped.
pub(crate) const ISSUE_LIBRARY: &str = "issue.games.steam_library_unavailable";
/// An `appmanifest_*.acf` cannot be read or is not valid: the game is skipped.
pub(crate) const ISSUE_APPMANIFEST: &str = "issue.games.steam_appmanifest_unreadable";

/// Detects Steam (SPEC-05 §4.4). The registry is read through a
/// [`RegistryReader`], so tests can use a fake one.
#[derive(Clone)]
pub struct SteamDetector {
    registry: Arc<dyn RegistryReader>,
}

impl SteamDetector {
    /// A detector reading the registry of this machine.
    pub fn new() -> Self {
        Self::with_registry(Arc::new(SystemRegistry))
    }

    /// A detector reading `registry`.
    pub fn with_registry(registry: Arc<dyn RegistryReader>) -> Self {
        Self { registry }
    }

    /// The Steam folder: `SteamPath` from `HKCU\Software\Valve\Steam`, else
    /// `{PROGRAMFILES_X86}\Steam`; the first that is a folder.
    fn root(&self, fs: &dyn FsScanner, env: &Environment) -> Option<PathBuf> {
        let registered = self
            .registry
            .string_value(RegHive::Hkcu, REGISTRY_KEY, REGISTRY_VALUE)
            .map(|s| native_path(&s))
            .filter(|p| !p.as_os_str().is_empty());
        let default = env
            .known_folder(KnownFolder::ProgramFilesX86)
            .map(|p| p.join("Steam"));
        registered
            .into_iter()
            .chain(default)
            .find(|p| fs.metadata(p).is_ok_and(|m| is_folder(&m)))
    }
}

impl Default for SteamDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for SteamDetector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SteamDetector").finish_non_exhaustive()
    }
}

impl LauncherDetector for SteamDetector {
    fn id(&self) -> &'static str {
        STEAM
    }

    fn detect(&self, fs: &dyn FsScanner, env: &Environment) -> Option<LauncherInfo> {
        self.detect_with_issues(fs, env).0
    }

    fn detect_with_issues(
        &self,
        fs: &dyn FsScanner,
        env: &Environment,
    ) -> (Option<LauncherInfo>, Vec<ScanIssue>) {
        let Some(root) = self.root(fs, env) else {
            return (None, Vec::new());
        };
        let mut issues = Vec::new();
        let mut games: Vec<InstalledGame> = Vec::new();
        for (index, library) in libraries(fs, env, &root, &mut issues).iter().enumerate() {
            for game in library_games(fs, env, library, index == 0, &mut issues) {
                if !games.iter().any(|g| g.store_game_id == game.store_game_id) {
                    games.push(game);
                }
            }
        }
        let launcher = LauncherInfo {
            id: STEAM.to_owned(),
            user_ids: users(fs, &root),
            games,
            root: Some(root),
        };
        (Some(launcher), issues)
    }
}

/// The main library (`root`) and the others from `steamapps\libraryfolders.vdf`,
/// without repeats (compared without case).
fn libraries(
    fs: &dyn FsScanner,
    env: &Environment,
    root: &Path,
    issues: &mut Vec<ScanIssue>,
) -> Vec<PathBuf> {
    let mut out = vec![root.to_path_buf()];
    let file = root.join("steamapps").join("libraryfolders.vdf");
    let listed = vdf::read_text(fs, &file, MAX_VDF)
        .and_then(|text| parse_libraryfolders(&text).ok_or(INVALID));
    match listed {
        Ok(paths) => {
            for path in paths {
                let path = native_path(&path);
                if !path.as_os_str().is_empty() && !out.iter().any(|p| eq_ci(p, &path)) {
                    out.push(path);
                }
            }
        }
        Err(reason) => issues.push(issue(ISSUE_LIBRARYFOLDERS, &file, env, reason, None)),
    }
    out
}

/// Library paths of `libraryfolders.vdf`, by number: `"1" { "path" "D:\…" }`,
/// or `"1" "D:\…"` in the old format.
pub(crate) fn parse_libraryfolders(text: &str) -> Option<Vec<String>> {
    let obj = vdf::parse(text)?;
    let mut numbered: Vec<(u64, String)> = Vec::new();
    for (key, values) in obj.iter() {
        let Some(n) = number(key) else { continue };
        let Some(value) = values.first() else {
            continue;
        };
        let path = match value {
            Value::Str(path) => Some(path.as_ref()),
            Value::Obj(entry) => vdf::get_str(entry, "path"),
        };
        if let Some(path) = path {
            numbered.push((n, path.to_owned()));
        }
    }
    numbered.sort_by_key(|(n, _)| *n);
    Some(numbered.into_iter().map(|(_, path)| path).collect())
}

/// Games of the library `library` from its `steamapps\appmanifest_*.acf`,
/// by app id. A missing `steamapps` of the main library is not an issue
/// (no games yet); a library on a drive that is not present is not touched.
fn library_games(
    fs: &dyn FsScanner,
    env: &Environment,
    library: &Path,
    main: bool,
    issues: &mut Vec<ScanIssue>,
) -> Vec<InstalledGame> {
    let steamapps = library.join("steamapps");
    let drive = drive_letter(library);
    let drive_text = drive.map(String::from);
    if let Some(letter) = drive {
        if !main && !env.drives.iter().any(|d| d.letter == letter) {
            issues.push(issue(
                ISSUE_LIBRARY,
                library,
                env,
                "drive_missing",
                drive_text.as_deref(),
            ));
            return Vec::new();
        }
    }
    let entries = match fs.read_dir(&steamapps) {
        Ok(entries) => entries,
        Err(FsError::NotFound) if main => return Vec::new(),
        Err(e) => {
            let reason = vdf::fs_reason(&e);
            issues.push(issue(
                ISSUE_LIBRARY,
                library,
                env,
                reason,
                drive_text.as_deref(),
            ));
            return Vec::new();
        }
    };
    let mut manifests: Vec<(u64, PathBuf)> = entries
        .into_iter()
        .filter(|e| e.meta.kind == EntryKind::File)
        .filter_map(|e| {
            let name = e.path.file_name()?.to_str()?.to_ascii_lowercase();
            let id = number(name.strip_prefix("appmanifest_")?.strip_suffix(".acf")?)?;
            Some((id, e.path))
        })
        .collect();
    manifests.sort_by_key(|(id, _)| *id);

    let mut games = Vec::new();
    for (file_id, file) in manifests {
        let app = vdf::read_text(fs, &file, MAX_ACF)
            .and_then(|text| parse_appmanifest(&text, file_id).ok_or(INVALID));
        match app {
            Ok(app) => games.push(InstalledGame {
                install_dir: steamapps.join("common").join(&app.install_dir),
                store_game_id: app.app_id,
                name: app.name,
                size_bytes: app.size_on_disk,
                manifest_key: None,
            }),
            Err(reason) => issues.push(issue(ISSUE_APPMANIFEST, &file, env, reason, None)),
        }
    }
    games
}

/// Fields of an `appmanifest_<appid>.acf`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AppManifest {
    /// `appid`, else the id in the file name.
    pub(crate) app_id: String,
    /// `name`, else `installdir`.
    pub(crate) name: String,
    /// `installdir`: one folder name under `steamapps\common`.
    pub(crate) install_dir: String,
    /// `SizeOnDisk`.
    pub(crate) size_on_disk: Option<u64>,
}

/// Parses an app manifest; `None` without a valid `installdir` (a single
/// folder name: no separators, `:`, `.` or `..`).
pub(crate) fn parse_appmanifest(text: &str, file_id: u64) -> Option<AppManifest> {
    let obj = vdf::parse(text)?;
    let install_dir = vdf::get_str(&obj, "installdir")?.trim();
    let valid = !install_dir.is_empty()
        && install_dir != "."
        && install_dir != ".."
        && !install_dir.contains(['\\', '/', ':']);
    if !valid {
        return None;
    }
    let app_id = vdf::get_str(&obj, "appid")
        .and_then(number)
        .unwrap_or(file_id);
    let name = vdf::get_str(&obj, "name")
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or(install_dir);
    Some(AppManifest {
        app_id: app_id.to_string(),
        name: name.to_owned(),
        install_dir: install_dir.to_owned(),
        size_on_disk: vdf::get_str(&obj, "SizeOnDisk").and_then(number),
    })
}

/// Accounts with a `userdata\<id3>` folder, by id3, named from
/// `config\loginusers.vdf` (`PersonaName`).
fn users(fs: &dyn FsScanner, root: &Path) -> Vec<StoreUser> {
    let Ok(entries) = fs.read_dir(&root.join("userdata")) else {
        return Vec::new();
    };
    let mut ids: Vec<u32> = entries
        .iter()
        .filter(|e| e.meta.kind == EntryKind::Dir)
        .filter_map(|e| {
            let name = e.path.file_name()?.to_str()?;
            let id: u32 = name.parse().ok()?;
            // Only the canonical spelling: the name goes into templates as is.
            (id.to_string() == name).then_some(id)
        })
        .collect();
    ids.sort_unstable();
    ids.dedup();
    let names = vdf::read_text(fs, &root.join("config").join("loginusers.vdf"), MAX_VDF)
        .ok()
        .and_then(|text| parse_loginusers(&text))
        .unwrap_or_default();
    ids.into_iter()
        .map(|id| StoreUser {
            id: id.to_string(),
            alt_id: Some((u64::from(id) + STEAM_ID64_BASE).to_string()),
            name: names
                .iter()
                .find(|(id3, _)| *id3 == id)
                .map(|(_, name)| name.clone()),
        })
        .collect()
}

/// `(id3, PersonaName)` of the accounts in `loginusers.vdf`.
pub(crate) fn parse_loginusers(text: &str) -> Option<Vec<(u32, String)>> {
    let obj = vdf::parse(text)?;
    let mut out = Vec::new();
    for (key, values) in obj.iter() {
        let Some(id3) = number(key).and_then(id3_of) else {
            continue;
        };
        let Some(Value::Obj(user)) = values.first() else {
            continue;
        };
        if let Some(name) = vdf::get_str(user, "PersonaName").filter(|n| !n.trim().is_empty()) {
            out.push((id3, name.to_owned()));
        }
    }
    Some(out)
}

/// id3 of a Steam id64; `None` outside the individual account range.
pub(crate) fn id3_of(id64: u64) -> Option<u32> {
    id64.checked_sub(STEAM_ID64_BASE)
        .and_then(|id| u32::try_from(id).ok())
}

/// A decimal number of ASCII digits only.
fn number(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// A path from Steam files or the registry in native form: `/` becomes the
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

/// Upper-case drive letter of `C:\…` (or `\\?\C:\…`), on every OS.
fn drive_letter(path: &Path) -> Option<char> {
    let s = path.to_string_lossy();
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
    let mut chars = s.chars();
    match (chars.next(), chars.next()) {
        (Some(letter), Some(':')) if letter.is_ascii_alphabetic() => {
            Some(letter.to_ascii_uppercase())
        }
        _ => None,
    }
}

/// A folder, or a link to one (the Steam folder may be moved by a junction).
fn is_folder(meta: &EntryMeta) -> bool {
    match meta.kind {
        EntryKind::Dir => true,
        EntryKind::Reparse(ReparseKind::Junction | ReparseKind::Symlink) => {
            meta.attrs & FILE_ATTRIBUTE_DIRECTORY != 0
        }
        EntryKind::File | EntryKind::Reparse(_) => false,
    }
}

fn issue(
    key: &str,
    path: &Path,
    env: &Environment,
    reason: Reason,
    drive: Option<&str>,
) -> ScanIssue {
    let mut args = vec![("reason", reason)];
    if let Some(drive) = drive {
        args.push(("drive", drive));
    }
    let template = PathTemplate::from_path(path, env).to_string();
    info_issue(STEAM, key, Some(template), &args)
}

#[cfg(test)]
#[path = "steam_tests.rs"]
mod tests;
