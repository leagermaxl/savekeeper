//! H-WEB: Electron and Chromium profiles (SPEC-07 §4.6).
//!
//! The candidates of the H-UNK zones (§4.2.1-§4.2.2) are probed with a
//! shallow summary: a folder with the marker `ElectronApp` or
//! `ChromiumProfile` (SPEC-03 §4.4), or its `User Data` subfolder with one of
//! them, is a profile. Each profile gives one `AppData` finding that saves the
//! whole folder except the caches of the §4.6 table, so that data an
//! application keeps outside the table (`sql\`, `attachments\`) is never lost.
//! Browsers covered by the rules (SPEC-04) are claimed and not touched.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sk_core::fs::{EntryKind, Exclusion, FsError, PathFilter};
use sk_core::model::{Category, Finding, FolderSummary, Marker, Sensitivity, Target};
use sk_core::template::PathTemplate;
use sk_scan::{summarize, SummaryOptions};

use crate::data;
use crate::finding::{
    display_name, file_name, is_opaque_name, new_finding, tags_from, TITLE_SEPARATOR,
};
use crate::heuristic::{Heuristic, HeuristicContext, HeuristicOutput};
use crate::unk::{relative_glob, topmost};
use crate::unk_zones::{candidates, is_dir, read_issue, Kind};
use crate::verdict::MarkerVerdict;

#[cfg(test)]
#[path = "web_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "web_fixture_tests.rs"]
mod fixture_tests;

/// `Heuristic::id` of H-WEB.
pub(crate) const HEURISTIC_ID: &str = "h-web";
/// Reason: the data folder of an Electron application.
pub(crate) const REASON_ELECTRON: &str = "heuristic.web.electron";
/// Reason: a Chromium profile (or a `User Data` folder with profiles).
pub(crate) const REASON_CHROMIUM: &str = "heuristic.web.chromium";
/// i18n key of the title part after the application name (SPEC-11 §4.7).
pub(crate) const TITLE_APP_DATA: &str = "heuristic.title.app_data";
/// Note of a profile with cookies or passwords: they are encrypted by DPAPI
/// and cannot be decrypted on another Windows installation.
pub(crate) const NOTES_DPAPI_BOUND: &str = "heuristic.dpapi_bound";
/// Tag of a profile with cookies or passwords.
pub(crate) const TAG_BROWSER_CREDENTIALS: &str = "browser-credentials";

/// The cache rows of the §4.6 table as exclude globs. `**/` matches the
/// profile root and any depth: a Chromium `User Data` folder keeps them in
/// each profile (`Default/Cache`), an Electron app in its partitions.
pub(crate) const CACHE_EXCLUDE: [&str; 10] = [
    "**/Cache",
    "**/Code Cache",
    "**/GPUCache",
    "**/DawnCache",
    "**/GrShaderCache",
    "**/ShaderCache",
    "**/Service Worker/CacheStorage",
    "**/Crashpad",
    "**/blob_storage",
    "**/logs",
];
/// Lowercase names of the cache rows that stand directly in a folder.
const CACHE_NAMES: [&str; 9] = [
    "cache",
    "code cache",
    "gpucache",
    "dawncache",
    "grshadercache",
    "shadercache",
    "crashpad",
    "blob_storage",
    "logs",
];
/// Lowercase names of the credential files of a profile (§4.6).
const CREDENTIAL_FILES: [&str; 3] = ["cookies", "login data", "web data"];
/// Folder of `Network\Cookies`.
const NETWORK: &str = "network";
/// Extensions of a profile: reinstallable when synchronized.
const EXTENSIONS: &str = "extensions";
/// The Chromium sync storage next to `Extensions`: the browser account
/// reinstalls the extensions, so the folder is excluded.
const SYNC_DATA: &str = "sync data";
/// The Chromium folder with profiles under an application folder.
const USER_DATA: &str = "User Data";
/// Depth of the probing summary: the markers look at most at `Default\Preferences`.
const PROBE_DEPTH: u32 = 2;
/// Confidence of a profile (§4.6).
const CONFIDENCE: f32 = 0.8;
/// Confidence lost by an application named by a GUID or a hash (§5).
const OPAQUE_PENALTY: f32 = 0.1;

/// H-WEB: one finding per Electron or Chromium profile.
#[derive(Debug, Default)]
pub(crate) struct HWeb;

impl HWeb {
    /// H-WEB.
    pub(crate) fn new() -> Self {
        Self
    }
}

