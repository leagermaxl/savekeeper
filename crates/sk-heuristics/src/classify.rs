//! Classification of a folder by the markers of its summary (SPEC-07 §4.7).
//!
//! An ordered list of rules over the `FolderSummary`; the first rule that
//! matches wins. The thresholds (share of save files, size of a config folder,
//! age of "recent" SQLite files) come from `HeuristicsConfig::thresholds`.

use std::path::{Path, PathBuf};

use sk_core::config::HeuristicThresholds;
use sk_core::env::{Environment, KnownFolder};
use sk_core::model::{Category, FolderSummary, Marker};
use sk_core::path::{eq_ci, starts_with_ci};
use sk_core::template::ResolveContext;
use time::{Duration, OffsetDateTime};

use crate::junk::junk_verdict;
use crate::verdict::MarkerVerdict;

#[cfg(test)]
#[path = "classify_tests.rs"]
mod tests;

/// Reason: the folder lies under `{SAVED_GAMES}`.
pub(crate) const REASON_SAVED_GAMES: &str = "heuristic.saved_games";
/// Reason: the `UnrealSaveGames` marker.
pub(crate) const REASON_UNREAL_SAVES: &str = "heuristic.unreal_saves";
/// Reason: a Unity game folder with save-like files.
pub(crate) const REASON_UNITY_GAME: &str = "heuristic.unity_game";
/// Reason: many files with save extensions.
pub(crate) const REASON_SAVE_EXT: &str = "heuristic.save_ext";
/// Reason: a small config-like dot folder in the profile.
pub(crate) const REASON_DOTFOLDER: &str = "heuristic.dotfolder";
/// Reason: a small config-like folder without programs.
pub(crate) const REASON_CONFIG_LIKE: &str = "heuristic.config_like";
/// Reason: SQLite databases changed recently.
pub(crate) const REASON_SQLITE_RECENT: &str = "heuristic.sqlite_recent";
/// Reason: documents or media outside `AppData`.
pub(crate) const REASON_USER_CONTENT: &str = "heuristic.user_content";
/// Reason: a development project.
pub(crate) const REASON_PROJECT_LIKE: &str = "heuristic.project_like";
/// Reason: the folder has no files or only empty ones.
pub(crate) const REASON_EMPTY: &str = "heuristic.empty";
/// Reason: nothing matched.
pub(crate) const REASON_UNKNOWN: &str = "heuristic.unknown";

/// Extensions of Unity save files (rule 4); `""` is a file without extension.
const UNITY_SAVE_EXTS: &[&str] = &["", "dat", "sav", "json"];
/// Extensions of game saves (rule 5).
const SAVE_EXTS: &[&str] = &["sav", "save", "sl2", "ess"];
/// Largest config-like dot folder in the profile (rule 6): 10 MiB.
const DOTFOLDER_MAX_BYTES: u64 = 10 * 1024 * 1024;

/// What the folder probably is, judged by its summary (SPEC-07 §4.7), with the
/// default thresholds and the current time.
///
/// H-UNK uses [`classify_by_markers_with`] to apply the configured thresholds.
pub fn classify_by_markers(s: &FolderSummary, env: &Environment) -> MarkerVerdict {
    classify_by_markers_with(
        s,
        env,
        &HeuristicThresholds::default(),
        OffsetDateTime::now_utc(),
    )
}

/// [`classify_by_markers`] with the thresholds of `HeuristicsConfig::thresholds`
/// and `now` as the time against which "recent" (rule 8) is measured.
///
/// The rules of §4.7 are tried in order and the first match wins; a folder that
/// matches none is `Unknown` with confidence 0.
pub fn classify_by_markers_with(
    s: &FolderSummary,
    env: &Environment,
    thresholds: &HeuristicThresholds,
    now: OffsetDateTime,
) -> MarkerVerdict {
    if let Some(verdict) = junk_verdict(s, env) {
        return verdict;
    }
    let resolved = s.path.resolve(env, &ResolveContext::default());
    let has = |m: Marker| s.markers.contains(&m);
    let whole = MarkerVerdict::whole;

    if is_under(s, &resolved, env, &[KnownFolder::SavedGames]) {
        return whole(Category::GameSave, 0.7, REASON_SAVED_GAMES);
    }
    if has(Marker::UnrealSaveGames) {
        return whole(Category::GameSave, 0.8, REASON_UNREAL_SAVES);
    }
    if has(Marker::UnityGame) && has_any_ext(s, UNITY_SAVE_EXTS) {
        return whole(Category::GameSave, 0.65, REASON_UNITY_GAME);
    }
    if share_of_files(s, SAVE_EXTS) > f64::from(thresholds.save_ext_ratio) {
        return whole(Category::GameSave, 0.75, REASON_SAVE_EXT);
    }
    if has(Marker::ConfigLike)
        && s.total_bytes < DOTFOLDER_MAX_BYTES
        && is_home_dotfolder(s, &resolved, env)
    {
        return whole(Category::DevEnvironment, 0.65, REASON_DOTFOLDER);
    }
    if has(Marker::ConfigLike)
        && s.total_bytes < thresholds.config_max_bytes
        && !has(Marker::HasExecutables)
    {
        return whole(Category::AppConfig, 0.6, REASON_CONFIG_LIKE);
    }
    if has(Marker::SqliteFiles) && is_recent(s, thresholds.sqlite_recent_days, now) {
        return whole(Category::AppData, 0.55, REASON_SQLITE_RECENT);
    }
    if (has(Marker::DocumentHeavy) || has(Marker::MediaHeavy)) && !is_in_appdata(s, &resolved, env)
    {
        return whole(Category::UserFiles, 0.7, REASON_USER_CONTENT);
    }
    if has(Marker::ProjectLike) {
        return whole(Category::DevEnvironment, 0.7, REASON_PROJECT_LIKE);
    }
    // A truncated summary has not seen every file: it is not proof of emptiness.
    if (s.total_bytes == 0 || s.file_count == 0) && !s.truncated {
        return whole(Category::Cache, 0.95, REASON_EMPTY);
    }
    whole(Category::Unknown, 0.0, REASON_UNKNOWN)
}

