//! `summarize`: a compact, anonymized `FolderSummary` with markers
//! (SPEC-03 §4.4, SPEC-02 §4).
//!
//! The folder is walked once through [`FsScanner`]; memory stays bounded by
//! the number of extensions and direct subfolders plus a few fixed-size
//! selections (NFR-03-02). Every order is deterministic, since `walk` gives no
//! order between folders and SPEC-08 keys its cache by the summary content.
//! Files are only read for the SQLite signature, at most five of them, and
//! cloud-only files never (FR-03-03).

mod acc;
mod rules;

use std::collections::HashSet;
use std::io;
use std::path::Path;
use std::sync::Arc;

use sk_core::env::Environment;
use sk_core::model::{ChildStat, ExtStat, FolderSummary};
use sk_core::privacy::redact;
use sk_core::template::PathTemplate;

use crate::measure::{counted, Counted, Mode};
use crate::{CancellationToken, ExcludeSet, FsError, FsScanner, WalkControl, WalkOptions};
use acc::{Acc, ExtAcc, NEWEST_SAMPLES};
use rules::SQLITE_MAGIC;

/// Default limit of walked entries.
const MAX_ENTRIES: u64 = 200_000;
/// Default walk depth.
const MAX_DEPTH: u32 = 12;
/// Extensions in `ext_histogram`.
const TOP_EXTS: usize = 15;
/// Paths in `sample_names` at most.
const MAX_SAMPLES: usize = 20;
/// Folders in `top_children`.
const TOP_CHILDREN: usize = 10;

/// Parameters of [`summarize`] (SPEC-03 §4.1).
#[derive(Debug, Clone)]
pub struct SummaryOptions {
    /// Entries walked at most (200 000 by default); then `truncated`.
    pub max_entries: u64,
    /// Walk depth limit (12 by default).
    pub max_depth: u32,
    /// Global exclusions (§4.5), applied as in `measure` (`explicit_root`).
    pub excludes: Arc<ExcludeSet>,
}

impl SummaryOptions {
    /// Options with `excludes`; the limits get their defaults.
    pub fn new(excludes: Arc<ExcludeSet>) -> Self {
        Self {
            max_entries: MAX_ENTRIES,
            max_depth: MAX_DEPTH,
            excludes,
        }
    }
}

/// Summary of the folder `dir` (SPEC-03 §4.4).
///
/// A missing `dir` is [`FsError::NotFound`], a file is [`FsError::Io`]. A
/// reparse point other than a cloud placeholder folder is not entered: the
/// summary has zero counts and only the path markers (`CloudSynced`,
/// `UwpPackage`). Cancellation is [`FsError::Cancelled`].
pub fn summarize(
    fs: &dyn FsScanner,
    dir: &Path,
    env: &Environment,
    opts: &SummaryOptions,
    cancel: &CancellationToken,
) -> Result<FolderSummary, FsError> {
    let meta = fs.metadata(dir)?;
    let template = PathTemplate::from_path(dir, env);
    let dir_name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match counted(&meta) {
        Counted::File => {
            return Err(FsError::Io(io::Error::new(
                io::ErrorKind::NotADirectory,
                "the folder to summarize is a file",
            )));
        }
        Counted::Not => {
            let markers = rules::path_markers(dir, &template, env);
            return Ok(build(Acc::new(&dir_name), template, markers, false, env));
        }
        Counted::Dir => {}
    }
    let walk_opts = WalkOptions {
        max_depth: opts.max_depth,
        max_entries: opts.max_entries,
        follow_links: false,
        excludes: Mode::for_excludes(&opts.excludes, dir).filter(&opts.excludes),
        include: None,
        exclude: None,
        threads: 0,
    };
    let mut acc = Acc::new(&dir_name);
    let walk = fs.walk(
        dir,
        &walk_opts,
        &mut |e| {
            acc.visit(e);
            WalkControl::Continue
        },
        cancel,
    )?;
    let sqlite = has_sqlite(fs, acc.take_sqlite());
    let markers = rules::markers(&acc, dir, &template, env, sqlite);
    Ok(build(
        acc,
        template,
        markers,
        walk.truncated || walk.errors > 0,
        env,
    ))
}

/// Reads the signatures of the SQLite candidates (at most five, smallest
/// depth and path first) until one matches; read errors count as tries.
fn has_sqlite(fs: &dyn FsScanner, candidates: Vec<acc::SqliteCandidate>) -> bool {
    candidates.into_iter().any(|(_, _, path)| {
        fs.read_head(&path, SQLITE_MAGIC.len())
            .is_ok_and(|head| head.as_slice() == SQLITE_MAGIC)
    })
}

fn build(
    acc: Acc,
    path: PathTemplate,
    markers: Vec<sk_core::model::Marker>,
    truncated: bool,
    env: &Environment,
) -> FolderSummary {
    let mut hist: Vec<(String, ExtAcc)> = acc.exts.into_iter().collect();
    hist.sort_by(|(xa, a), (xb, b)| {
        b.count
            .cmp(&a.count)
            .then(b.bytes.cmp(&a.bytes))
            .then(xa.cmp(xb))
    });
    let sample_names = samples(acc.top_level, acc.fresh, &hist)
        .iter()
        .map(|s| redact(s, env))
        .collect();
    let mut children: Vec<_> = acc.children.into_values().collect();
    children.sort_by(|a, b| {
        b.bytes
            .cmp(&a.bytes)
            .then(b.files.cmp(&a.files))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    FolderSummary {
        path,
        total_bytes: acc.total_bytes,
        file_count: acc.file_count,
        dir_count: acc.dir_count,
        max_depth: acc.max_depth,
        newest_mtime: acc.newest,
        oldest_mtime: acc.oldest,
        ext_histogram: hist
            .iter()
            .take(TOP_EXTS)
            .map(|(ext, a)| ExtStat {
                ext: ext.clone(),
                count: a.count,
                bytes: a.bytes,
            })
            .collect(),
        sample_names,
        top_children: children
            .into_iter()
            .take(TOP_CHILDREN)
            .map(|c| ChildStat {
                name: redact(&c.name, env),
                bytes: c.bytes,
                files: c.files,
            })
            .collect(),
        markers,
        truncated,
    }
}

/// The three `sample_names` steps (§4.4), before redaction.
fn samples(
    top_level: acc::Smallest<acc::TopLevel>,
    fresh: acc::Smallest<acc::Fresh>,
    hist: &[(String, ExtAcc)],
) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(MAX_SAMPLES);
    let mut chosen: HashSet<String> = HashSet::new();
    let mut exts: HashSet<String> = HashSet::new();
    for (path, ext) in top_level.into_sorted() {
        exts.extend(ext);
        chosen.insert(path.display.clone());
        out.push(path.display);
    }
    let mut newest = 0;
    for (_, path, ext) in fresh.into_sorted() {
        if newest == NEWEST_SAMPLES {
            break;
        }
        if chosen.insert(path.display.clone()) {
            exts.insert(ext);
            out.push(path.display);
            newest += 1;
        }
    }
    for (ext, a) in hist {
        if out.len() >= MAX_SAMPLES {
            break;
        }
        if !ext.is_empty() && exts.insert(ext.clone()) {
            out.push(a.first.display.clone());
        }
    }
    out
}

#[cfg(test)]
#[path = "summary_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "summary_marker_tests.rs"]
mod marker_tests;