impl Heuristic for HWeb {
    fn id(&self) -> &'static str {
        HEURISTIC_ID
    }

    fn run(&self, hctx: &HeuristicContext<'_>) -> HeuristicOutput {
        let vendors = data::builtin().ok().map(|d| &d.vendors);
        let (cands, issues) =
            candidates(hctx.scanner, hctx.env, hctx.excludes, hctx.claimed, vendors);
        // H-UNK lists and reads the same folders and reports their problems;
        // H-WEB does it only when H-UNK is switched off.
        let report = !hctx.cfg.enabled.unk;
        let mut run = Run {
            hctx,
            report,
            opts: SummaryOptions {
                max_depth: PROBE_DEPTH,
                ..SummaryOptions::new(Arc::clone(hctx.excludes))
            },
            out: HeuristicOutput::default(),
        };
        if report {
            run.out.issues = issues;
        }
        for (done, cand) in (1u64..).zip(&cands) {
            if hctx.cancel.is_cancelled() {
                break;
            }
            let path = cand.path.as_path();
            if !matches!(cand.kind, Kind::Folder | Kind::ProgramData) || hctx.claimed.covers(path) {
                continue;
            }
            let current = PathTemplate::from_path(path, hctx.env);
            (hctx.progress)(done, Some(current.as_str()));
            match run.probe(path) {
                Ok(Some((root, summary, kind))) => run.profile(&root, path, &summary, kind),
                Ok(None) => {}
                Err(()) => break,
            }
        }
        run.out
    }
}

/// What kind of profile a folder is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WebKind {
    Electron,
    Chromium,
}

/// State of one run of H-WEB.
struct Run<'a, 'b> {
    hctx: &'a HeuristicContext<'b>,
    /// Report read problems (H-UNK is off).
    report: bool,
    opts: SummaryOptions,
    out: HeuristicOutput,
}

/// A found profile: its root, its probing summary and its kind.
type Probe = Option<(PathBuf, FolderSummary, WebKind)>;

impl Run<'_, '_> {
    /// The profile of the candidate `cand`: the candidate itself or its
    /// `User Data` folder. `Err` when the scan is cancelled.
    fn probe(&mut self, cand: &Path) -> Result<Probe, ()> {
        let Some(summary) = self.summary(cand)? else {
            return Ok(None);
        };
        if let Some(kind) = web_kind(&summary) {
            return Ok(Some((cand.to_path_buf(), summary, kind)));
        }
        let user_data = cand.join(USER_DATA);
        if !self.is_candidate_dir(&user_data) {
            return Ok(None);
        }
        let Some(summary) = self.summary(&user_data)? else {
            return Ok(None);
        };
        Ok(web_kind(&summary).map(|kind| (user_data, summary, kind)))
    }

    /// The probing summary of `dir`; `None` if it cannot be read, `Err` on
    /// cancellation.
    fn summary(&mut self, dir: &Path) -> Result<Option<FolderSummary>, ()> {
        let h = self.hctx;
        match summarize(h.scanner, dir, h.env, &self.opts, h.cancel) {
            Ok(summary) => Ok(Some(summary)),
            Err(FsError::Cancelled) => Err(()),
            Err(FsError::NotFound) => Ok(None),
            Err(err) => {
                if self.report {
                    self.out.issues.push(read_issue(dir, &err, h.env));
                }
                Ok(None)
            }
        }
    }

    /// `dir` is a folder (not a link), not excluded globally and not claimed.
    fn is_candidate_dir(&self, dir: &Path) -> bool {
        let h = self.hctx;
        let name = dir.file_name().unwrap_or(OsStr::new(""));
        h.scanner.metadata(dir).is_ok_and(|m| is_dir(&m))
            && h.excludes.check(dir, name, true) == Exclusion::Keep
            && !h.claimed.covers(dir)
    }

