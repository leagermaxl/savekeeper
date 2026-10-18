//! The eval set `fixtures/llm-eval/*.json` (SPEC-08 §6.3).
//!
//! Each file is a JSON array of [`EvalCase`]. The set is written by hand from
//! real folder layouts of known programs and games; names, paths and dates
//! are synthetic. Shared with `tests/eval_dataset.rs` via `#[path]`.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sk_core::model::{Category, FolderSummary};
use sk_llm::ClassifyItem;

/// Every category in display order (SPEC-02 §2.3).
pub const CATEGORIES: [Category; 11] = [
    Category::GameSave,
    Category::GameConfig,
    Category::AppConfig,
    Category::AppData,
    Category::UserFiles,
    Category::DevEnvironment,
    Category::Credentials,
    Category::SystemSettings,
    Category::Reinstallable,
    Category::Cache,
    Category::Unknown,
];

/// Minimum number of cases in the set (SPEC-08 §6.3).
pub const MIN_CASES: usize = 150;
/// Minimum number of cases per category, `unknown` excluded (SPEC-08 §6.3).
pub const MIN_PER_CATEGORY: usize = 10;

/// One labelled folder summary.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvalCase {
    /// Stable, descriptive id; never sent to the model (it would leak the label).
    pub id: String,
    /// The correct category.
    pub expected_category: Category,
    /// Other categories that are also acceptable (the expected one is implied).
    #[serde(default)]
    pub acceptable_categories: Vec<Category>,
    /// The application or game, for reading the report.
    #[serde(default)]
    pub expected_app: Option<String>,
    /// Categories that are a serious mistake (e.g. a save classified as `cache`).
    #[serde(default)]
    pub must_not: Vec<Category>,
    /// Why the case is a trap, if it is one.
    #[serde(default)]
    pub trap: Option<String>,
    /// Optional heuristic hint sent with the item.
    #[serde(default)]
    pub hint: Option<String>,
    /// The summary, as `sk-scan` would produce it after redaction.
    pub summary: FolderSummary,
}

impl EvalCase {
    /// `category` is the expected one or one of the acceptable ones.
    pub fn accepts(&self, category: Category) -> bool {
        category == self.expected_category || self.acceptable_categories.contains(&category)
    }
}

/// A case with the file it came from.
#[derive(Debug, Clone)]
pub struct LoadedCase {
    /// File name inside the eval folder.
    pub file: String,
    /// The case.
    pub case: EvalCase,
}

/// The default eval folder: `<workspace>/fixtures/llm-eval`.
pub fn default_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("fixtures")
        .join("llm-eval")
}

/// Loads every `*.json` file of `dir` in name order.
pub fn load_dir(dir: &Path) -> Result<Vec<LoadedCase>, String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut files = Vec::new();
    for entry in entries {
        let path = entry.map_err(|e| format!("{}: {e}", dir.display()))?.path();
        if path.extension().is_some_and(|ext| ext == "json") {
            files.push(path);
        }
    }
    files.sort();
    let mut out = Vec::new();
    for path in files {
        let file = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{file}: {e}"))?;
        let cases: Vec<EvalCase> =
            serde_json::from_str(&text).map_err(|e| format!("{file}: {e}"))?;
        out.extend(cases.into_iter().map(|case| LoadedCase {
            file: file.clone(),
            case,
        }));
    }
    Ok(out)
}

/// Problems that make the set unusable: duplicate ids, the expected category
/// listed as acceptable, `must_not` overlapping the accepted ones.
pub fn problems(cases: &[LoadedCase]) -> Vec<String> {
    let mut out = Vec::new();
    let mut ids = HashSet::new();
    for LoadedCase { file, case } in cases {
        if !ids.insert(case.id.as_str()) {
            out.push(format!("{file}: duplicate id {}", case.id));
        }
        if case.acceptable_categories.contains(&case.expected_category) {
            out.push(format!(
                "{file}: {}: expected category is also acceptable",
                case.id
            ));
        }
        if case.must_not.iter().any(|c| case.accepts(*c)) {
            out.push(format!("{file}: {}: must_not overlaps accepted", case.id));
        }
    }
    out
}

/// Number of cases per expected category.
pub fn per_category(cases: &[LoadedCase]) -> BTreeMap<Category, usize> {
    let mut out: BTreeMap<Category, usize> = CATEGORIES.iter().map(|c| (*c, 0)).collect();
    for c in cases {
        *out.entry(c.case.expected_category).or_default() += 1;
    }
    out
}

/// A deterministic order that mixes categories (sorted by BLAKE3 of the id),
/// so a batch is not made of one category only.
pub fn mixed_order(cases: &[LoadedCase]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..cases.len()).collect();
    order.sort_by_key(|&i| *blake3::hash(cases[i].case.id.as_bytes()).as_bytes());
    order
}

/// The id sent to the model for the case at position `n` of the run:
/// opaque, so it carries no hint about the label.
pub fn wire_id(n: usize) -> String {
    format!("item-{n:03}")
}

/// The item sent to the model.
pub fn to_item(case: &EvalCase, wire_id: String) -> ClassifyItem {
    ClassifyItem {
        id: wire_id,
        summary: case.summary.clone(),
        peek: Vec::new(),
        hint: case.hint.clone(),
    }
}

/// `snake_case` name of a category, as in the response schema (§4.5).
pub fn category_name(category: Category) -> String {
    match serde_json::to_value(category) {
        Ok(serde_json::Value::String(s)) => s,
        _ => format!("{category:?}"),
    }
}
