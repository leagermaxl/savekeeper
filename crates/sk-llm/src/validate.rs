//! Validation and normalization of the model response (SPEC-08 §4.6, steps 1–7).
//!
//! Retrying a batch with invalid JSON (step 1) or with missing ids (step 2)
//! is the job of the runner: this module only reports what failed.

use std::collections::HashSet;

use serde::Deserialize;
use sk_core::model::Category;

use crate::{Classification, ClassifyItem, LlmError};

/// Maximum length of [`Classification::reason`], in characters.
pub const MAX_REASON_CHARS: usize = 200;
/// Maximum length of [`Classification::app_guess`], in characters.
pub const MAX_APP_GUESS_CHARS: usize = 80;
/// Maximum number of globs of each kind (include, exclude).
pub const MAX_GLOBS: usize = 20;
/// `reason` used when the model gave an empty one.
pub const EMPTY_REASON: &str = "—";

/// A model response matched against the batch it answers.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ParsedResponse {
    /// Normalized results, one per known id, in the order of the response.
    pub results: Vec<Classification>,
    /// Ids of the batch the response has no (usable) result for, in batch order.
    pub missing: Vec<String>,
}

/// The response object as the schema (§4.5) describes it.
#[derive(Deserialize)]
struct RawResponse {
    results: Vec<serde_json::Value>,
}

/// One result before validation. Fields are lenient: a wrong type or a
/// missing value is normalized instead of rejecting the whole response.
#[derive(Deserialize)]
struct RawResult {
    id: String,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    app_guess: Option<String>,
    #[serde(default)]
    importance: Option<f64>,
    #[serde(default)]
    confidence: Option<f64>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    suggested_include: Vec<String>,
    #[serde(default)]
    suggested_exclude: Vec<String>,
}

/// Parses the JSON text of a model answer for `items` (§4.6).
///
/// Invalid JSON, or JSON without a `results` array, is
/// [`LlmError::BadResponse`] (step 1). Results for ids not in `items` and
/// repeated ids are ignored; a result that cannot be read (no string `id`)
/// counts as absent, and the ids without a result are listed in
/// [`ParsedResponse::missing`] (step 2). Every result is normalized
/// (steps 3–7) with `from_cache = false`.
pub fn parse_response(json: &str, items: &[ClassifyItem]) -> Result<ParsedResponse, LlmError> {
    let raw: RawResponse = serde_json::from_str(json.trim())
        .map_err(|e| LlmError::BadResponse(format!("invalid response JSON: {e}")))?;
    let wanted: HashSet<&str> = items.iter().map(|i| i.id.as_str()).collect();
    let mut seen: HashSet<String> = HashSet::new();
    let mut results = Vec::new();
    for value in raw.results {
        let Ok(r) = serde_json::from_value::<RawResult>(value) else {
            continue;
        };
        if !wanted.contains(r.id.as_str()) || seen.contains(&r.id) {
            continue;
        }
        seen.insert(r.id.clone());
        results.push(normalize(r));
    }
    let missing = items
        .iter()
        .filter(|i| !seen.contains(&i.id))
        .map(|i| i.id.clone())
        .collect();
    Ok(ParsedResponse { results, missing })
}

fn normalize(r: RawResult) -> Classification {
    Classification {
        id: r.id,
        category: r.category.as_deref().map_or(Category::Unknown, category),
        app_guess: r.app_guess.as_deref().and_then(app_guess),
        importance: unit(r.importance.unwrap_or(0.0)),
        confidence: unit(r.confidence.unwrap_or(0.0)),
        reason: reason(r.reason.as_deref().unwrap_or("")),
        suggested_include: globs(&r.suggested_include),
        suggested_exclude: globs(&r.suggested_exclude),
        from_cache: false,
    }
}

/// Step 3: a `snake_case` category name; anything else is `Unknown`.
pub fn category(name: &str) -> Category {
    serde_json::from_value(serde_json::Value::String(name.trim().to_owned()))
        .unwrap_or(Category::Unknown)
}

/// Step 4: NaN becomes 0, everything else is clamped to `[0, 1]`.
pub fn unit(value: f64) -> f32 {
    if value.is_nan() {
        0.0
    } else {
        // The clamp keeps the value inside the `f32` range, so the cast is exact enough.
        value.clamp(0.0, 1.0) as f32
    }
}

/// Step 5: trimmed, without control characters, at most
/// [`MAX_REASON_CHARS`] characters, never empty ([`EMPTY_REASON`]).
pub fn reason(text: &str) -> String {
    let cleaned = clean(text, MAX_REASON_CHARS);
    if cleaned.is_empty() {
        EMPTY_REASON.to_owned()
    } else {
        cleaned
    }
}

/// Step 6: trimmed, without control characters, at most
/// [`MAX_APP_GUESS_CHARS`] characters; an empty guess is `None`.
pub fn app_guess(text: &str) -> Option<String> {
    let cleaned = clean(text, MAX_APP_GUESS_CHARS);
    (!cleaned.is_empty()).then_some(cleaned)
}

/// Removes control characters (whitespace ones become a space), trims and
/// cuts to `max_chars` characters on a `char` boundary.
fn clean(text: &str, max_chars: usize) -> String {
    let no_controls: String = text
        .chars()
        .filter_map(|c| match c {
            c if c.is_control() && c.is_whitespace() => Some(' '),
            c if c.is_control() => None,
            c => Some(c),
        })
        .collect();
    let cut: String = no_controls.trim().chars().take(max_chars).collect();
    cut.trim_end().to_owned()
}

/// Step 7: keeps the globs `globset` accepts that are relative to the
/// folder: not absolute, not starting with a drive, without `..`. Trimmed,
/// without duplicates, at most [`MAX_GLOBS`].
pub fn globs(raw: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for g in raw {
        let g = g.trim();
        if out.len() >= MAX_GLOBS {
            break;
        }
        if is_valid_glob(g) && !out.iter().any(|o| o == g) {
            out.push(g.to_owned());
        }
    }
    out
}

fn is_valid_glob(g: &str) -> bool {
    if g.is_empty() || g.contains("..") || g.starts_with(['/', '\\']) || has_drive(g) {
        return false;
    }
    globset::Glob::new(g).is_ok()
}

/// `C:`, `c:foo` ...: an ASCII letter followed by a colon.
fn has_drive(g: &str) -> bool {
    let b = g.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

#[cfg(test)]
#[path = "validate_tests.rs"]
mod tests;
