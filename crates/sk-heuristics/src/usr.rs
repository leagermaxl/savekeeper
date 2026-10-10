//! H-USR: user files outside the standard places (SPEC-07 §4.3).
//!
//! Works on the shared walk of the roots (§4.3.1, [`RootsWalkCache`]): the
//! per-folder aggregates are turned into "candidate folders" (§4.3.3) instead
//! of one finding per file. A standard user folder (`{DOCUMENTS}` …) that no
//! one claimed gives one finding for the whole folder; single valuable files
//! (`credentials`, `ebook_mail`) outside the clusters give a finding each.
//!
//! The walk is shared with H-GIT and does not know the claimed paths: the
//! aggregates of a folder with claimed subfolders are recomputed without them,
//! and the claimed parts become `exclude` globs of the finding (as in §4.2.4).
//!
//! [`RootsWalkCache`]: crate::walk::RootsWalkCache

use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path};

use sk_core::env::KnownFolder;
use sk_core::model::{IssueSeverity, ScanIssue};
use sk_core::path::{eq_ci, starts_with_ci, PathSet};

use crate::collector::COLLECTOR_ID;
use crate::data::{self, ExtensionTable};
use crate::finding::file_name;
use crate::heuristic::{Heuristic, HeuristicContext, HeuristicOutput};
use crate::junk::junk_verdict_with_siblings;
use crate::unk::{relative_glob, topmost};
use crate::usr_finding::{
    cluster_verdict, file_finding, folder_finding, positive_weight, summary_of, REASON_CLUSTER,
    REASON_USER_FOLDER,
};
use crate::verdict::MarkerVerdict;
use crate::walk_tree::{DirAggregate, RootsWalk};

#[cfg(test)]
#[path = "usr_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "usr_cluster_tests.rs"]
mod cluster_tests;

#[cfg(test)]
#[path = "usr_fixture_tests.rs"]
mod fixture_tests;

/// `Heuristic::id` of H-USR.
pub(crate) const HEURISTIC_ID: &str = "h-usr";
/// Issue: more clusters than `usr.max_candidates`; the lightest are dropped.
pub(crate) const ISSUE_TRUNCATED: &str = "heuristic.usr_truncated";
/// A child holding this share of its parent's weight takes its place (§4.3.3, step 3).
const DOMINANT_SHARE: f64 = 0.9;
/// A cluster holds less than this share of program bytes (`binary` group).
const MAX_BINARY_SHARE: f64 = 0.3;
/// The standard user folders (§4.3.1): one finding for the whole folder.
const STANDARD_FOLDERS: [KnownFolder; 6] = [
    KnownFolder::Documents,
    KnownFolder::Desktop,
    KnownFolder::Pictures,
    KnownFolder::Music,
    KnownFolder::Videos,
    KnownFolder::Downloads,
];

/// H-USR: clusters of user files found by the shared walk of the roots.
#[derive(Debug, Default)]
pub(crate) struct HUsr;

impl HUsr {
    /// H-USR.
    pub(crate) fn new() -> Self {
        Self
    }
}

impl Heuristic for HUsr {
    fn id(&self) -> &'static str {
        HEURISTIC_ID
    }

    fn run(&self, hctx: &HeuristicContext<'_>) -> HeuristicOutput {
        let (walk, issues) = hctx.roots_walk.get(hctx);
        let mut out = HeuristicOutput {
            issues,
            ..HeuristicOutput::default()
        };
        // An incomplete walk would give wrong clusters.
        if walk.cancelled || hctx.cancel.is_cancelled() {
            return out;
        }
        let table = data::builtin().ok().map(|d| &d.extensions);
        let mut select = Select::new(hctx, walk, table);
        for root in &walk.roots {
            select.root(root.node);
        }
        let max = usize::try_from(hctx.cfg.usr.max_candidates).unwrap_or(usize::MAX);
        let (kept, dropped) = limit(select.picks, max);
        if dropped > 0 {
            out.issues.push(truncated_issue(dropped, max));
        }
        build(hctx, walk, table, &kept, &mut out);
        out
    }
}

/// What a picked folder becomes.
#[derive(Debug, Clone, PartialEq)]
enum PickKind {
    /// A cluster of user files (§4.3.3, step 3).
    Cluster,
    /// A standard user folder, whole (§4.3.1).
    UserFolder,
    /// A cluster that is junk or reinstallable (§4.5).
    Junk(MarkerVerdict),
}

/// A picked folder with its aggregates (without the claimed parts).
#[derive(Debug, Clone, PartialEq)]
struct Pick {
    /// Index in [`RootsWalk::dirs`].
    node: usize,
    agg: DirAggregate,
    kind: PickKind,
}

