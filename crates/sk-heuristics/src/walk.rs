//! The shared walk of the roots of H-USR and H-GIT (SPEC-07 §4.3.1, §4.4.1):
//! which roots are walked, what is skipped, and the cache that makes the
//! walk happen once per scan (§4.1).

// Read by H-USR and H-GIT (T-07-08, T-07-09); until then only by the tests.
#![cfg_attr(not(test), allow(dead_code))]

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, OnceLock};

use sk_core::config::HeuristicsConfig;
use sk_core::env::{DriveKind, Environment, KnownFolder};
use sk_core::fs::{EntryKind, Exclusion, FsError, PathFilter, ReparseKind, WalkOptions};
use sk_core::model::{IssueSeverity, ScanIssue};
use sk_core::path::{eq_ci, starts_with_ci, PathSet};
use sk_core::privacy::redact_path;
use sk_core::template::PathTemplate;
use sk_scan::ExcludeSet;

use crate::collector::COLLECTOR_ID;
use crate::data;
use crate::heuristic::HeuristicContext;
use crate::junk::is_specific;
use crate::unk_zones::{is_dir, read_issue};
use crate::walk_tree::{RootsWalk, TreeBuilder};

#[cfg(test)]
#[path = "walk_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "walk_run_tests.rs"]
mod run_tests;

#[cfg(test)]
#[path = "walk_fixture_tests.rs"]
mod fixture_tests;

/// Depth limit of the walk when none is given: `scan.max_depth` by default.
const DEFAULT_MAX_DEPTH: u32 = 32;
/// Progress is reported once per this many entries.
const PROGRESS_EVERY: u64 = 1024;
/// Folder of the Steam libraries above the games (`…\steamapps\common\<game>`).
const STEAMAPPS: &str = "steamapps";
/// Issue: folders below a root could not be read (as in SPEC-03 §4.3).
const ISSUE_DIRS_UNREADABLE: &str = "issue.scan.dirs_unreadable";
/// Issue: a root is a link or junction and is not followed (as in SPEC-03 §4.3).
const ISSUE_REPARSE_ROOT: &str = "issue.scan.reparse_root";

/// The walk of the roots of SPEC-07 §4.3.1, shared by H-GIT and H-USR: it
/// is done by the first of them that asks for it and kept for the scan.
pub struct RootsWalkCache {
    /// Roots given by the user: the collector passes `scan.extra_roots`.
    extra_roots: Vec<PathBuf>,
    /// Depth limit of the walk (`scan.max_depth`).
    max_depth: u32,
    walked: OnceLock<RootsWalk>,
}

impl RootsWalkCache {
    /// A cache that walks the fixed drives (§4.3.1) and `extra_roots`, down
    /// to `max_depth` levels.
    pub fn new(extra_roots: Vec<PathBuf>, max_depth: u32) -> Self {
        Self {
            extra_roots,
            max_depth,
            walked: OnceLock::new(),
        }
    }

    /// The walk, done on the first call. The issues of the walk are returned
    /// by the first call only, so that they are reported once.
    pub(crate) fn get(&self, hctx: &HeuristicContext<'_>) -> (&RootsWalk, Vec<ScanIssue>) {
        let mut issues = Vec::new();
        let walk = self.walked.get_or_init(|| {
            let roots = roots(hctx.env, hctx.cfg, &self.extra_roots);
            let (walk, found) = walk_roots(hctx, &roots, self.max_depth);
            issues = found;
            walk
        });
        (walk, issues)
    }
}

impl Default for RootsWalkCache {
    /// No extra roots, the default depth limit.
    fn default() -> Self {
        Self::new(Vec::new(), DEFAULT_MAX_DEPTH)
    }
}

impl fmt::Debug for RootsWalkCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RootsWalkCache")
            .field("extra_roots", &self.extra_roots)
            .field("max_depth", &self.max_depth)
            .field("walked", &self.walked.get().is_some())
            .finish()
    }
}

/// Roots of the walk (§4.3.1): the user's `extra` roots and the roots of the
/// fixed drives — all of them, or with `usr.scan_all_fixed_drives = false`
/// only the system drive (the one of `{WINDIR}`). Network, removable and
/// other drives are not walked. Sorted by path, without duplicates; roots
/// inside other roots are sorted out by [`walk_roots`].
pub(crate) fn roots(env: &Environment, cfg: &HeuristicsConfig, extra: &[PathBuf]) -> Vec<PathBuf> {
    let system = env.known_folder(KnownFolder::WinDir).and_then(drive_letter);
    let drives = env
        .drives
        .iter()
        .filter(|d| d.kind == DriveKind::Fixed)
        .filter(|d| cfg.usr.scan_all_fixed_drives || system == Some(d.letter.to_ascii_uppercase()))
        .map(|d| PathBuf::from(format!("{}:\\", d.letter.to_ascii_uppercase())));
    let mut all: Vec<PathBuf> = drives.chain(extra.iter().cloned()).collect();
    all.sort_by_key(|p| p.to_string_lossy().to_lowercase());
    all.dedup_by(|a, b| eq_ci(a, b));
    all
}

