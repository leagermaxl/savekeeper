//! H-UNK: unknown top-level folders of the search zones (SPEC-07 §4.2).
//!
//! Every candidate of [`unk_zones`](crate::unk_zones) not covered by the
//! claimed paths is summarized (SPEC-03 §4.4), classified by its markers
//! (§4.7) and becomes one finding; below `unknown_threshold` the finding is
//! `Unknown` and its summary goes to the LLM (FR-07-02, FR-07-08).

use std::collections::BTreeMap;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sk_core::fs::{EntryKind, FsError};
use sk_core::model::{Category, FolderSummary, Marker, Target, TargetStats};
use sk_core::path::{starts_with_ci, PathSet};
use sk_core::template::PathTemplate;
use sk_scan::{summarize, SummaryOptions};
use time::{Duration, OffsetDateTime};

use crate::classify::{classify_by_markers_with, REASON_EMPTY, REASON_UNKNOWN};
use crate::data;
use crate::finding::{file_name, is_opaque_name, make_finding, new_finding};
use crate::heuristic::{Heuristic, HeuristicContext, HeuristicOutput};
use crate::junk::is_specific;
use crate::junk_exclude;
use crate::unk_zones::{candidates, is_dir, read_issue, read_reason, Candidate, Kind};
use crate::verdict::MarkerVerdict;

#[cfg(test)]
#[path = "unk_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "unk_zone_tests.rs"]
mod zone_tests;

#[cfg(test)]
#[path = "unk_edge_tests.rs"]
mod edge_tests;

#[cfg(test)]
#[path = "unk_fixture_tests.rs"]
mod fixture_tests;

#[cfg(test)]
#[path = "unk_junk_tests.rs"]
mod junk_tests;

/// `Heuristic::id` of H-UNK.
pub(crate) const HEURISTIC_ID: &str = "h-unk";
/// Reason: a Store package with Xbox / Game Pass saves (`SystemAppData\wgs`).
pub(crate) const REASON_UWP_SAVES: &str = "heuristic.uwp_saves";
/// Reason: the data of a Store package.
pub(crate) const REASON_UWP_PACKAGE: &str = "heuristic.uwp_package";
/// Reason: a small dot file in the profile (`.gitconfig`).
pub(crate) const REASON_DOTFILE: &str = "heuristic.dotfile";

/// Folders of a Store package that are saved (§4.2.3).
const PACKAGE_INCLUDE: [&str; 4] = [
    "LocalState/**",
    "RoamingState/**",
    "Settings/**",
    "SystemAppData/wgs/**",
];
/// Folders of a Store package that are not saved (§4.2.3).
const PACKAGE_EXCLUDE: [&str; 3] = ["LocalCache", "TempState", "AC"];
/// Confidence of a Store package with `SystemAppData\wgs`.
const UWP_SAVES_CONFIDENCE: f32 = 0.8;
/// Confidence of any other Store package: a candidate for the LLM.
const UWP_PACKAGE_CONFIDENCE: f32 = 0.5;
/// A `{PROGRAMDATA}` folder is kept if a file changed within this many days.
const PROGRAMDATA_RECENT_DAYS: i64 = 365;
/// A partly claimed candidate is kept if the rest is at least 1 MiB ...
const MIN_REST_BYTES: u64 = 1024 * 1024;
/// ... or at least this share of it, in percent (§5).
const MIN_REST_PERCENT: u128 = 10;
/// Largest dot file taken for a config (as rule 6 of §4.7): 10 MiB.
const DOTFILE_MAX_BYTES: u64 = 10 * 1024 * 1024;
/// Confidence of a dot file (as a config-like dot folder, rule 6 of §4.7).
const DOTFILE_CONFIDENCE: f32 = 0.65;
/// Confidence lost by a folder named by a GUID or a hash (§5).
const OPAQUE_PENALTY: f32 = 0.1;

/// H-UNK: one finding per unclaimed top-level folder of the search zones.
#[derive(Debug, Default)]
pub(crate) struct HUnk {
    /// "Now" for the age rules; the start of the run when `None`.
    now: Option<OffsetDateTime>,
}

impl HUnk {
    /// H-UNK measuring ages from the start of each run.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// H-UNK measuring ages from `now`.
    #[cfg(test)]
    pub(crate) fn at(now: OffsetDateTime) -> Self {
        Self { now: Some(now) }
    }
}

impl Heuristic for HUnk {
    fn id(&self) -> &'static str {
        HEURISTIC_ID
    }

    fn run(&self, hctx: &HeuristicContext<'_>) -> HeuristicOutput {
        let vendors = data::builtin().ok().map(|d| &d.vendors);
        let (cands, issues) =
            candidates(hctx.scanner, hctx.env, hctx.excludes, hctx.claimed, vendors);
        let mut run = Run {
            hctx,
            // One "now" for the whole scan (SPEC-07 §4.1).
            now: self.now.unwrap_or_else(OffsetDateTime::now_utc),
            opts: SummaryOptions::new(Arc::clone(hctx.excludes)),
            seen: hctx.claimed.clone(),
            out: HeuristicOutput {
                issues,
                ..HeuristicOutput::default()
            },
        };
        for (done, cand) in (1u64..).zip(&cands) {
            if hctx.cancel.is_cancelled() {
                break;
            }
            let current = PathTemplate::from_path(&cand.path, hctx.env);
            (hctx.progress)(done, Some(current.as_str()));
            if run.candidate(cand).is_break() {
                break;
            }
        }
        run.out
    }
}