    /// The finding of the profile `root` of the application folder `app`
    /// (§4.6). A folder holding only caches is left to H-UNK.
    fn profile(&mut self, root: &Path, app: &Path, s: &FolderSummary, kind: WebKind) {
        let h = self.hctx;
        let top = self.listing(root);
        if top.iter().all(|e| is_cache_name(&e.name)) {
            return;
        }
        let mut exclude: Vec<String> = CACHE_EXCLUDE.map(str::to_owned).to_vec();
        let mut credentials = false;
        // The profile root and its subfolders: the profiles of `User Data`.
        let mut folders = vec![(root.to_path_buf(), top)];
        let subfolders: Vec<PathBuf> = folders[0]
            .1
            .iter()
            .filter(|e| e.is_dir && !is_cache_name(&e.name))
            .map(|e| root.join(&e.name))
            .collect();
        for dir in subfolders {
            let listing = self.listing(&dir);
            folders.push((dir, listing));
        }
        for (dir, listing) in &folders {
            credentials |= self.has_credentials(dir, listing);
            if let Some(ext) = synced_extensions(listing) {
                exclude.push(relative_glob(&dir.join(ext), root));
            }
        }
        let claimed = topmost(h.claimed.descendants(root));
        exclude.extend(claimed.iter().map(|p| relative_glob(p, root)));

        let mut confidence = CONFIDENCE;
        if is_opaque_name(&file_name(app)) {
            confidence -= OPAQUE_PENALTY;
        }
        let verdict = MarkerVerdict {
            category: Category::AppData,
            confidence,
            reason_key: match kind {
                WebKind::Electron => REASON_ELECTRON,
                WebKind::Chromium => REASON_CHROMIUM,
            },
            include: Vec::new(),
            exclude,
        };
        let target = Target::FileSet {
            root: PathTemplate::from_path(root, h.env),
            resolved: root.to_path_buf(),
            include: Vec::new(),
            exclude: verdict.exclude.clone(),
        };
        let args = BTreeMap::from([("app".to_owned(), file_name(app))]);
        let mut finding: Finding = new_finding(
            target,
            app,
            &verdict,
            args,
            HEURISTIC_ID,
            h.env,
            tags_from(s),
        );
        finding.title = format!(
            "{}{TITLE_SEPARATOR}{TITLE_APP_DATA}",
            display_name(app, h.env)
        );
        if credentials {
            finding.sensitivity = Sensitivity::High;
            finding.tags.push(TAG_BROWSER_CREDENTIALS.to_owned());
            finding.notes_key = Some(NOTES_DPAPI_BOUND.to_owned());
        }
        self.out.findings.push(finding);
        self.out.claimed.push(root.to_path_buf());
    }

    /// Entries of `dir` sorted by name; empty if it cannot be listed.
    fn listing(&self, dir: &Path) -> Vec<Entry> {
        let mut entries: Vec<Entry> = self
            .hctx
            .scanner
            .read_dir(dir)
            .unwrap_or_default()
            .into_iter()
            .filter(|e| e.meta.kind == EntryKind::File || is_dir(&e.meta))
            .map(|e| Entry {
                name: file_name(&e.path),
                is_dir: is_dir(&e.meta),
            })
            .collect();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        entries
    }

    /// `Cookies`, `Login Data`, `Web Data` or `Network\Cookies` in `dir`.
    fn has_credentials(&self, dir: &Path, listing: &[Entry]) -> bool {
        listing.iter().any(|e| {
            let lower = e.name.to_lowercase();
            if e.is_dir {
                lower == NETWORK
                    && self
                        .listing(&dir.join(&e.name))
                        .iter()
                        .any(|c| !c.is_dir && c.name.eq_ignore_ascii_case("cookies"))
            } else {
                CREDENTIAL_FILES.contains(&lower.as_str())
            }
        })
    }
}

/// An entry of a listed folder.
struct Entry {
    name: String,
    is_dir: bool,
}

/// `ChromiumProfile` wins over `ElectronApp`: an Electron app has no
/// `Bookmarks`, `History` or `Default\Preferences`.
fn web_kind(s: &FolderSummary) -> Option<WebKind> {
    if s.markers.contains(&Marker::ChromiumProfile) {
        Some(WebKind::Chromium)
    } else if s.markers.contains(&Marker::ElectronApp) {
        Some(WebKind::Electron)
    } else {
        None
    }
}

fn is_cache_name(name: &str) -> bool {
    CACHE_NAMES.contains(&name.to_lowercase().as_str())
}

/// The name of the `extensions` folder of a listing when a `Sync Data`
/// folder stands next to it (§4.6: excluded); otherwise it is saved.
fn synced_extensions(listing: &[Entry]) -> Option<&str> {
    let synced = listing
        .iter()
        .any(|e| e.is_dir && e.name.eq_ignore_ascii_case(SYNC_DATA));
    listing
        .iter()
        .find(|e| e.is_dir && e.name.eq_ignore_ascii_case(EXTENSIONS))
        .filter(|_| synced)
        .map(|e| e.name.as_str())
}