/// Picks the folders of §4.3.3 from the walk, top down.
struct Select<'a, 'b> {
    hctx: &'a HeuristicContext<'b>,
    walk: &'a RootsWalk,
    table: Option<&'a ExtensionTable>,
    min_weight: f64,
    /// All known folders: a folder that is or holds one is never a cluster.
    known: Vec<&'a Path>,
    /// The standard user folders.
    standard: Vec<&'a Path>,
    /// Aggregates of folders with claimed descendants, without them.
    memo: HashMap<usize, DirAggregate>,
    /// Paths of the picks, so that a root inside one is not picked again.
    taken: PathSet,
    picks: Vec<Pick>,
}

impl<'a, 'b> Select<'a, 'b> {
    fn new(
        hctx: &'a HeuristicContext<'b>,
        walk: &'a RootsWalk,
        table: Option<&'a ExtensionTable>,
    ) -> Self {
        let env = hctx.env;
        Self {
            hctx,
            walk,
            table,
            min_weight: f64::from(hctx.cfg.usr.min_weight),
            known: env.known_folders.values().map(|p| p.as_path()).collect(),
            standard: STANDARD_FOLDERS
                .iter()
                .filter_map(|&f| env.known_folder(f))
                .collect(),
            memo: HashMap::new(),
            taken: PathSet::new(),
            picks: Vec::new(),
        }
    }

    /// A walked root; skipped when a pick of an outer root holds it.
    fn root(&mut self, node: usize) {
        if !self.taken.covers(&self.walk.dirs[node].path) {
            self.visit(node);
        }
    }

    /// The folder `i` and, unless it is picked or skipped, its subfolders.
    fn visit(&mut self, i: usize) {
        let walk = self.walk;
        let path = walk.dirs[i].path.as_path();
        if self.hctx.claimed.covers(path) {
            return;
        }
        if self.standard.iter().any(|s| eq_ci(s, path)) {
            let agg = self.effective(i);
            if agg.files > 0 {
                self.pick(i, agg, PickKind::UserFolder);
            }
            return;
        }
        // Step 4: a drive root is never a cluster; nor is a folder that holds
        // a known folder (`C:\Users`, `{HOME}`): their children are looked at.
        if is_drive_root(path) || self.known.iter().any(|k| starts_with_ci(k, path)) {
            self.visit_children(i);
            return;
        }
        let agg = self.effective(i);
        // No folder below can weigh more than the positive part of this one.
        if positive_weight(&agg, self.table) < self.min_weight {
            return;
        }
        // §4.5 runs over every folder looked at: junk is never entered; when
        // it passes the thresholds, it is a finding too (FR-07-07).
        if let Some(verdict) = self.junk(i, &agg) {
            if self.qualifies(&agg) {
                self.pick(i, agg, PickKind::Junk(verdict));
            }
            return;
        }
        if !self.qualifies(&agg) {
            self.visit_children(i);
            return;
        }
        let (mut d, mut agg) = (i, agg);
        while let Some((child, child_agg)) = self.dominant_child(d, &agg) {
            if let Some(verdict) = self.junk(child, &child_agg) {
                self.pick(child, child_agg, PickKind::Junk(verdict));
                return;
            }
            d = child;
            agg = child_agg;
        }
        self.pick(d, agg, PickKind::Cluster);
    }

    fn visit_children(&mut self, i: usize) {
        let walk = self.walk;
        for child in walk.children(i) {
            if self.hctx.cancel.is_cancelled() {
                return;
            }
            self.visit(child);
        }
    }

    /// Step 3: the folder is heavy enough and not mostly programs.
    fn qualifies(&self, agg: &DirAggregate) -> bool {
        // Sizes far below 2^53 bytes convert exactly enough.
        agg.files > 0
            && agg.user_weight >= self.min_weight
            && (agg.binary_bytes as f64) < MAX_BINARY_SHARE * agg.bytes as f64
    }

    /// Step 3, dominance: the only unclaimed child of `d` with at least 90%
    /// of its weight, when it is a cluster itself and what `d` keeps without
    /// it (own files, other children) weighs less than `min_weight`: the
    /// rest could not be a cluster, so no sibling is lost by the descent.
    fn dominant_child(&mut self, d: usize, agg: &DirAggregate) -> Option<(usize, DirAggregate)> {
        let walk = self.walk;
        let mut dominant = Vec::new();
        for child in walk.children(d) {
            if self.hctx.claimed.covers(&walk.dirs[child].path) {
                continue;
            }
            let child_agg = self.effective(child);
            if child_agg.user_weight >= DOMINANT_SHARE * agg.user_weight {
                dominant.push((child, child_agg));
            }
        }
        let [(child, child_agg)] = <[_; 1]>::try_from(dominant).ok()?;
        let rest = positive_weight(agg, self.table) - positive_weight(&child_agg, self.table);
        (self.qualifies(&child_agg) && rest < self.min_weight).then_some((child, child_agg))
    }

