//! H-JUNK over the subfolders of findings (SPEC-07 §4.5, "Уточнения (T-07-13)"):
//! junk subfolders of at least 1 MiB inside the findings of H-USR and H-UNK
//! become `exclude` globs of those findings.
//!
//! Only the topmost junk subfolders are taken; the search does not go into a
//! junk subfolder, and smaller junk stays in the finding. Folders under the
//! global exclusions (SPEC-03 §4.5) are never seen by the walk or by
//! `summarize`, and Measure and the backup skip them anyway. The excluded
//! subfolders are not claimed and do not change the aggregates of a finding.

use std::path::Path;

use sk_core::model::{Category, FolderSummary};
use sk_core::path::PathSet;
use sk_core::template::PathTemplate;

use crate::finding::file_name;
use crate::heuristic::HeuristicContext;
use crate::junk::{
    junk_verdict_with_siblings, REASON_CACHE_NAME, REASON_DOWNLOADABLE, REASON_LAUNCHER,
    REASON_RESTORABLE,
};
use crate::unk::relative_glob;
use crate::unk_zones::is_dir;
use crate::usr_finding::summary_of;
use crate::verdict::MarkerVerdict;
use crate::walk_tree::{DirAggregate, RootsWalk};

/// A junk subfolder goes into `exclude` from this size on: 1 MiB.
pub(crate) const MIN_JUNK_BYTES: u64 = 1024 * 1024;

/// Rows of §4.5 that depend on the name only (1, 3, 6, 7): the summary of a
/// child in `top_children` has no markers.
const NAME_REASONS: [&str; 4] = [
    REASON_CACHE_NAME,
    REASON_RESTORABLE,
    REASON_LAUNCHER,
    REASON_DOWNLOADABLE,
];

/// Whether the junk `exclude` lists are built (`enabled.junk`).
pub(crate) fn enabled(hctx: &HeuristicContext<'_>) -> bool {
    hctx.cfg.enabled.junk
}

/// §4.5 for the walked folder `i` with the aggregates `agg`: its name, its
/// neighbours and the environment. The walk keeps the names of folders only,
/// so `siblings` are the other subfolders of its parent (the
/// `target`/`bin`/`obj` rows are global exclusions of the walk anyway,
/// SPEC-03 §4.5).
pub(crate) fn walked_verdict(
    hctx: &HeuristicContext<'_>,
    walk: &RootsWalk,
    i: usize,
    agg: &DirAggregate,
) -> Option<MarkerVerdict> {
    let node = &walk.dirs[i];
    let siblings: Vec<String> = node.parent.map_or_else(Vec::new, |p| {
        walk.children(p)
            .filter(|&c| c != i)
            .map(|c| file_name(&walk.dirs[c].path))
            .collect()
    });
    let summary = summary_of(&node.path, agg, hctx.env);
    junk_verdict_with_siblings(&summary, hctx.env, &siblings)
}

/// H-USR: the topmost junk subfolders of at least 1 MiB below the walked
/// folder `node` (not `node` itself), in walk order. Claimed folders are
/// skipped with their contents: they are excluded already.
pub(crate) fn walked_junk_dirs(
    hctx: &HeuristicContext<'_>,
    walk: &RootsWalk,
    node: usize,
) -> Vec<usize> {
    let mut out = Vec::new();
    let mut stack: Vec<usize> = walk.children(node).collect();
    stack.reverse();
    while let Some(i) = stack.pop() {
        let dir = &walk.dirs[i];
        if hctx.claimed.covers(&dir.path) {
            continue;
        }
        if dir.subtree.bytes >= MIN_JUNK_BYTES
            && walked_verdict(hctx, walk, i, &dir.subtree).is_some()
        {
            out.push(i);
            continue;
        }
        let first = stack.len();
        stack.extend(walk.children(i));
        stack[first..].reverse();
    }
    out
}

/// H-UNK: `exclude` globs of the junk direct children of `root` among
/// `summary.top_children`, by the rows of §4.5 that depend on the name only.
/// Not built for a finding that is junk itself (`verdict` Cache or
/// Reinstallable). Children covered by `claimed` are skipped (they are
/// excluded already); a child must still be a folder under its listed name,
/// since the names of the summary are redacted.
pub(crate) fn summary_junk_excludes(
    hctx: &HeuristicContext<'_>,
    root: &Path,
    summary: &FolderSummary,
    verdict: &MarkerVerdict,
    claimed: &PathSet,
) -> Vec<String> {
    if matches!(verdict.category, Category::Cache | Category::Reinstallable) {
        return Vec::new();
    }
    let env = hctx.env;
    let names: Vec<&str> = summary
        .top_children
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    let mut out = Vec::new();
    for child in &summary.top_children {
        if child.bytes < MIN_JUNK_BYTES {
            continue;
        }
        let path = root.join(&child.name);
        if claimed.covers(&path) {
            continue;
        }
        let siblings: Vec<&str> = names.iter().copied().filter(|n| *n != child.name).collect();
        let child_summary = FolderSummary {
            path: PathTemplate::from_path(&path, env),
            total_bytes: child.bytes,
            file_count: child.files,
            dir_count: 0,
            max_depth: 0,
            newest_mtime: None,
            oldest_mtime: None,
            ext_histogram: Vec::new(),
            sample_names: Vec::new(),
            top_children: Vec::new(),
            markers: Vec::new(),
            truncated: false,
        };
        let junk = junk_verdict_with_siblings(&child_summary, env, &siblings)
            .is_some_and(|v| NAME_REASONS.contains(&v.reason_key));
        if junk && hctx.scanner.metadata(&path).is_ok_and(|m| is_dir(&m)) {
            out.push(relative_glob(&path, root));
        }
    }
    out
}
