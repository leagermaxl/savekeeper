//! `measure_all`: the Measure phase over all findings (SPEC-03 §4.3).

use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};

use rayon::prelude::*;
use sk_core::events::{Event, EventSink, ScanPhase, ThrottledSink};
use sk_core::model::{Finding, IssueSeverity, ScanIssue, Target, TargetStats};

use super::cache::CacheKey;
use super::{
    counted, measure_target, Acc, Counted, DirStatsCache, MeasureOptions, Mode, CACHE_DEPTH,
};
use crate::win::FILE_ATTRIBUTE_DIRECTORY;
use crate::{CancellationToken, EntryKind, EntryMeta, FsError, FsScanner, ReparseKind};

/// `source` of the issues of this phase.
const SOURCE: &str = "measure";
/// Tag of a finding whose root is a reparse point (§5).
const REPARSE_TAG: &str = "reparse_root";

/// A finding with a file system target.
#[derive(Debug)]
struct Job {
    idx: usize,
    /// Key of a `FileSet` root; `None` for a `File`.
    key: Option<CacheKey>,
    /// Template of the root or path, for progress and issues.
    template: String,
}

/// What measuring one finding gave.
#[derive(Debug)]
enum Outcome {
    Measured(TargetStats, u64),
    Reparse,
    Missing,
    Failed(String),
    Cancelled,
}

/// Measures every `FileSet`/`File` finding and fills its `stats`
/// (SPEC-03 §4.3); `Registry` and `SystemExport` findings keep `stats = None`.
///
/// The order of `findings` is kept. A `FileSet` 1–3 levels below another one
/// (both without globs, same exclusion mode), or repeating an earlier key, is
/// measured after it, from the cache: roots run level by level (a chain of
/// nested roots gives several levels), each level in parallel. Returns the issues
/// (`source = "measure"`): `issue.scan.reparse_root` (Info),
/// `issue.scan.dirs_unreadable` and `issue.scan.measure_failed` (Warning).
/// After cancellation the findings not yet measured keep `stats = None`.
/// Progress goes to `events` as `Event::Progress` of `ScanPhase::Measure`,
/// throttled.
pub fn measure_all(
    fs: &dyn FsScanner,
    findings: &mut [Finding],
    opts: &MeasureOptions,
    events: &EventSink,
    cancel: &CancellationToken,
) -> Vec<ScanIssue> {
    let mut jobs: Vec<Job> = findings
        .iter()
        .enumerate()
        .filter_map(|(idx, f)| job(idx, f, opts))
        .collect();
    let total = jobs.len() as u64;
    // Shorter roots first (the index is sorted, not the findings): a root
    // another one is nested in, or an earlier equal key, always comes before it.
    jobs.sort_by_key(|j| (j.key.as_ref().map_or(0, |k| k.path.len()), j.idx));
    let levels = levels(&jobs);
    let mut batches: Vec<Vec<&Job>> = Vec::new();
    for (job, level) in jobs.iter().zip(&levels) {
        if batches.len() <= *level {
            batches.resize_with(level + 1, Vec::new);
        }
        batches[*level].push(job);
    }

    let cache = DirStatsCache::new();
    let sink = ThrottledSink::new(events.clone());
    // Counting and sending under one lock keeps `done` increasing in the events.
    let done = Mutex::new(0u64);
    let shared: &[Finding] = findings;
    let run = |batch: &[&Job]| -> Vec<(usize, Outcome)> {
        batch
            .par_iter()
            .map(|job| {
                let target = &shared[job.idx].target;
                let outcome = measure_one(fs, target, &cache, opts, cancel);
                if !matches!(outcome, Outcome::Cancelled) {
                    let mut done = done.lock().unwrap_or_else(PoisonError::into_inner);
                    *done += 1;
                    sink.send(Event::Progress {
                        phase: ScanPhase::Measure,
                        done: *done,
                        total: Some(total),
                        current: Some(job.template.clone()),
                    });
                }
                (job.idx, outcome)
            })
            .collect()
    };
    // Level by level, so that a nested root finds its parent in the cache.
    let mut outcomes = Vec::new();
    for batch in &batches {
        outcomes.extend(run(batch));
    }
    sink.flush();

    outcomes.sort_by_key(|(idx, _)| *idx);
    let templates: BTreeMap<usize, &str> =
        jobs.iter().map(|j| (j.idx, j.template.as_str())).collect();
    let mut issues = Vec::new();
    for (idx, outcome) in outcomes {
        let finding = &mut findings[idx];
        let path = templates.get(&idx).map(|t| (*t).to_owned());
        match outcome {
            Outcome::Measured(stats, errors) => {
                finding.stats = Some(stats);
                if errors > 0 {
                    let args = [("count", errors.to_string())];
                    issues.push(issue(
                        IssueSeverity::Warning,
                        "issue.scan.dirs_unreadable",
                        path,
                        &args,
                    ));
                }
            }
            Outcome::Reparse => {
                finding.stats = Some(Acc::default().stats(false));
                if !finding.tags.iter().any(|t| t == REPARSE_TAG) {
                    finding.tags.push(REPARSE_TAG.to_owned());
                }
                issues.push(issue(
                    IssueSeverity::Info,
                    "issue.scan.reparse_root",
                    path,
                    &[],
                ));
            }
            Outcome::Failed(error) => {
                finding.stats = None;
                let args = [("error", error)];
                issues.push(issue(
                    IssueSeverity::Warning,
                    "issue.scan.measure_failed",
                    path,
                    &args,
                ));
            }
            Outcome::Missing | Outcome::Cancelled => finding.stats = None,
        }
    }
    issues
}

