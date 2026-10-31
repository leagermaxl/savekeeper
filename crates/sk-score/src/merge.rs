//! Merging findings with the same `FindingId` (SPEC-09 §4.3).

use std::cmp::Ordering;
use std::collections::btree_map::Entry;
use std::collections::BTreeMap;

use sk_core::model::{Category, Evidence, EvidenceSource, Finding, FindingId, ScanIssue, Target};

use crate::authority::{authority, evidence_authority};
use crate::config::category_name;
use crate::issues::{compare_issues, target_path, warning, CATEGORY_CONFLICT_KEY};
use crate::order::category_rank;
use crate::sanitize::sanitize;

/// Tag of a finding whose merged inputs disagreed dangerously on the category.
pub const CONFLICT_TAG: &str = "conflict";

/// Minimum confidence of a losing important category that makes a conflict
/// dangerous (SPEC-09 §4.3, rule 8).
const CONFLICT_MIN_CONFIDENCE: f32 = 0.7;

/// Merges findings with the same `id` into one (SPEC-09 §4.3, FR-09-01).
///
/// The winner is the finding with the highest [`authority`]; ties go to the
/// category that comes first in the §4.8 order, then to the smaller `title`,
/// then to the smaller `Debug` rendering of the whole finding, so the result
/// does not depend on the input order (NFR-09-02). Category, app, title and
/// target come from the winner; evidence, tags and children are united;
/// sensitivity is the maximum and `requires_elevation` is an OR.
///
/// A dangerous category conflict adds the tag `conflict`. Its
/// `scoring.category_conflict` issue, like the `scoring.no_evidence` and
/// `scoring.bad_number` issues of the input checks (§5), is reported by
/// `run()`; this function drops issues. The output is ordered by id.
pub fn merge_by_id(findings: Vec<Finding>) -> Vec<Finding> {
    let mut issues = Vec::new();
    merge_by_id_reporting(findings, &mut issues)
}

/// [`merge_by_id`] that appends its issues to `issues`, sorted.
///
/// Runs the input checks of SPEC-09 §5 first: a finding without evidence trips
/// a `debug_assert!` and in release gets the synthetic fallback evidence; NaN
/// and infinite confidence or importance become 0.
pub(crate) fn merge_by_id_reporting(
    mut findings: Vec<Finding>,
    issues: &mut Vec<ScanIssue>,
) -> Vec<Finding> {
    let start = issues.len();
    sanitize(&mut findings, issues);

    let mut groups: BTreeMap<FindingId, Vec<Finding>> = BTreeMap::new();
    for finding in findings {
        groups.entry(finding.id.clone()).or_default().push(finding);
    }
    let merged = groups
        .into_values()
        .filter_map(|group| merge_group(group, issues))
        .collect();

    issues[start..].sort_by(compare_issues);
    merged
}

/// Merges one non-empty group of findings sharing an id.
fn merge_group(mut group: Vec<Finding>, issues: &mut Vec<ScanIssue>) -> Option<Finding> {
    group.sort_by(compare_members);
    let mut members = group.into_iter();
    let mut winner = members.next()?;
    let losers: Vec<Finding> = members.collect();

    if let Some(loser) = dangerous_loser(&winner, &losers) {
        winner.tags.push(CONFLICT_TAG.to_owned());
        issues.push(warning(
            CATEGORY_CONFLICT_KEY,
            target_path(&winner.target),
            &[
                ("finding_id", winner.id.as_str().to_owned()),
                ("winner", category_name(winner.category).to_owned()),
                ("loser", category_name(loser).to_owned()),
            ],
        ));
    }
    if losers.is_empty() {
        normalize(&mut winner);
        return Some(winner);
    }

    if winner.app.is_none() {
        winner.app = losers.iter().find_map(|l| l.app.clone());
    }
    if winner.stats.is_none() {
        winner.stats = losers.iter().find_map(|l| l.stats.clone());
    }
    if winner.notes_key.is_none() {
        winner.notes_key = losers.iter().find_map(|l| l.notes_key.clone());
    }
    merge_globs(&mut winner.target, &losers);
    for loser in losers {
        winner.evidence.extend(loser.evidence);
        winner.tags.extend(loser.tags);
        winner.children.extend(loser.children);
        winner.sensitivity = winner.sensitivity.max(loser.sensitivity);
        winner.requires_elevation |= loser.requires_elevation;
    }
    normalize(&mut winner);
    Some(winner)
}

/// Dedups and sorts evidence, tags, children and globs of a merged finding.
fn normalize(finding: &mut Finding) {
    finding.evidence = merge_evidence(std::mem::take(&mut finding.evidence));
    finding.tags.sort();
    finding.tags.dedup();
    finding.children.sort();
    finding.children.dedup();
    if let Target::FileSet {
        include, exclude, ..
    } = &mut finding.target
    {
        include.sort();
        include.dedup();
        exclude.sort();
        exclude.dedup();
    }
}