    /// §4.5 for a folder: its name, its neighbours and the environment.
    /// The walk keeps the names of folders only, so `siblings` are the other
    /// subfolders of its parent (the `target`/`bin`/`obj` rows are global
    /// exclusions of the walk anyway, SPEC-03 §4.5).
    fn junk(&self, i: usize, agg: &DirAggregate) -> Option<MarkerVerdict> {
        let walk = self.walk;
        let node = &walk.dirs[i];
        let siblings: Vec<String> = node.parent.map_or_else(Vec::new, |p| {
            walk.children(p)
                .filter(|&c| c != i)
                .map(|c| file_name(&walk.dirs[c].path))
                .collect()
        });
        let summary = summary_of(&node.path, agg, self.hctx.env);
        junk_verdict_with_siblings(&summary, self.hctx.env, &siblings)
    }

    /// Aggregates of the subtree of `i` without the claimed folders in it.
    fn effective(&mut self, i: usize) -> DirAggregate {
        let walk = self.walk;
        let node = &walk.dirs[i];
        if !self.hctx.claimed.has_descendant(&node.path) {
            return node.subtree.clone();
        }
        if let Some(agg) = self.memo.get(&i) {
            return agg.clone();
        }
        let mut agg = node.own.clone();
        for child in walk.children(i) {
            if !self.hctx.claimed.covers(&walk.dirs[child].path) {
                let child_agg = self.effective(child);
                agg.add(&child_agg);
            }
        }
        agg.finish(self.table);
        self.memo.insert(i, agg.clone());
        agg
    }

    fn pick(&mut self, node: usize, agg: DirAggregate, kind: PickKind) {
        self.taken.insert(self.walk.dirs[node].path.clone());
        self.picks.push(Pick { node, agg, kind });
    }
}

/// A drive root (`D:\`, `\\?\D:\`, `/`): a path without a named component.
fn is_drive_root(path: &Path) -> bool {
    !path.components().any(|c| matches!(c, Component::Normal(_)))
}

/// Step 6: keeps the `max` heaviest clusters (junk ones included); the
/// standard user folders are always kept. Returns the kept picks in walk
/// order and the number of dropped ones.
fn limit(picks: Vec<Pick>, max: usize) -> (Vec<Pick>, usize) {
    let (mut kept, mut clusters): (Vec<Pick>, Vec<Pick>) = picks
        .into_iter()
        .partition(|p| p.kind == PickKind::UserFolder);
    clusters.sort_by(|a, b| {
        b.agg
            .user_weight
            .total_cmp(&a.agg.user_weight)
            .then(a.node.cmp(&b.node))
    });
    let dropped = clusters.len().saturating_sub(max);
    clusters.truncate(max);
    kept.extend(clusters);
    kept.sort_by_key(|p| p.node);
    (kept, dropped)
}

/// The issue `heuristic.usr_truncated {count, limit}`.
fn truncated_issue(dropped: usize, max: usize) -> ScanIssue {
    ScanIssue {
        severity: IssueSeverity::Warning,
        source: COLLECTOR_ID.to_owned(),
        path: None,
        message_key: ISSUE_TRUNCATED.to_owned(),
        message_args: BTreeMap::from([
            ("count".to_owned(), dropped.to_string()),
            ("limit".to_owned(), max.to_string()),
        ]),
    }
}

/// The findings of the kept picks, in walk order, then those of the
/// valuable files outside them (step 5), by path. A valuable file inside a
/// junk cluster gets its own finding and is excluded from the cluster.
fn build(
    hctx: &HeuristicContext<'_>,
    walk: &RootsWalk,
    table: Option<&ExtensionTable>,
    kept: &[Pick],
    out: &mut HeuristicOutput,
) {
    let env = hctx.env;
    let covered: PathSet = kept
        .iter()
        .filter(|p| !matches!(p.kind, PickKind::Junk(_)))
        .map(|p| walk.dirs[p.node].path.clone())
        .collect();
    let files: Vec<_> = walk
        .valuable_files
        .iter()
        .filter(|f| !hctx.claimed.covers(&f.path) && !covered.covers(&f.path))
        .collect();
    for pick in kept {
        let path = walk.dirs[pick.node].path.as_path();
        let mut exclude: Vec<String> = topmost(hctx.claimed.descendants(path))
            .iter()
            .map(|p| relative_glob(p, path))
            .collect();
        let verdict = match &pick.kind {
            PickKind::Cluster => cluster_verdict(&pick.agg, table, REASON_CLUSTER),
            PickKind::UserFolder => cluster_verdict(&pick.agg, table, REASON_USER_FOLDER),
            PickKind::Junk(verdict) => {
                let inside = files.iter().filter(|f| starts_with_ci(&f.path, path));
                exclude.extend(inside.map(|f| relative_glob(&f.path, path)));
                verdict.clone()
            }
        };
        let junk = matches!(pick.kind, PickKind::Junk(_));
        let finding = folder_finding(path, &pick.agg, verdict, exclude, junk, env, table);
        out.findings.push(finding);
        out.claimed.push(path.to_path_buf());
    }
    for file in files {
        out.findings.push(file_finding(file, env, table));
        out.claimed.push(file.path.clone());
    }
}
