//! Metrics of an eval run (SPEC-08 §6.3): strict accuracy, accuracy by
//! `acceptable_categories`, confusion matrix, latency and cost.
//! Shared with `tests/eval_dataset.rs` via `#[path]`.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::Duration;

use sk_core::model::Category;

use crate::dataset::{category_name, LoadedCase, CATEGORIES};

/// What the model answered for one case.
#[derive(Debug, Clone)]
pub enum Outcome {
    /// A validated classification.
    Predicted {
        /// Category after validation (§4.6).
        category: Category,
        /// Model confidence.
        confidence: f32,
        /// Application guess.
        app_guess: Option<String>,
        /// Short reason.
        reason: String,
    },
    /// The batch succeeded but had no result for this id.
    Missing,
    /// The batch failed (after the retry); the error text.
    Failed(String),
}

/// Column of the confusion matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Column {
    /// A predicted category.
    Category(Category),
    /// No result for the id.
    Missing,
    /// Request failed.
    Failed,
}

/// Aggregated results.
#[derive(Debug, Default)]
pub struct Metrics {
    /// Cases evaluated.
    pub total: usize,
    /// Predicted == expected.
    pub strict: usize,
    /// Predicted is expected or acceptable.
    pub acceptable: usize,
    /// Predicted is in `must_not`.
    pub must_not_hits: usize,
    /// Cases marked as traps.
    pub traps: usize,
    /// Traps answered acceptably.
    pub traps_acceptable: usize,
    /// expected -> column -> count.
    pub confusion: BTreeMap<Category, BTreeMap<Column, usize>>,
    /// expected -> (cases, strict, acceptable).
    pub per_category: BTreeMap<Category, (usize, usize, usize)>,
}

impl Metrics {
    /// Computes metrics; `outcomes[i]` belongs to `cases[i]`.
    pub fn compute(cases: &[LoadedCase], outcomes: &[Outcome]) -> Self {
        let mut m = Self::default();
        for (loaded, outcome) in cases.iter().zip(outcomes) {
            let case = &loaded.case;
            let expected = case.expected_category;
            let (column, strict, acceptable) = match outcome {
                Outcome::Predicted { category, .. } => (
                    Column::Category(*category),
                    *category == expected,
                    case.accepts(*category),
                ),
                Outcome::Missing => (Column::Missing, false, false),
                Outcome::Failed(_) => (Column::Failed, false, false),
            };
            m.total += 1;
            m.strict += usize::from(strict);
            m.acceptable += usize::from(acceptable);
            if let Outcome::Predicted { category, .. } = outcome {
                m.must_not_hits += usize::from(case.must_not.contains(category));
            }
            if case.trap.is_some() {
                m.traps += 1;
                m.traps_acceptable += usize::from(acceptable);
            }
            *m.confusion
                .entry(expected)
                .or_default()
                .entry(column)
                .or_default() += 1;
            let row = m.per_category.entry(expected).or_default();
            row.0 += 1;
            row.1 += usize::from(strict);
            row.2 += usize::from(acceptable);
        }
        m
    }

    /// Strict accuracy, 0 for an empty run.
    pub fn strict_accuracy(&self) -> f64 {
        ratio(self.strict, self.total)
    }

    /// Accuracy by `acceptable_categories`, 0 for an empty run.
    pub fn acceptable_accuracy(&self) -> f64 {
        ratio(self.acceptable, self.total)
    }
}

/// `n / d`, 0 when `d = 0`.
pub fn ratio(n: usize, d: usize) -> f64 {
    if d == 0 {
        0.0
    } else {
        n as f64 / d as f64
    }
}

/// Short column label of a category for the matrix.
pub fn short_name(category: Category) -> &'static str {
    match category {
        Category::GameSave => "gs",
        Category::GameConfig => "gc",
        Category::AppConfig => "ac",
        Category::AppData => "ad",
        Category::UserFiles => "uf",
        Category::DevEnvironment => "de",
        Category::Credentials => "cr",
        Category::SystemSettings => "ss",
        Category::Reinstallable => "ri",
        Category::Cache => "ca",
        Category::Unknown => "un",
    }
}

/// Confusion matrix as a Markdown table: rows expected, columns predicted.
pub fn confusion_markdown(m: &Metrics) -> String {
    let mut columns: Vec<Column> = CATEGORIES.iter().map(|c| Column::Category(*c)).collect();
    columns.extend([Column::Missing, Column::Failed]);
    let label = |c: &Column| match c {
        Column::Category(cat) => short_name(*cat).to_owned(),
        Column::Missing => "miss".to_owned(),
        Column::Failed => "err".to_owned(),
    };
    let mut out = String::from("| expected \\ predicted |");
    for c in &columns {
        let _ = write!(out, " {} |", label(c));
    }
    out.push_str(" n | strict | acceptable |\n|---|");
    for _ in &columns {
        out.push_str("---:|");
    }
    out.push_str("---:|---:|---:|\n");
    for cat in CATEGORIES {
        let Some(row) = m.confusion.get(&cat) else {
            continue;
        };
        let _ = write!(out, "| {} ({}) |", category_name(cat), short_name(cat));
        for c in &columns {
            match row.get(c) {
                Some(n) => {
                    let _ = write!(out, " {n} |");
                }
                None => out.push_str(" · |"),
            }
        }
        let (n, strict, acceptable) = m.per_category.get(&cat).copied().unwrap_or_default();
        let _ = writeln!(
            out,
            " {n} | {:.2} | {:.2} |",
            ratio(strict, n),
            ratio(acceptable, n)
        );
    }
    out
}

/// Latency of the successful requests.
#[derive(Debug, Default)]
pub struct Latency {
    /// (duration, items in the batch) of each successful request.
    pub batches: Vec<(Duration, usize)>,
}

impl Latency {
    /// Mean duration of one request.
    pub fn mean_per_batch(&self) -> Duration {
        let n = u32::try_from(self.batches.len()).unwrap_or(u32::MAX);
        if n == 0 {
            return Duration::ZERO;
        }
        self.batches.iter().map(|(d, _)| *d).sum::<Duration>() / n
    }

    /// Mean duration per classified item.
    pub fn mean_per_item(&self) -> Duration {
        let items: usize = self.batches.iter().map(|(_, n)| n).sum();
        let items = u32::try_from(items).unwrap_or(u32::MAX);
        if items == 0 {
            return Duration::ZERO;
        }
        self.batches.iter().map(|(d, _)| *d).sum::<Duration>() / items
    }

    /// Slowest request.
    pub fn max(&self) -> Duration {
        self.batches
            .iter()
            .map(|(d, _)| *d)
            .max()
            .unwrap_or_default()
    }
}
