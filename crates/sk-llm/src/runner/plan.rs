//! Planning a run (SPEC-08 §4.12, FR-08-06): dedupe by cache key, selection
//! of what is sent, deterministic order and batches.

use std::collections::HashMap;

use sk_core::model::FolderSummary;
use time::OffsetDateTime;

use crate::request::item_for_prompt;
use crate::{cache_key, ClassifierInfo, ClassifyItem};

/// Upper bound of the input of one batch: the summed length, in characters,
/// of the items as they are sent ([`item_for_prompt`](crate::item_for_prompt)).
/// A single larger item still goes alone.
pub const MAX_BATCH_INPUT_CHARS: usize = 12_000;

/// Size above which a folder gets no higher selection priority (FR-08-06):
/// huge folders are almost always caches or games.
pub const MAX_PRIORITY_BYTES: u64 = 50 * 1024 * 1024 * 1024;

/// Age in days at which the recency factor of the selection priority halves.
pub const RECENCY_HALF_LIFE_DAYS: f64 = 365.0;

/// Items with the same cache key: one request element, the answer is copied
/// to every id.
#[derive(Debug, Clone)]
pub(crate) struct Group {
    pub key: String,
    /// The item that is sent (the first one with this key).
    pub item: ClassifyItem,
    /// Ids of all items with this key, the sent one first, no duplicates.
    pub ids: Vec<String>,
    /// Characters of the sent JSON of `item`.
    pub chars: usize,
}

/// Groups `items` by cache key, in the order of their first occurrence.
pub(crate) fn group(items: Vec<ClassifyItem>, info: &ClassifierInfo) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for item in items {
        let key = cache_key(&item, info.provider, &info.model);
        match index.get(&key) {
            Some(&i) => {
                let ids = &mut groups[i].ids;
                if !ids.contains(&item.id) {
                    ids.push(item.id);
                }
            }
            None => {
                index.insert(key.clone(), groups.len());
                let chars = item_for_prompt(&item).to_string().chars().count();
                groups.push(Group {
                    key,
                    ids: vec![item.id.clone()],
                    item,
                    chars,
                });
            }
        }
    }
    groups
}

/// Selection priority of a folder (FR-08-06):
/// `min(total_bytes, MAX_PRIORITY_BYTES) × recency`, where
/// `recency = 0.5 ^ (age_days / RECENCY_HALF_LIFE_DAYS)` by `newest_mtime`
/// (a future time counts as age 0) and 0 without `newest_mtime`.
pub fn selection_priority(summary: &FolderSummary, now: OffsetDateTime) -> f64 {
    let Some(newest) = summary.newest_mtime else {
        return 0.0;
    };
    let age_days = ((now - newest).as_seconds_f64() / 86_400.0).max(0.0);
    let recency = 0.5_f64.powf(age_days / RECENCY_HALF_LIFE_DAYS);
    #[allow(clippy::cast_precision_loss)] // byte counts: relative order is what matters
    let bytes = summary.total_bytes.min(MAX_PRIORITY_BYTES) as f64;
    bytes * recency
}

/// Keeps the `limit` groups with the highest [`selection_priority`] (ties:
/// path, then key); returns the kept and the dropped groups.
pub(crate) fn select(
    mut groups: Vec<Group>,
    limit: usize,
    now: OffsetDateTime,
) -> (Vec<Group>, Vec<Group>) {
    if groups.len() <= limit {
        return (groups, Vec::new());
    }
    let mut scored: Vec<(f64, Group)> = groups
        .drain(..)
        .map(|g| (selection_priority(&g.item.summary, now), g))
        .collect();
    scored.sort_by(|(pa, a), (pb, b)| {
        pb.total_cmp(pa)
            .then_with(|| a.item.summary.path.cmp(&b.item.summary.path))
            .then_with(|| a.key.cmp(&b.key))
    });
    let dropped = scored.split_off(limit);
    (
        scored.into_iter().map(|(_, g)| g).collect(),
        dropped.into_iter().map(|(_, g)| g).collect(),
    )
}

/// Sorts by `summary.path` (then key) for a deterministic request order.
pub(crate) fn sort(groups: &mut [Group]) {
    groups.sort_by(|a, b| {
        a.item
            .summary
            .path
            .cmp(&b.item.summary.path)
            .then_with(|| a.key.cmp(&b.key))
    });
}

/// Splits `groups` (in order) into batches of at most `max_batch` groups and
/// at most [`MAX_BATCH_INPUT_CHARS`] characters of input.
pub(crate) fn batches(groups: Vec<Group>, max_batch: usize) -> Vec<Vec<Group>> {
    let max_batch = max_batch.max(1);
    let mut out: Vec<Vec<Group>> = Vec::new();
    let mut current: Vec<Group> = Vec::new();
    let mut chars = 0usize;
    for g in groups {
        let full = current.len() >= max_batch
            || (!current.is_empty() && chars + g.chars > MAX_BATCH_INPUT_CHARS);
        if full {
            out.push(std::mem::take(&mut current));
            chars = 0;
        }
        chars += g.chars;
        current.push(g);
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

#[cfg(test)]
#[path = "plan_tests.rs"]
mod tests;