/// State of one run of H-UNK.
struct Run<'a, 'b> {
    hctx: &'a HeuristicContext<'b>,
    now: OffsetDateTime,
    opts: SummaryOptions,
    /// Claimed paths: those of the context plus the candidates done.
    seen: PathSet,
    out: HeuristicOutput,
}

impl Run<'_, '_> {
    /// Handles one candidate; `Break` when the scan is cancelled.
    fn candidate(&mut self, cand: &Candidate) -> ControlFlow<()> {
        let path = cand.path.as_path();
        if self.seen.covers(path) {
            return ControlFlow::Continue(());
        }
        if let Kind::DotFile(size) = cand.kind {
            self.dot_file(path, size);
            return ControlFlow::Continue(());
        }
        let env = self.hctx.env;
        let summary = match summarize(self.hctx.scanner, path, env, &self.opts, self.hctx.cancel) {
            Ok(summary) => summary,
            Err(FsError::NotFound) => return ControlFlow::Continue(()),
            Err(FsError::Cancelled) => return ControlFlow::Break(()),
            Err(err) => {
                self.unreadable(path, &err);
                return ControlFlow::Continue(());
            }
        };
        if cand.kind == Kind::ProgramData && !self.keeps_program_data(path, &summary) {
            return ControlFlow::Continue(());
        }
        let Some(rest_excludes) = self.unclaimed_rest(path, &summary) else {
            return ControlFlow::Continue(());
        };
        let verdict = if cand.kind == Kind::Package {
            package_verdict(self.hctx, path)
        } else {
            let thresholds = &self.hctx.cfg.thresholds;
            Some(classify_by_markers_with(
                &summary, env, thresholds, self.now,
            ))
        };
        let Some(mut verdict) = verdict.filter(|v| v.reason_key != REASON_EMPTY) else {
            // Nothing to save: explained, without a finding (§4.7 rule 11).
            self.claim(path);
            return ControlFlow::Continue(());
        };
        if is_opaque_name(&file_name(path)) {
            verdict.confidence = (verdict.confidence - OPAQUE_PENALTY).max(0.0);
        }
        verdict.exclude.extend(rest_excludes);
        // Junk subfolders (§4.5, T-07-13); a Store package has its own exclude.
        if cand.kind != Kind::Package && junk_exclude::enabled(self.hctx) {
            let junk = junk_exclude::summary_junk_excludes(
                self.hctx, path, &summary, &verdict, &self.seen,
            );
            verdict.exclude.extend(junk);
        }
        let mut finding = make_finding(path, &verdict, &summary, HEURISTIC_ID, env);
        if verdict.confidence < self.hctx.cfg.unknown_threshold {
            finding.category = Category::Unknown;
            self.out.unknown.push(summary);
        }
        self.out.findings.push(finding);
        self.claim(path);
        ControlFlow::Continue(())
    }

    fn claim(&mut self, path: &Path) {
        self.seen.insert(path);
        self.out.claimed.push(path.to_path_buf());
    }

    /// A dot file of the profile: a config if it is small (§4.2.1 `{HOME}\.*`).
    fn dot_file(&mut self, path: &Path, size: u64) {
        let env = self.hctx.env;
        let verdict = if size < DOTFILE_MAX_BYTES {
            MarkerVerdict::whole(Category::DevEnvironment, DOTFILE_CONFIDENCE, REASON_DOTFILE)
        } else {
            MarkerVerdict::whole(Category::Unknown, 0.0, REASON_UNKNOWN)
        };
        let target = Target::File {
            path: PathTemplate::from_path(path, env),
            resolved: path.to_path_buf(),
        };
        let args = BTreeMap::from([("bytes".to_owned(), size.to_string())]);
        let mut finding = new_finding(target, path, &verdict, args, HEURISTIC_ID, env, Vec::new());
        // No summary for a file: an unknown dot file is not sent to the LLM.
        if verdict.confidence < self.hctx.cfg.unknown_threshold {
            finding.category = Category::Unknown;
        }
        self.out.findings.push(finding);
        self.claim(path);
    }

    /// A candidate that could not be read: an `Unknown` finding with
    /// truncated stats and a warning; no summary goes to the LLM (§5).
    fn unreadable(&mut self, path: &Path, err: &FsError) {
        let env = self.hctx.env;
        let issue = read_issue(path, err, env);
        let verdict = MarkerVerdict::whole(Category::Unknown, 0.0, read_reason(err));
        let target = Target::FileSet {
            root: PathTemplate::from_path(path, env),
            resolved: path.to_path_buf(),
            include: Vec::new(),
            exclude: Vec::new(),
        };
        let mut finding = new_finding(
            target,
            path,
            &verdict,
            issue.message_args.clone(),
            HEURISTIC_ID,
            env,
            Vec::new(),
        );
        finding.stats = Some(truncated_stats());
        self.out.issues.push(issue);
        self.out.findings.push(finding);
        self.claim(path);
    }

