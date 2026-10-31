//! Totals over findings (SPEC-09 §4.9).

use sk_core::model::{Category, CategoryTotals, Finding, Sensitivity, Totals};

use crate::nested::NESTED_TAG;
use crate::select::{ALTERNATIVE_TAG, TOO_LARGE_DEFAULT_TAG};

/// Counts findings, their size and the selected part, per category and over
/// all findings (SPEC-09 §4.9).
///
/// - Findings tagged `nested` or `alternative` are skipped, so a nested
///   finding is not counted again on top of its parent.
/// - `bytes` is `stats.total_bytes`, or 0 without `stats`.
/// - `selected` decides which findings count as selected; the UI passes the
///   user's manual selection, the Score phase `default_selected`.
/// - `sensitive_selected` counts selected `Credentials` or
///   `Sensitivity::High` findings (as the `scoring.sensitive_selected` issue,
///   §4.7); `unknown_count` counts `Unknown` findings;
///   `needs_elevation_count` counts findings with `requires_elevation`;
///   `too_large_count` counts findings tagged `too_large_default`.
/// - `by_category` holds only categories with at least one counted finding,
///   so an empty input gives `Totals::default()`.
///
/// Counters saturate instead of overflowing.
pub fn totals(findings: &[Finding], selected: impl Fn(&Finding) -> bool) -> Totals {
    let mut out = Totals::default();
    for f in findings {
        if f.tags
            .iter()
            .any(|t| t == NESTED_TAG || t == ALTERNATIVE_TAG)
        {
            continue;
        }
        let bytes = f.stats.as_ref().map_or(0, |s| s.total_bytes);
        let is_selected = selected(f);
        add(
            out.by_category.entry(f.category).or_default(),
            bytes,
            is_selected,
        );
        add(&mut out.all, bytes, is_selected);

        if is_selected
            && (f.category == Category::Credentials || f.sensitivity == Sensitivity::High)
        {
            out.sensitive_selected = out.sensitive_selected.saturating_add(1);
        }
        if f.category == Category::Unknown {
            out.unknown_count = out.unknown_count.saturating_add(1);
        }
        if f.requires_elevation {
            out.needs_elevation_count = out.needs_elevation_count.saturating_add(1);
        }
        if f.tags.iter().any(|t| t == TOO_LARGE_DEFAULT_TAG) {
            out.too_large_count = out.too_large_count.saturating_add(1);
        }
    }
    out
}

fn add(t: &mut CategoryTotals, bytes: u64, selected: bool) {
    t.count = t.count.saturating_add(1);
    t.bytes = t.bytes.saturating_add(bytes);
    if selected {
        t.selected_count = t.selected_count.saturating_add(1);
        t.selected_bytes = t.selected_bytes.saturating_add(bytes);
    }
}