/// The roots walked on their own. A root inside another root is dropped when
/// the walk of the nearest such root enters it; when that walk would not (a
/// skipped folder of §4.3.1, a global exclusion, `.git`, a link or the depth
/// limit lies on the way), the root is kept and walked as given, as an
/// explicit root, and the outer walk skips it. So no folder is counted twice.
fn own_roots(
    hctx: &HeuristicContext<'_>,
    roots: &[PathBuf],
    excluded: &[PathBuf],
    max_depth: u32,
) -> Vec<PathBuf> {
    let mut kept: Vec<PathBuf> = Vec::new();
    for root in roots {
        // `roots` is sorted, so the outer roots come first, the nearest last.
        let outer = kept.iter().rev().find(|o| starts_with_ci(root, o));
        if !outer.is_some_and(|o| is_entered(hctx, o, root, excluded, max_depth)) {
            kept.push(root.clone());
        }
    }
    kept
}

/// Whether the walk of `outer` enters `root`, which lies below it.
fn is_entered(
    hctx: &HeuristicContext<'_>,
    outer: &Path,
    root: &Path,
    excluded: &[PathBuf],
    max_depth: u32,
) -> bool {
    let rel: Vec<Component<'_>> = root.components().skip(outer.components().count()).collect();
    // The walk lists the folders above `max_depth` only.
    if u32::try_from(rel.len()).map_or(true, |depth| depth >= max_depth) {
        return false;
    }
    let filter = root_filter(hctx.excludes, outer);
    let mut path = outer.to_path_buf();
    for component in rel {
        path.push(component);
        let name = component.as_os_str();
        let entered = !name.eq_ignore_ascii_case(".git")
            && !excluded.iter().any(|e| eq_ci(e, &path))
            && filter.check(&path, name, true) == Exclusion::Keep
            && hctx.scanner.metadata(&path).is_ok_and(|m| is_dir(&m));
        if !entered {
            return false;
        }
    }
    true
}

/// Uppercase drive letter of `X:\…` or `\\?\X:\…`, read from the text so that
/// it works on every platform.
fn drive_letter(path: &Path) -> Option<char> {
    let text = path.to_string_lossy();
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(letter), Some(':')) if letter.is_ascii_alphabetic() => {
            Some(letter.to_ascii_uppercase())
        }
        _ => None,
    }
}

/// Folders skipped by the walk besides the global exclusions (§4.3.1):
/// `{WINDIR}`, `{PROGRAMFILES}`, `{PROGRAMFILES_X86}`, `{PROGRAMDATA}`,
/// `{HOME}\AppData` and the AppData folders (H-UNK looks there), the
/// launcher roots, the installation folders of games and their Steam
/// libraries (`…\steamapps`), and the `InstallLocation` of installed
/// programs. Folders from the launchers and programs that are not specific
/// (a drive root, a known folder or a folder holding one) are ignored.
///
/// The drive-level folders of §4.3.1 (`$Recycle.Bin`, `System Volume
/// Information`, `Recovery`, `PerfLogs`, `Windows.old`, `MSOCache`,
/// `pagefile.sys` …) are global exclusions (SPEC-03 §4.5).
pub(crate) fn excluded_dirs(env: &Environment) -> Vec<PathBuf> {
    const KNOWN: [KnownFolder; 7] = [
        KnownFolder::WinDir,
        KnownFolder::ProgramFiles,
        KnownFolder::ProgramFilesX86,
        KnownFolder::ProgramData,
        KnownFolder::AppData,
        KnownFolder::LocalAppData,
        KnownFolder::LocalLow,
    ];
    let mut out: Vec<PathBuf> = KNOWN
        .iter()
        .filter_map(|&f| env.known_folder(f).map(Path::to_path_buf))
        .collect();
    if let Some(home) = env.known_folder(KnownFolder::Home) {
        out.push(home.join("AppData"));
    }
    let mut specific = Vec::new();
    for launcher in &env.launchers {
        specific.extend(launcher.root.clone());
        for game in &launcher.games {
            specific.push(game.install_dir.clone());
            specific.extend(steam_library(&game.install_dir));
        }
    }
    let programs = env.installed_programs.iter();
    specific.extend(programs.filter_map(|p| p.install_location.clone()));
    out.extend(specific.into_iter().filter(|p| is_specific(p, env)));
    out
}

/// The `steamapps` folder above a game installed in a Steam library.
fn steam_library(install_dir: &Path) -> Option<PathBuf> {
    install_dir
        .ancestors()
        .skip(1)
        .find(|a| {
            matches!(a.components().next_back(), Some(Component::Normal(name))
                if name.eq_ignore_ascii_case(STEAMAPPS))
        })
        .map(Path::to_path_buf)
}

