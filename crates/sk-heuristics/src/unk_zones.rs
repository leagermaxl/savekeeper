//! H-UNK candidates: the folders of the search zones (SPEC-07 §4.2.1), the
//! children of vendor containers (§4.2.2) and the Store packages (§4.2.3).

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use sk_core::env::{Environment, KnownFolder};
use sk_core::fs::{EntryKind, EntryMeta, Exclusion, FsError, FsScanner, PathFilter, ReparseKind};
use sk_core::model::{IssueSeverity, ScanIssue};
use sk_core::path::{starts_with_ci, PathSet};
use sk_core::privacy::redact_path;
use sk_scan::ExcludeSet;

use crate::collector::COLLECTOR_ID;
use crate::data::VendorList;
use crate::finding::file_name;

/// `FILE_ATTRIBUTE_DIRECTORY`: tells a cloud placeholder folder from a file.
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
/// Issue: a folder of a zone or a candidate could not be read.
pub(crate) const ISSUE_ACCESS_DENIED: &str = "heuristic.access_denied";
/// Issue: a folder could not be read for another reason.
pub(crate) const ISSUE_READ_FAILED: &str = "heuristic.read_failed";
/// The Store packages folder of `{LOCALAPPDATA}` (§4.2.3).
const PACKAGES: &str = "Packages";
/// Prefixes of Windows' own Store packages, skipped (§4.2.3).
const SYSTEM_PACKAGE_PREFIXES: [&str; 2] = ["microsoft.windows.", "microsoftwindows."];

/// How a zone lists its candidates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Zone {
    /// `{APPDATA}`: children, vendor containers one level deeper.
    Vendored,
    /// `{LOCALAPPDATA}`: as `Vendored`, plus the Store packages.
    LocalAppData,
    /// `{LOCALLOW}`: `Company\Product`.
    Nested,
    /// `{PROGRAMDATA}`: children, kept only when recently used (§4.2.1).
    ProgramData,
    /// `{DOCUMENTS}`, `{SAVED_GAMES}`: children.
    Plain,
    /// `{HOME}`: children that are not known folders, and dot files.
    Home,
}

/// Search zones in the order of SPEC-07 §4.2.1.
const ZONES: [(KnownFolder, Zone); 7] = [
    (KnownFolder::AppData, Zone::Vendored),
    (KnownFolder::LocalAppData, Zone::LocalAppData),
    (KnownFolder::LocalLow, Zone::Nested),
    (KnownFolder::ProgramData, Zone::ProgramData),
    (KnownFolder::Documents, Zone::Plain),
    (KnownFolder::SavedGames, Zone::Plain),
    (KnownFolder::Home, Zone::Home),
];

/// What kind of candidate it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// A folder classified by its markers.
    Folder,
    /// A folder of `{PROGRAMDATA}`: a finding only if recently used, without
    /// programs and not an installation folder.
    ProgramData,
    /// A Store package `{LOCALAPPDATA}\Packages\<PFN>`.
    Package,
    /// A dot file in the profile (`.gitconfig`) of this size.
    DotFile(u64),
}

/// A candidate of H-UNK.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Candidate {
    /// Absolute path.
    pub path: PathBuf,
    /// Kind.
    pub kind: Kind,
}

/// The candidates of all zones, in zone order and by name within a zone,
/// and the issues of folders that could not be listed. Zones covered by
/// `claimed` are skipped whole; covered candidates are left to the caller.
pub(crate) fn candidates(
    fs: &dyn FsScanner,
    env: &Environment,
    excludes: &ExcludeSet,
    claimed: &PathSet,
    vendors: Option<&VendorList>,
) -> (Vec<Candidate>, Vec<ScanIssue>) {
    let mut lister = Lister {
        fs,
        env,
        excludes,
        issues: Vec::new(),
    };
    let mut out = Vec::new();
    for (folder, zone) in ZONES {
        let Some(root) = env.known_folder(folder) else {
            continue;
        };
        if claimed.covers(root) {
            continue;
        }
        for entry in lister.list(root) {
            let name = file_name(&entry.path);
            let cand = |path: PathBuf, kind| Candidate { path, kind };
            match zone {
                Zone::Home if !entry.is_dir => {
                    if name.starts_with('.') {
                        out.push(cand(entry.path, Kind::DotFile(entry.meta.size)));
                    }
                }
                _ if !entry.is_dir => {}
                Zone::LocalAppData if name.eq_ignore_ascii_case(PACKAGES) => {
                    for package in lister.dirs(&entry.path) {
                        if !is_system_package(&file_name(&package)) {
                            out.push(cand(package, Kind::Package));
                        }
                    }
                }
                Zone::Vendored | Zone::LocalAppData
                    if vendors.is_some_and(|v| v.contains(&name)) =>
                {
                    let children = lister.dirs(&entry.path);
                    out.extend(children.into_iter().map(|c| cand(c, Kind::Folder)));
                }
                Zone::Nested => {
                    let products = lister.dirs(&entry.path);
                    out.extend(products.into_iter().map(|p| cand(p, Kind::Folder)));
                }
                Zone::ProgramData => out.push(cand(entry.path, Kind::ProgramData)),
                _ => out.push(cand(entry.path, Kind::Folder)),
            }
        }
    }
    (out, lister.issues)
}

