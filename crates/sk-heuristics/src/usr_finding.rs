//! Findings of H-USR (SPEC-07 §4.3.3): clusters, standard user folders and
//! single valuable files, built from the aggregates of the shared walk.

use std::collections::BTreeMap;
use std::path::Path;

use sk_core::env::Environment;
use sk_core::model::{Category, Finding, FolderSummary, Sensitivity, Target};
use sk_core::template::PathTemplate;

use crate::data::{ContentKind, ExtensionGroup, ExtensionTable};
use crate::finding::{new_finding, HUGE_BYTES};
use crate::usr::HEURISTIC_ID;
use crate::verdict::MarkerVerdict;
use crate::walk_tree::{DirAggregate, KindStat, ValuableFile};

/// Reason: a folder where user files gather (§4.3.3).
pub(crate) const REASON_CLUSTER: &str = "heuristic.usr.cluster";
/// Reason: a standard user folder no rule claimed (§4.3.1).
pub(crate) const REASON_USER_FOLDER: &str = "heuristic.usr.user_folder";
/// Reason: a single valuable file outside the clusters (§4.3.3, step 5).
pub(crate) const REASON_VALUABLE_FILE: &str = "heuristic.usr.valuable_file";
/// Tag of a cluster holding files of the `credentials` group.
pub(crate) const TAG_CREDENTIALS_INSIDE: &str = "credentials-inside";
/// Tag of a cluster with files that are only in the cloud (§5).
pub(crate) const TAG_CLOUD_SYNCED: &str = "cloud-synced";
/// Tag of a cluster larger than 50 GiB (§5).
pub(crate) const TAG_HUGE: &str = "huge";
/// Confidence of a cluster and of a standard user folder (as rule 9 of §4.7).
const CLUSTER_CONFIDENCE: f32 = 0.7;
/// Confidence of a single valuable file: its extension says what it is.
const VALUABLE_FILE_CONFIDENCE: f32 = 0.9;
/// Number of groups named in the evidence of a cluster.
const TOP_GROUPS: usize = 3;
/// `user_weight` is measured in MiB.
const MIB: f64 = 1024.0 * 1024.0;

/// The verdict of a cluster: the category of the group with the largest
/// weight (§4.3.3); `UserFiles` when no group weighs more than 0.
pub(crate) fn cluster_verdict(
    agg: &DirAggregate,
    table: Option<&ExtensionTable>,
    reason_key: &'static str,
) -> MarkerVerdict {
    let category = ranked(agg, table)
        .first()
        .map_or(Category::UserFiles, |(_, g)| g.category);
    MarkerVerdict::whole(category, CLUSTER_CONFIDENCE, reason_key)
}

/// The finding of a picked folder: a `FileSet` of the whole folder without
/// `exclude`. The evidence names the number of files, their size and the
/// three heaviest groups (`group1`, `group1_files` …); a cluster with
/// credentials (not a `junk` one, whose valuable files have own findings) is
/// `High` and tagged `credentials-inside`.
pub(crate) fn folder_finding(
    path: &Path,
    agg: &DirAggregate,
    mut verdict: MarkerVerdict,
    exclude: Vec<String>,
    junk: bool,
    env: &Environment,
    table: Option<&ExtensionTable>,
) -> Finding {
    verdict.exclude.extend(exclude);
    let target = Target::FileSet {
        root: PathTemplate::from_path(path, env),
        resolved: path.to_path_buf(),
        include: verdict.include.clone(),
        exclude: verdict.exclude.clone(),
    };
    let credentials = !junk
        && agg
            .kinds
            .iter()
            .any(|k| k.kind == ContentKind::Credentials && k.files > 0);
    let mut tags = Vec::new();
    if credentials {
        tags.push(TAG_CREDENTIALS_INSIDE.to_owned());
    }
    if agg.cloud_only_files > 0 {
        tags.push(TAG_CLOUD_SYNCED.to_owned());
    }
    if agg.bytes > HUGE_BYTES {
        tags.push(TAG_HUGE.to_owned());
    }
    let args = cluster_args(agg, table);
    let mut finding = new_finding(target, path, &verdict, args, HEURISTIC_ID, env, tags);
    // A folder of user files is not an application.
    finding.app = None;
    if credentials {
        finding.sensitivity = Sensitivity::High;
    }
    finding
}