/// Walks `roots` one after another and aggregates their folders. Stops at
/// cancellation and returns what was walked, with `cancelled` set.
pub(crate) fn walk_roots(
    hctx: &HeuristicContext<'_>,
    roots: &[PathBuf],
    max_depth: u32,
) -> (RootsWalk, Vec<ScanIssue>) {
    let table = data::builtin().ok().map(|d| &d.extensions);
    let excluded = excluded_dirs(hctx.env);
    let roots = own_roots(hctx, roots, &excluded, max_depth);
    let mut out = RootsWalk::default();
    let mut issues = Vec::new();
    let mut entries = 0u64;
    for root in &roots {
        if hctx.cancel.is_cancelled() {
            out.cancelled = true;
            break;
        }
        match hctx.scanner.metadata(root) {
            Ok(meta) if is_dir(&meta) => {}
            Ok(meta)
                if matches!(
                    meta.kind,
                    EntryKind::Reparse(
                        ReparseKind::Symlink | ReparseKind::Junction | ReparseKind::Other(_)
                    )
                ) =>
            {
                // Links are not followed (FR-03-02).
                issues.push(root_issue(
                    hctx.env,
                    root,
                    IssueSeverity::Info,
                    ISSUE_REPARSE_ROOT,
                    None,
                ));
                continue;
            }
            Ok(_) => {
                let err = FsError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotADirectory,
                    "not a folder",
                ));
                issues.push(read_issue(root, &err, hctx.env));
                continue;
            }
            Err(FsError::NotFound) => continue,
            Err(FsError::Cancelled) => {
                out.cancelled = true;
                break;
            }
            Err(err) => {
                issues.push(read_issue(root, &err, hctx.env));
                continue;
            }
        }
        // An excluded folder that the user gave as a root is walked; the
        // exclusions below it still apply. Roots below it are walked on their own.
        let skip: PathSet = excluded
            .iter()
            .chain(&roots)
            .filter(|e| starts_with_ci(e, root) && !eq_ci(e, root))
            .cloned()
            .collect();
        let opts = WalkOptions {
            max_depth,
            max_entries: u64::MAX,
            follow_links: false,
            excludes: root_filter(hctx.excludes, root),
            include: None,
            exclude: None,
            threads: 0,
        };
        let current = PathTemplate::from_path(root, hctx.env);
        let mut builder = TreeBuilder::new(root, &skip, table);
        let walked = hctx.scanner.walk(
            root,
            &opts,
            &mut |e| {
                entries += 1;
                if entries.is_multiple_of(PROGRESS_EVERY) {
                    (hctx.progress)(entries, Some(current.as_str()));
                }
                builder.visit(e)
            },
            hctx.cancel,
        );
        match walked {
            Ok(stats) => {
                if stats.errors > 0 {
                    let count = stats.errors.to_string();
                    issues.push(root_issue(
                        hctx.env,
                        root,
                        IssueSeverity::Warning,
                        ISSUE_DIRS_UNREADABLE,
                        Some(("count", count)),
                    ));
                }
                builder.finish(stats.errors, &mut out);
            }
            Err(FsError::Cancelled) => {
                builder.finish(0, &mut out);
                out.cancelled = true;
                break;
            }
            Err(FsError::NotFound) => {}
            Err(err) => issues.push(read_issue(root, &err, hctx.env)),
        }
    }
    out.valuable_files
        .sort_by_cached_key(|f| f.path.to_string_lossy().to_lowercase());
    (out, issues)
}

/// The global exclusions for a root (SPEC-03 §4.5 `explicit_root`): a root
/// inside a path exclusion is walked with the name exclusions only.
fn root_filter(excludes: &Arc<ExcludeSet>, root: &Path) -> Arc<dyn PathFilter> {
    let name = root.file_name().unwrap_or(OsStr::new(""));
    let names_only = excludes.names_only();
    if excludes.check(root, name, true) == Exclusion::Exclude
        && names_only.check(root, name, true) == Exclusion::Keep
    {
        Arc::new(names_only)
    } else {
        Arc::clone(excludes) as Arc<dyn PathFilter>
    }
}

/// An issue of the `heuristics` source about a root.
fn root_issue(
    env: &Environment,
    root: &Path,
    severity: IssueSeverity,
    key: &str,
    arg: Option<(&str, String)>,
) -> ScanIssue {
    ScanIssue {
        severity,
        source: COLLECTOR_ID.to_owned(),
        path: Some(redact_path(root, env)),
        message_key: key.to_owned(),
        message_args: arg
            .map(|(k, v)| BTreeMap::from([(k.to_owned(), v)]))
            .unwrap_or_default(),
    }
}