/// `Microsoft.Windows.*` and `MicrosoftWindows.*` packages (§4.2.3).
fn is_system_package(name: &str) -> bool {
    let lower = name.to_lowercase();
    SYSTEM_PACKAGE_PREFIXES.iter().any(|p| lower.starts_with(p))
}

/// Whether the entry is a folder that is walked: a folder or a cloud
/// placeholder folder. Links and junctions are not candidates (§5).
pub(crate) fn is_dir(meta: &EntryMeta) -> bool {
    match meta.kind {
        EntryKind::Dir => true,
        EntryKind::Reparse(ReparseKind::CloudPlaceholder) => {
            meta.attrs & FILE_ATTRIBUTE_DIRECTORY != 0
        }
        EntryKind::File | EntryKind::Reparse(_) => false,
    }
}

/// An entry of a listed folder.
struct Entry {
    path: PathBuf,
    meta: EntryMeta,
    is_dir: bool,
}

/// Lists folders, skipping what is never a candidate and collecting issues.
struct Lister<'a> {
    fs: &'a dyn FsScanner,
    env: &'a Environment,
    excludes: &'a ExcludeSet,
    issues: Vec<ScanIssue>,
}

impl Lister<'_> {
    /// Entries of `dir` sorted by name: folders that are not excluded
    /// globally (SPEC-03 §4.5) and do not hold a known folder,
    /// and regular files. Reparse points other than cloud folders are dropped.
    fn list(&mut self, dir: &Path) -> Vec<Entry> {
        let listing = match self.fs.read_dir(dir) {
            Ok(listing) => listing,
            Err(FsError::NotFound | FsError::Cancelled) => return Vec::new(),
            Err(err) => {
                self.issues.push(read_issue(dir, &err, self.env));
                return Vec::new();
            }
        };
        let mut entries: Vec<Entry> = listing
            .into_iter()
            .filter_map(|e| {
                let is_dir = is_dir(&e.meta);
                let keep = if is_dir {
                    !self.is_excluded(&e.path) && !self.holds_known_folder(&e.path)
                } else {
                    e.meta.kind == EntryKind::File
                };
                keep.then_some(Entry {
                    path: e.path,
                    meta: e.meta,
                    is_dir,
                })
            })
            .collect();
        entries.sort_by(|a, b| by_name(&a.path, &b.path));
        entries
    }

    /// The candidate folders of [`list`](Self::list) only.
    fn dirs(&mut self, dir: &Path) -> Vec<PathBuf> {
        self.list(dir)
            .into_iter()
            .filter(|e| e.is_dir)
            .map(|e| e.path)
            .collect()
    }

    fn is_excluded(&self, path: &Path) -> bool {
        let name = path.file_name().unwrap_or(OsStr::new(""));
        self.excludes.check(path, name, true) == Exclusion::Exclude
    }

    /// The folder is or contains a known folder: it is a zone of its own
    /// (`{HOME}\AppData`, `{HOME}\Documents`). Cloud roots (`{HOME}\Dropbox`)
    /// stay candidates and get the `cloud-synced` tag from their marker (§5).
    fn holds_known_folder(&self, path: &Path) -> bool {
        self.env
            .known_folders
            .values()
            .any(|known| starts_with_ci(known, path))
    }
}

/// Order by lowercase name, then by the name as is.
fn by_name(a: &Path, b: &Path) -> Ordering {
    let (a, b) = (file_name(a), file_name(b));
    a.to_lowercase().cmp(&b.to_lowercase()).then(a.cmp(&b))
}

/// Key of the warning for a folder that could not be read.
pub(crate) fn read_reason(err: &FsError) -> &'static str {
    match err {
        FsError::AccessDenied => ISSUE_ACCESS_DENIED,
        _ => ISSUE_READ_FAILED,
    }
}

/// The warning for a folder that could not be read: `heuristic.access_denied`,
/// or `heuristic.read_failed {error}` for other errors.
pub(crate) fn read_issue(path: &Path, err: &FsError, env: &Environment) -> ScanIssue {
    let message_args = match err {
        FsError::AccessDenied => BTreeMap::new(),
        _ => BTreeMap::from([("error".to_owned(), err.to_string())]),
    };
    ScanIssue {
        severity: IssueSeverity::Warning,
        source: COLLECTOR_ID.to_owned(),
        path: Some(redact_path(path, env)),
        message_key: read_reason(err).to_owned(),
        message_args,
    }
}
