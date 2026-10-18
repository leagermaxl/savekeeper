//! What is sent to the model: `summary_for_prompt`, the `user` message and
//! the request preview (SPEC-08 §4.3, §4.7, §4.8, §5).

use serde::Serialize;
use serde_json::{Map, Value};
use sk_core::model::FolderSummary;
use time::{OffsetDateTime, Time, UtcOffset};

use crate::{ClassifierInfo, ClassifyItem};

/// Maximum length of one `sample_names` entry (and of a `top_children`
/// name) in the prompt, in characters (§5).
pub const MAX_NAME_CHARS: usize = 120;
/// Maximum length of the compact JSON of one summary in the prompt, in
/// characters (§5).
pub const MAX_SUMMARY_CHARS: usize = 4_000;
/// Number of items shown in [`RequestPreview::sample`] (§4.8).
pub const PREVIEW_SAMPLE_ITEMS: usize = 3;

/// The summary exactly as it is sent and as it enters the cache key (§4.7).
///
/// - `newest_mtime` / `oldest_mtime` are truncated to the day: midnight UTC,
///   RFC 3339 (`"2026-10-18T00:00:00Z"`), so the key does not change with
///   seconds;
/// - every `sample_names` entry and `top_children` name is cut to
///   [`MAX_NAME_CHARS`] characters;
/// - while the compact JSON is longer than [`MAX_SUMMARY_CHARS`], the last
///   `sample_names` entry is dropped, then the last `top_children` entry,
///   then the last `ext_histogram` entry;
/// - fields with empty arrays and `null` values are left out.
///
/// The summary must already be redacted (`sk_core::privacy::redact`); this
/// function does not redact again, [`final_check`](crate::privacy::final_check)
/// is the safety net.
pub fn summary_for_prompt(summary: &FolderSummary) -> Value {
    let mut s = summary.clone();
    s.newest_mtime = s.newest_mtime.and_then(day);
    s.oldest_mtime = s.oldest_mtime.and_then(day);
    for name in &mut s.sample_names {
        cut(name, MAX_NAME_CHARS);
    }
    for child in &mut s.top_children {
        cut(&mut child.name, MAX_NAME_CHARS);
    }
    loop {
        let value = compact(&s);
        if value.to_string().chars().count() <= MAX_SUMMARY_CHARS {
            return value;
        }
        let dropped = s.sample_names.pop().is_some()
            || s.top_children.pop().is_some()
            || s.ext_histogram.pop().is_some();
        if !dropped {
            // Only scalars and the path are left: nothing more to cut.
            return value;
        }
    }
}

/// One element of the `user` message: `{id, summary, peek?, hint?}` (§4.3).
/// `peek` is left out when empty, `hint` when absent.
pub fn item_for_prompt(item: &ClassifyItem) -> Value {
    let mut obj = Map::new();
    obj.insert("id".to_owned(), Value::String(item.id.clone()));
    obj.insert("summary".to_owned(), summary_for_prompt(&item.summary));
    if !item.peek.is_empty() {
        let peek = item
            .peek
            .iter()
            .map(|p| {
                let mut o = Map::new();
                o.insert("file".to_owned(), Value::String(p.file.clone()));
                o.insert("head".to_owned(), Value::String(p.head.clone()));
                Value::Object(o)
            })
            .collect();
        obj.insert("peek".to_owned(), Value::Array(peek));
    }
    if let Some(hint) = &item.hint {
        obj.insert("hint".to_owned(), Value::String(hint.clone()));
    }
    Value::Object(obj)
}

/// The `user` message for one batch: compact `{"items":[…]}` (§4.3).
pub fn user_message(items: &[ClassifyItem]) -> String {
    let items: Vec<Value> = items.iter().map(item_for_prompt).collect();
    let mut obj = Map::new();
    obj.insert("items".to_owned(), Value::Array(items));
    Value::Object(obj).to_string()
}

/// What the user is shown before a cloud classification (§4.8).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct RequestPreview {
    /// [`ClassifierInfo::provider`].
    pub provider: String,
    /// [`ClassifierInfo::model`].
    pub model: String,
    /// [`ClassifierInfo::is_local`].
    pub is_local: bool,
    /// Number of items to classify.
    pub item_count: usize,
    /// The first [`PREVIEW_SAMPLE_ITEMS`] items exactly as they are sent
    /// ([`item_for_prompt`]).
    pub sample: Vec<Value>,
    /// Size of the item data to send: the UTF-8 length of [`user_message`]
    /// over all items (the system prompt is not counted).
    pub total_bytes_to_send: u64,
    /// Estimated cost in US dollars; `None` for local providers or when the
    /// price of the model is unknown.
    pub est_cost_usd: Option<f64>,
}

impl RequestPreview {
    /// Builds the preview of sending `items` to the classifier `info`.
    pub fn new(info: &ClassifierInfo, items: &[ClassifyItem], est_cost_usd: Option<f64>) -> Self {
        let bytes = user_message(items).len();
        Self {
            provider: info.provider.to_owned(),
            model: info.model.clone(),
            is_local: info.is_local,
            item_count: items.len(),
            sample: items
                .iter()
                .take(PREVIEW_SAMPLE_ITEMS)
                .map(item_for_prompt)
                .collect(),
            total_bytes_to_send: u64::try_from(bytes).unwrap_or(u64::MAX),
            est_cost_usd,
        }
    }
}

/// Midnight UTC of the day of `t`; `None` when the year cannot be written as
/// RFC 3339 (outside `0..=9999`).
fn day(t: OffsetDateTime) -> Option<OffsetDateTime> {
    let utc = t
        .checked_to_offset(UtcOffset::UTC)?
        .replace_time(Time::MIDNIGHT);
    (0..=9999).contains(&utc.year()).then_some(utc)
}

/// Cuts `s` to `max` characters on a `char` boundary.
fn cut(s: &mut String, max: usize) {
    if let Some((idx, _)) = s.char_indices().nth(max) {
        s.truncate(idx);
    }
}

/// The summary as a JSON object without empty arrays and `null` values.
fn compact(s: &FolderSummary) -> Value {
    let Ok(Value::Object(obj)) = serde_json::to_value(s) else {
        // Not reachable: the dates are valid RFC 3339 and the keys are strings.
        return Value::Object(Map::new());
    };
    let obj = obj
        .into_iter()
        .filter(|(_, v)| match v {
            Value::Null => false,
            Value::Array(a) => !a.is_empty(),
            _ => true,
        })
        .collect();
    Value::Object(obj)
}

#[cfg(test)]
#[path = "request_tests.rs"]
mod tests;