/// Whether the folder is one of `folders` or lies under it, by its resolved
/// paths or, when they are unknown, by the leading token of its template.
fn is_under(
    s: &FolderSummary,
    resolved: &[PathBuf],
    env: &Environment,
    folders: &[KnownFolder],
) -> bool {
    folders.iter().any(|folder| {
        let by_path = env
            .known_folders
            .get(folder)
            .is_some_and(|base| resolved.iter().any(|p| starts_with_ci(p, base)));
        by_path || starts_with_token(s.path.as_str(), *folder)
    })
}

/// Whether the template starts with the token of `folder` (`{SAVED_GAMES}\…`).
/// Stored templates are canonical, so tokens are upper case.
fn starts_with_token(template: &str, folder: KnownFolder) -> bool {
    template
        .strip_prefix(&format!("{{{}}}", folder.token()))
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('\\'))
}

/// Rule 6: a direct child of `{HOME}` whose name starts with a dot.
fn is_home_dotfolder(s: &FolderSummary, resolved: &[PathBuf], env: &Environment) -> bool {
    let by_path = env
        .known_folders
        .get(&KnownFolder::Home)
        .is_some_and(|home| {
            resolved
                .iter()
                .any(|p| p.parent().is_some_and(|parent| eq_ci(parent, home)) && is_dot_name(p))
        });
    let by_template = s
        .path
        .as_str()
        .split_once('\\')
        .is_some_and(|(head, rest)| {
            head.eq_ignore_ascii_case("{HOME}") && rest.starts_with('.') && !rest.contains('\\')
        });
    by_path || by_template
}

fn is_dot_name(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with('.'))
}

/// Rule 9: the folder is inside `{HOME}\AppData` (Roaming, Local or LocalLow).
fn is_in_appdata(s: &FolderSummary, resolved: &[PathBuf], env: &Environment) -> bool {
    let appdata_root = env
        .known_folders
        .get(&KnownFolder::Home)
        .map(|home| home.join("AppData"));
    is_under(
        s,
        resolved,
        env,
        &[
            KnownFolder::AppData,
            KnownFolder::LocalAppData,
            KnownFolder::LocalLow,
        ],
    ) || appdata_root.is_some_and(|root| resolved.iter().any(|p| starts_with_ci(p, &root)))
}

/// Whether the extension histogram has files with one of `exts`.
fn has_any_ext(s: &FolderSummary, exts: &[&str]) -> bool {
    s.ext_histogram
        .iter()
        .any(|e| e.count > 0 && exts.contains(&e.ext.as_str()))
}

/// Share of the folder's files with one of `exts`, 0 for an empty folder.
fn share_of_files(s: &FolderSummary, exts: &[&str]) -> f64 {
    if s.file_count == 0 {
        return 0.0;
    }
    let count: u64 = s
        .ext_histogram
        .iter()
        .filter(|e| exts.contains(&e.ext.as_str()))
        .map(|e| e.count)
        .sum();
    // File counts stay far below 2^52, so the conversion is exact.
    count as f64 / s.file_count as f64
}

/// Rule 8: the newest file is younger than `days`. A time in the future
/// (clock skew) counts as recent.
fn is_recent(s: &FolderSummary, days: u32, now: OffsetDateTime) -> bool {
    s.newest_mtime
        .is_some_and(|newest| now - newest < Duration::days(i64::from(days)))
}