    /// §4.2.1 `{PROGRAMDATA}`: a file changed within a year, no programs, and
    /// not the installation folder of an installed program.
    fn keeps_program_data(&self, path: &Path, s: &FolderSummary) -> bool {
        let env = self.hctx.env;
        let recent = s
            .newest_mtime
            .is_some_and(|m| self.now - m < Duration::days(PROGRAMDATA_RECENT_DAYS));
        let installed = env
            .installed_programs
            .iter()
            .filter_map(|p| p.install_location.as_deref())
            .filter(|location| is_specific(location, env))
            .any(|location| starts_with_ci(path, location));
        recent && !s.markers.contains(&Marker::HasExecutables) && !installed
    }

    /// Exclude globs of the claimed parts of `root` (§4.2.4), or `None` when
    /// the rest is below both 10% and 1 MiB and the candidate is skipped (§5).
    /// A truncated summary has not seen every file, so its rest proves
    /// nothing: the candidate is kept (as rule 11 of §4.7).
    fn unclaimed_rest(&self, root: &Path, s: &FolderSummary) -> Option<Vec<String>> {
        let parts = topmost(self.seen.descendants(root));
        if parts.is_empty() {
            return Some(Vec::new());
        }
        let claimed: u64 = parts
            .iter()
            .map(|p| self.size_of(p))
            .fold(0, u64::saturating_add);
        let rest = s.total_bytes.saturating_sub(claimed);
        let enough = s.truncated
            || rest >= MIN_REST_BYTES
            || u128::from(rest) * 100 >= u128::from(s.total_bytes) * MIN_REST_PERCENT;
        enough.then(|| parts.iter().map(|p| relative_glob(p, root)).collect())
    }

    /// Size of a claimed path: the summary of a folder, the size of a file;
    /// 0 if it cannot be read.
    fn size_of(&self, path: &Path) -> u64 {
        let h = self.hctx;
        match h.scanner.metadata(path) {
            Ok(meta) if is_dir(&meta) => {
                summarize(h.scanner, path, h.env, &self.opts, h.cancel).map_or(0, |s| s.total_bytes)
            }
            Ok(meta) if meta.kind == EntryKind::File => meta.size,
            _ => 0,
        }
    }
}

/// §4.2.3: a Store package saves its state folders; with `SystemAppData\wgs`
/// it holds game saves. `None` when it has none of them.
fn package_verdict(hctx: &HeuristicContext<'_>, package: &Path) -> Option<MarkerVerdict> {
    let has_dir = |rel: &[&str]| {
        let path = rel.iter().fold(package.to_path_buf(), |p, c| p.join(c));
        hctx.scanner.metadata(&path).is_ok_and(|m| is_dir(&m))
    };
    let wgs = has_dir(&["SystemAppData", "wgs"]);
    let state = ["LocalState", "RoamingState", "Settings"]
        .iter()
        .any(|d| has_dir(&[d]));
    if !wgs && !state {
        return None;
    }
    let (category, confidence, reason_key) = if wgs {
        (Category::GameSave, UWP_SAVES_CONFIDENCE, REASON_UWP_SAVES)
    } else {
        (
            Category::AppData,
            UWP_PACKAGE_CONFIDENCE,
            REASON_UWP_PACKAGE,
        )
    };
    Some(MarkerVerdict {
        category,
        confidence,
        reason_key,
        include: PACKAGE_INCLUDE.map(str::to_owned).to_vec(),
        exclude: PACKAGE_EXCLUDE.map(str::to_owned).to_vec(),
    })
}

/// Stats of a candidate that could not be read.
fn truncated_stats() -> TargetStats {
    TargetStats {
        total_bytes: 0,
        file_count: 0,
        dir_count: 0,
        newest_mtime: None,
        oldest_mtime: None,
        locked_files: 0,
        cloud_only_bytes: 0,
        cloud_only_files: 0,
        largest_file_bytes: None,
        truncated: true,
    }
}

/// The paths that do not lie under another one of them, in the given order
/// (parents come before their children).
pub(crate) fn topmost(paths: Vec<&Path>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for path in paths {
        if !out.iter().any(|top| starts_with_ci(path, top)) {
            out.push(path.to_path_buf());
        }
    }
    out
}

/// `path` relative to `root` as a glob with `/` separators; glob characters
/// in names are escaped. `path` lies under `root`.
pub(crate) fn relative_glob(path: &Path, root: &Path) -> String {
    let parts: Vec<String> = path
        .components()
        .skip(root.components().count())
        .map(|c| glob_escape(&c.as_os_str().to_string_lossy()))
        .collect();
    parts.join("/")
}

/// Escapes `*?[]{}` as one-character classes (`[*]`), as `globset::escape`.
fn glob_escape(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if matches!(c, '*' | '?' | '[' | ']' | '{' | '}') {
            out.push('[');
            out.push(c);
            out.push(']');
        } else {
            out.push(c);
        }
    }
    out
}