fn job(idx: usize, finding: &Finding, opts: &MeasureOptions) -> Option<Job> {
    match &finding.target {
        Target::FileSet {
            root,
            resolved,
            include,
            exclude,
        } => Some(Job {
            idx,
            key: Some(CacheKey::new(
                resolved,
                include,
                exclude,
                Mode::for_root(opts, resolved),
            )),
            template: root.as_str().to_owned(),
        }),
        Target::File { path, .. } => Some(Job {
            idx,
            key: None,
            template: path.as_str().to_owned(),
        }),
        Target::Registry { .. } | Target::SystemExport { .. } => None,
    }
}

/// Measuring level of each job of `jobs` (sorted by root length, then index):
/// 0 without a parent, else 1 + the highest level of its parents. A parent
/// is a root this one lies 1–3 levels below, or an earlier job with the same
/// key; both come earlier in `jobs`, so one pass suffices.
fn levels(jobs: &[Job]) -> Vec<usize> {
    let mut levels: Vec<usize> = Vec::with_capacity(jobs.len());
    for (i, job) in jobs.iter().enumerate() {
        let level = job.key.as_ref().map_or(0, |key| {
            jobs[..i]
                .iter()
                .zip(&levels)
                .filter(|(other, _)| {
                    other
                        .key
                        .as_ref()
                        .is_some_and(|other| other == key || key.nested_in(other, CACHE_DEPTH))
                })
                .map(|(_, level)| level + 1)
                .max()
                .unwrap_or(0)
        });
        levels.push(level);
    }
    levels
}

fn measure_one(
    fs: &dyn FsScanner,
    target: &Target,
    cache: &DirStatsCache,
    opts: &MeasureOptions,
    cancel: &CancellationToken,
) -> Outcome {
    if cancel.is_cancelled() {
        return Outcome::Cancelled;
    }
    let path = match target {
        Target::FileSet { resolved, .. } | Target::File { resolved, .. } => resolved,
        Target::Registry { .. } | Target::SystemExport { .. } => return Outcome::Missing,
    };
    match fs.metadata(path) {
        Ok(meta) if is_reparse_root(target, &meta) => return Outcome::Reparse,
        Ok(_) => {}
        Err(e) => return failure(e),
    }
    match measure_target(fs, target, cache, opts, cancel) {
        Ok(Some(c)) => Outcome::Measured(c.stats, c.errors),
        Ok(None) => Outcome::Missing,
        Err(e) => failure(e),
    }
}

fn failure(e: FsError) -> Outcome {
    match e {
        FsError::NotFound => Outcome::Missing,
        FsError::Cancelled => Outcome::Cancelled,
        e => Outcome::Failed(e.to_string()),
    }
}

/// A root that is a reparse point measure does not count (§4.3): for a
/// `FileSet` any reparse point except a cloud placeholder folder; for a
/// `File` a symlink, junction or other link (cloud placeholder files and app
/// aliases count as files).
fn is_reparse_root(target: &Target, meta: &EntryMeta) -> bool {
    match target {
        Target::FileSet { .. } => match meta.kind {
            EntryKind::Reparse(ReparseKind::CloudPlaceholder) => {
                meta.attrs & FILE_ATTRIBUTE_DIRECTORY == 0
            }
            EntryKind::Reparse(_) => true,
            EntryKind::File | EntryKind::Dir => false,
        },
        _ => matches!(meta.kind, EntryKind::Reparse(_)) && counted(meta) == Counted::Not,
    }
}

fn issue(
    severity: IssueSeverity,
    key: &str,
    path: Option<String>,
    args: &[(&str, String)],
) -> ScanIssue {
    ScanIssue {
        severity,
        source: SOURCE.to_owned(),
        path,
        message_key: key.to_owned(),
        message_args: args
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect(),
    }
}

#[cfg(test)]
#[path = "all_tests.rs"]
mod tests;