/// Group order: the winner first, then the losers by descending authority.
fn compare_members(a: &Finding, b: &Finding) -> Ordering {
    authority(b)
        .cmp(&authority(a))
        .then_with(|| category_rank(a.category).cmp(&category_rank(b.category)))
        .then_with(|| a.title.cmp(&b.title))
        .then_with(|| format!("{a:?}").cmp(&format!("{b:?}")))
}

/// The most authoritative losing category that makes the conflict dangerous:
/// the winner is `Cache`/`Reinstallable` and a loser with confidence ≥ 0.7 is
/// `GameSave`/`UserFiles`/`Credentials`/`DevEnvironment` (§4.3, rule 8).
fn dangerous_loser(winner: &Finding, losers: &[Finding]) -> Option<Category> {
    if !matches!(winner.category, Category::Cache | Category::Reinstallable) {
        return None;
    }
    losers
        .iter()
        .find(|l| {
            matches!(
                l.category,
                Category::GameSave
                    | Category::UserFiles
                    | Category::Credentials
                    | Category::DevEnvironment
            ) && authority(l).confidence >= CONFLICT_MIN_CONFIDENCE
        })
        .map(|l| l.category)
}

/// Rule 7: `include` is the union if every `FileSet` has a non-empty one and
/// empty (everything) otherwise; `exclude` is the intersection.
fn merge_globs(target: &mut Target, losers: &[Finding]) {
    let Target::FileSet {
        include, exclude, ..
    } = target
    else {
        return;
    };
    let others: Vec<(&Vec<String>, &Vec<String>)> = losers
        .iter()
        .filter_map(|l| match &l.target {
            Target::FileSet {
                include, exclude, ..
            } => Some((include, exclude)),
            _ => None,
        })
        .collect();
    if !include.is_empty() && others.iter().all(|(inc, _)| !inc.is_empty()) {
        for (inc, _) in &others {
            include.extend(inc.iter().cloned());
        }
    } else {
        include.clear();
    }
    exclude.retain(|glob| others.iter().all(|(_, exc)| exc.contains(glob)));
}

/// Owned sort key of an evidence source: variant, then its fields.
pub(crate) type SourceKey = (u8, String, String);

pub(crate) fn source_key(source: &EvidenceSource) -> SourceKey {
    let (variant, a, b) = match source {
        EvidenceSource::User => (0, "", ""),
        EvidenceSource::Rule { rule_id } => (1, rule_id.as_str(), ""),
        EvidenceSource::Ludusavi {
            game,
            manifest_version,
        } => (2, game.as_str(), manifest_version.as_str()),
        EvidenceSource::Launcher { launcher } => (3, launcher.as_str(), ""),
        EvidenceSource::System { exporter_id } => (4, exporter_id.as_str(), ""),
        EvidenceSource::Heuristic { heuristic_id } => (5, heuristic_id.as_str(), ""),
        EvidenceSource::Llm { provider, model } => (6, provider.as_str(), model.as_str()),
    };
    (variant, a.to_owned(), b.to_owned())
}

/// Rule 3: dedup by `(source, message_key)` keeping the higher confidence,
/// then sort by descending authority.
pub(crate) fn merge_evidence(all: Vec<Evidence>) -> Vec<Evidence> {
    let mut best: BTreeMap<(SourceKey, String), Evidence> = BTreeMap::new();
    for evidence in all {
        match best.entry((source_key(&evidence.source), evidence.message_key.clone())) {
            Entry::Vacant(slot) => {
                slot.insert(evidence);
            }
            Entry::Occupied(mut slot) => {
                let current = slot.get();
                let better = evidence
                    .confidence
                    .total_cmp(&current.confidence)
                    .then_with(|| compare_evidence(current, &evidence));
                if better == Ordering::Greater {
                    slot.insert(evidence);
                }
            }
        }
    }
    let mut out: Vec<Evidence> = best.into_values().collect();
    out.sort_by(compare_evidence);
    out
}

/// Evidence order: descending authority, then a total order on the rest.
fn compare_evidence(a: &Evidence, b: &Evidence) -> Ordering {
    evidence_authority(b)
        .cmp(&evidence_authority(a))
        .then_with(|| source_key(&a.source).cmp(&source_key(&b.source)))
        .then_with(|| a.message_key.cmp(&b.message_key))
        .then_with(|| a.message_args.cmp(&b.message_args))
        .then_with(|| compare_importance(a.importance, b.importance))
        .then_with(|| a.confidence.total_cmp(&b.confidence))
}

fn compare_importance(a: Option<f32>, b: Option<f32>) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => a.total_cmp(&b),
        (a, b) => a.is_some().cmp(&b.is_some()),
    }
}
