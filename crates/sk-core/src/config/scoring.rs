//! `scoring` section; fields and defaults are normative in SPEC-09 §4.1, §4.6.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};
use specta::Type;

use crate::model::Category;

const MIB: u64 = 1024 * 1024;

/// `scoring` section. Values are validated by `sk-score` (SPEC-09 §4.10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "snake_case")]
pub struct ScoringConfig {
    /// Weights per category; always the full table (missing entries and fields
    /// come from SPEC-09 §4.6).
    #[serde(deserialize_with = "deserialize_weights")]
    pub category_weights: BTreeMap<Category, CategoryWeights>,
    /// Score from which a finding is selected by default.
    pub select_threshold: f32,
    /// The same for `Unknown`.
    pub unknown_select_threshold: f32,
    /// `Unknown` findings larger than this are not selected by default.
    pub unknown_max_default_bytes: u64,
    /// Findings larger than this are not selected by default.
    pub max_default_item_bytes: u64,
    /// No size penalty up to this size.
    pub size_free_bytes: u64,
    /// Maximum size penalty.
    pub size_penalty_cap: f32,
    /// Penalty for folders synced to a cloud.
    pub cloud_synced_penalty: f32,
    /// Bonus when the application is installed.
    pub installed_bonus: f32,
    /// Penalty when the application is not installed.
    pub uninstalled_penalty: f32,
}

impl Default for ScoringConfig {
    fn default() -> Self {
        Self {
            category_weights: default_weights(),
            select_threshold: 0.40,
            unknown_select_threshold: 0.30,
            unknown_max_default_bytes: 200 * MIB,
            max_default_item_bytes: 2 * 1024 * MIB,
            size_free_bytes: 100 * MIB,
            size_penalty_cap: 0.40,
            cloud_synced_penalty: 0.15,
            installed_bonus: 0.05,
            uninstalled_penalty: 0.10,
        }
    }
}

/// Weights of one category (SPEC-09 §4.6).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub struct CategoryWeights {
    /// How hard the data is to restore.
    pub irreplaceability: f32,
    /// Default share created by the user.
    pub authored: f32,
    /// Halved size penalty.
    pub size_tolerant: bool,
}

/// The table of SPEC-09 §4.6.
pub fn default_weights() -> BTreeMap<Category, CategoryWeights> {
    let w = |irreplaceability, authored, size_tolerant| CategoryWeights {
        irreplaceability,
        authored,
        size_tolerant,
    };
    BTreeMap::from([
        (Category::Credentials, w(1.00, 1.00, false)),
        (Category::GameSave, w(0.95, 0.90, false)),
        (Category::UserFiles, w(0.85, 0.90, true)),
        (Category::DevEnvironment, w(0.80, 0.60, false)),
        (Category::AppData, w(0.70, 0.50, false)),
        (Category::AppConfig, w(0.65, 0.50, false)),
        (Category::SystemSettings, w(0.60, 0.40, false)),
        (Category::GameConfig, w(0.55, 0.40, false)),
        (Category::Unknown, w(0.35, 0.30, false)),
        (Category::Reinstallable, w(0.05, 0.00, true)),
        (Category::Cache, w(0.00, 0.00, true)),
    ])
}

#[derive(Deserialize)]
struct PartialWeights {
    irreplaceability: Option<f32>,
    authored: Option<f32>,
    size_tolerant: Option<bool>,
}

/// A partial table merged over the defaults, by category and by field.
fn deserialize_weights<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<Category, CategoryWeights>, D::Error> {
    let partial = BTreeMap::<Category, PartialWeights>::deserialize(deserializer)?;
    let mut weights = default_weights();
    for (category, p) in partial {
        let entry = weights.entry(category).or_insert(CategoryWeights {
            irreplaceability: 0.0,
            authored: 0.0,
            size_tolerant: false,
        });
        if let Some(v) = p.irreplaceability {
            entry.irreplaceability = v;
        }
        if let Some(v) = p.authored {
            entry.authored = v;
        }
        if let Some(v) = p.size_tolerant {
            entry.size_tolerant = v;
        }
    }
    Ok(weights)
}