/// The finding of a single valuable file: a `File` with the category of its
/// group (`Credentials` → sensitivity `High`).
pub(crate) fn file_finding(
    file: &ValuableFile,
    env: &Environment,
    table: Option<&ExtensionTable>,
) -> Finding {
    let category = group(table, file.kind).map_or(Category::UserFiles, |g| g.category);
    let verdict = MarkerVerdict::whole(category, VALUABLE_FILE_CONFIDENCE, REASON_VALUABLE_FILE);
    let target = Target::File {
        path: PathTemplate::from_path(&file.path, env),
        resolved: file.path.clone(),
    };
    let args = BTreeMap::from([
        ("bytes".to_owned(), file.size.to_string()),
        ("group".to_owned(), file.kind.as_str().to_owned()),
    ]);
    let mut finding = new_finding(
        target,
        &file.path,
        &verdict,
        args,
        HEURISTIC_ID,
        env,
        Vec::new(),
    );
    finding.app = None;
    finding
}

/// A summary of a folder made from its aggregates, for `junk_verdict` (§4.5):
/// path, size, number of files and newest time; no markers.
pub(crate) fn summary_of(path: &Path, agg: &DirAggregate, env: &Environment) -> FolderSummary {
    FolderSummary {
        path: PathTemplate::from_path(path, env),
        total_bytes: agg.bytes,
        file_count: agg.files,
        dir_count: 0,
        max_depth: 0,
        newest_mtime: agg.newest_mtime,
        oldest_mtime: None,
        ext_histogram: Vec::new(),
        sample_names: Vec::new(),
        top_children: Vec::new(),
        markers: Vec::new(),
        truncated: false,
    }
}

/// `user_weight` of the groups with a positive weight only: no part of the
/// folder weighs more.
pub(crate) fn positive_weight(agg: &DirAggregate, table: Option<&ExtensionTable>) -> f64 {
    ranked(agg, table)
        .iter()
        .map(|(k, g)| contribution(k, g))
        .sum::<f64>()
        / MIB
}

/// Arguments of a folder's evidence: `files`, `bytes`, the three heaviest
/// groups as `groupN` (kind, as in `extensions.yaml`) and `groupN_files`, and
/// `cloud_only_files` when some files are only in the cloud.
fn cluster_args(agg: &DirAggregate, table: Option<&ExtensionTable>) -> BTreeMap<String, String> {
    let mut args = BTreeMap::from([
        ("files".to_owned(), agg.files.to_string()),
        ("bytes".to_owned(), agg.bytes.to_string()),
    ]);
    for (n, (kind, _)) in (1..).zip(ranked(agg, table).iter().take(TOP_GROUPS)) {
        args.insert(format!("group{n}"), kind.kind.as_str().to_owned());
        args.insert(format!("group{n}_files"), kind.files.to_string());
    }
    if agg.cloud_only_files > 0 {
        args.insert(
            "cloud_only_files".to_owned(),
            agg.cloud_only_files.to_string(),
        );
    }
    args
}

/// The groups of the folder with a positive weight, heaviest first (weight ×
/// capped bytes), then by number of files, then in the order of §4.3.2.
fn ranked<'a, 't>(
    agg: &'a DirAggregate,
    table: Option<&'t ExtensionTable>,
) -> Vec<(&'a KindStat, &'t ExtensionGroup)> {
    let mut groups: Vec<_> = agg
        .kinds
        .iter()
        .filter(|k| k.files > 0)
        .filter_map(|k| group(table, k.kind).map(|g| (k, g)))
        .filter(|(_, g)| g.weight > 0.0)
        .collect();
    groups.sort_by(|a, b| {
        contribution(b.0, b.1)
            .total_cmp(&contribution(a.0, a.1))
            .then(b.0.files.cmp(&a.0.files))
    });
    groups
}

/// Weight × capped bytes of a group in a folder.
fn contribution(kind: &KindStat, group: &ExtensionGroup) -> f64 {
    // Sizes far below 2^53 bytes convert exactly enough.
    f64::from(group.weight) * kind.capped_bytes as f64
}

fn group(table: Option<&ExtensionTable>, kind: ContentKind) -> Option<&ExtensionGroup> {
    table?.groups().iter().find(|g| g.kind == kind)
}
