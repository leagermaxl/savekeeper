//! Category display order of the result (SPEC-09 §4.8).

use std::cmp::Ordering;

use sk_core::model::{Category, Finding};

/// Rank of a category in the result order of SPEC-09 §4.8: Credentials,
/// GameSave, UserFiles, DevEnvironment, AppData, AppConfig, GameConfig,
/// SystemSettings, Unknown, Reinstallable, Cache. Lower comes first.
///
/// This differs from the declaration order of [`Category`].
pub(crate) fn category_rank(category: Category) -> u8 {
    match category {
        Category::Credentials => 0,
        Category::GameSave => 1,
        Category::UserFiles => 2,
        Category::DevEnvironment => 3,
        Category::AppData => 4,
        Category::AppConfig => 5,
        Category::GameConfig => 6,
        Category::SystemSettings => 7,
        Category::Unknown => 8,
        Category::Reinstallable => 9,
        Category::Cache => 10,
    }
}

/// Score value used for ordering: `f.score.value`, 0 without a score or when
/// it is not finite.
fn order_value(f: &Finding) -> f32 {
    f.score
        .as_ref()
        .map(|s| s.value)
        .filter(|v| v.is_finite())
        .unwrap_or(0.0)
}

/// Descending wrapper of an `f32` with a total order.
#[derive(PartialEq)]
struct Desc(f32);

impl Eq for Desc {}

impl PartialOrd for Desc {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Desc {
    fn cmp(&self, other: &Self) -> Ordering {
        other.0.total_cmp(&self.0)
    }
}

/// Sorts findings in the result order of SPEC-09 §4.8: category rank
/// ([`category_rank`]), then `score.value` descending (a missing or
/// non-finite score counts as 0), then `title` case-insensitively, then `id`.
///
/// Nested findings are sorted the same way; the UI groups them under their
/// parent through `children`.
#[cfg_attr(not(test), allow(dead_code))] // called by run() (T-09-08)
pub(crate) fn sort_findings(findings: &mut [Finding]) {
    findings.sort_by_cached_key(|f| {
        (
            category_rank(f.category),
            Desc(order_value(f)),
            f.title.to_lowercase(),
            f.id.clone(),
        )
    });
}
