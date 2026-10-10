//! Result of classifying a folder by its summary (SPEC-07 §4.1, §4.5, §4.7).

use sk_core::model::Category;

/// What a folder probably is, decided from its `FolderSummary`.
///
/// Produced by `junk_verdict` (§4.5) and the marker classification (§4.7).
#[derive(Debug, Clone, PartialEq)]
pub struct MarkerVerdict {
    /// Category of the folder.
    pub category: Category,
    /// Confidence in the category, 0..=1.
    pub confidence: f32,
    /// i18n key of the reason: "heuristic.config_like", "heuristic.junk.cache_name" ...
    pub reason_key: &'static str,
    /// Include globs of the finding; empty means the whole folder.
    pub include: Vec<String>,
    /// Exclude globs of the finding.
    pub exclude: Vec<String>,
}

impl MarkerVerdict {
    /// A verdict for the whole folder, without include or exclude globs.
    pub(crate) fn whole(category: Category, confidence: f32, reason_key: &'static str) -> Self {
        Self {
            category,
            confidence,
            reason_key,
            include: Vec::new(),
            exclude: Vec::new(),
        }
    }
}
