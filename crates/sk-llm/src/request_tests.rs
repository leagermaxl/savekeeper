use serde_json::{json, Value};
use sk_core::model::{ChildStat, ExtStat, FolderSummary, Marker};
use sk_core::template::PathTemplate;
use time::OffsetDateTime;

use super::*;
use crate::{ClassifierInfo, ClassifyItem, ContentPeek};

/// 2026-10-18T13:45:07Z.
const TS: i64 = 1_792_331_107;

fn ts(unix: i64) -> OffsetDateTime {
    match OffsetDateTime::from_unix_timestamp(unix) {
        Ok(t) => t,
        Err(e) => panic!("bad timestamp: {e}"),
    }
}

fn summary() -> FolderSummary {
    let path = match PathTemplate::parse(r"{APPDATA}\App") {
        Ok(t) => t,
        Err(e) => panic!("bad template: {e}"),
    };
    FolderSummary {
        path,
        total_bytes: 2048,
        file_count: 3,
        dir_count: 1,
        max_depth: 1,
        newest_mtime: Some(ts(TS)),
        oldest_mtime: None,
        ext_histogram: vec![ExtStat {
            ext: "json".to_owned(),
            count: 3,
            bytes: 2048,
        }],
        sample_names: vec!["settings.json".to_owned()],
        top_children: Vec::new(),
        markers: vec![Marker::ConfigLike],
        truncated: false,
    }
}

fn item(id: &str) -> ClassifyItem {
    ClassifyItem {
        id: id.to_owned(),
        summary: summary(),
        peek: Vec::new(),
        hint: None,
    }
}

fn len(v: &Value) -> usize {
    v.to_string().chars().count()
}

#[test]
fn dates_are_truncated_to_the_day_in_utc() {
    let v = summary_for_prompt(&summary());
    assert_eq!(v["newest_mtime"], json!("2026-10-18T00:00:00Z"));

    // A local offset is converted to UTC first: 01:30 at +03:00 is the day before in UTC.
    let mut s = summary();
    let local = ts(TS).replace_time(time::Time::MIDNIGHT);
    let offset = match time::UtcOffset::from_hms(3, 0, 0) {
        Ok(o) => o,
        Err(e) => panic!("bad offset: {e}"),
    };
    s.newest_mtime = Some(local.to_offset(offset) - time::Duration::hours(2));
    let v = summary_for_prompt(&s);
    assert_eq!(v["newest_mtime"], json!("2026-10-17T00:00:00Z"));
}

#[test]
fn seconds_do_not_change_the_summary() {
    let mut a = summary();
    let mut b = summary();
    a.newest_mtime = Some(ts(TS));
    b.newest_mtime = Some(ts(TS + 3_600));
    assert_eq!(summary_for_prompt(&a), summary_for_prompt(&b));
}

#[test]
fn empty_arrays_and_nulls_are_left_out() {
    let v = summary_for_prompt(&summary());
    let obj = match v.as_object() {
        Some(o) => o,
        None => panic!("not an object: {v}"),
    };
    assert!(!obj.contains_key("top_children"));
    assert!(!obj.contains_key("oldest_mtime"));
    assert_eq!(obj["path"], json!(r"{APPDATA}\App"));
    assert_eq!(obj["sample_names"], json!(["settings.json"]));
    assert_eq!(obj["markers"], json!(["config_like"]));
    assert_eq!(obj["truncated"], json!(false));
}

#[test]
fn long_names_are_cut() {
    let mut s = summary();
    s.sample_names = vec!["я".repeat(300)];
    s.top_children = vec![ChildStat {
        name: "c".repeat(200),
        bytes: 1,
        files: 1,
    }];
    let v = summary_for_prompt(&s);
    let name = v["sample_names"][0].as_str().unwrap_or_default();
    assert_eq!(name.chars().count(), MAX_NAME_CHARS);
    let child = v["top_children"][0]["name"].as_str().unwrap_or_default();
    assert_eq!(child.chars().count(), MAX_NAME_CHARS);
}

#[test]
fn summary_is_limited_to_4000_chars() {
    let mut s = summary();
    s.sample_names = (0..20)
        .map(|i| format!("{i:02}{}", "n".repeat(150)))
        .collect();
    s.top_children = (0..10)
        .map(|i| ChildStat {
            name: format!("{i}{}", "c".repeat(150)),
            bytes: 1,
            files: 1,
        })
        .collect();
    let v = summary_for_prompt(&s);
    assert!(len(&v) <= MAX_SUMMARY_CHARS, "{}", len(&v));
    // Sample names are dropped from the end first; children are kept.
    let names = v["sample_names"].as_array().map_or(0, Vec::len);
    assert!(names > 0 && names < 20, "{names}");
    assert_eq!(
        v["sample_names"][0].as_str().map(|n| n.starts_with("00")),
        Some(true)
    );
    assert_eq!(v["top_children"].as_array().map_or(0, Vec::len), 10);
}

#[test]
fn small_summary_is_not_cut() {
    let s = summary();
    let v = summary_for_prompt(&s);
    assert_eq!(v["sample_names"], json!(s.sample_names));
    assert_eq!(v["ext_histogram"].as_array().map_or(0, Vec::len), 1);
}

#[test]
fn item_omits_empty_peek_and_missing_hint() {
    let v = item_for_prompt(&item("a1"));
    assert_eq!(v["id"], json!("a1"));
    assert_eq!(v["summary"], summary_for_prompt(&summary()));
    assert!(v.get("peek").is_none());
    assert!(v.get("hint").is_none());

    let mut it = item("a2");
    it.hint = Some("electron".to_owned());
    it.peek = vec![ContentPeek {
        file: "config.ini".to_owned(),
        head: "[main]\nx=1".to_owned(),
    }];
    let v = item_for_prompt(&it);
    assert_eq!(v["hint"], json!("electron"));
    assert_eq!(
        v["peek"],
        json!([{ "file": "config.ini", "head": "[main]\nx=1" }])
    );
}

#[test]
fn user_message_is_compact_items_object() {
    let items = [item("a1"), item("a2")];
    let msg = user_message(&items);
    assert!(!msg.contains('\n') && !msg.contains(": "));
    let v: Value = match serde_json::from_str(&msg) {
        Ok(v) => v,
        Err(e) => panic!("invalid JSON: {e}"),
    };
    assert_eq!(
        v,
        json!({ "items": [item_for_prompt(&items[0]), item_for_prompt(&items[1])] })
    );
}

#[test]
fn summary_for_prompt_reads_back_as_folder_summary() {
    let v = summary_for_prompt(&summary());
    // Empty arrays are left out, so add them back before reading.
    let mut obj = v.as_object().cloned().unwrap_or_default();
    obj.insert("top_children".to_owned(), json!([]));
    let back: Result<FolderSummary, _> = serde_json::from_value(Value::Object(obj));
    assert!(back.is_ok(), "{back:?}");
}

#[test]
fn preview_shows_the_first_three_items_as_sent() {
    let info = ClassifierInfo {
        provider: "anthropic",
        model: "claude-haiku-4-5".to_owned(),
        is_local: false,
        max_batch: 20,
        max_concurrency: 2,
    };
    let items: Vec<ClassifyItem> = (0..5).map(|i| item(&format!("id{i}"))).collect();
    let p = RequestPreview::new(&info, &items, Some(0.01));
    assert_eq!(p.provider, "anthropic");
    assert_eq!(p.model, "claude-haiku-4-5");
    assert!(!p.is_local);
    assert_eq!(p.item_count, 5);
    assert_eq!(p.sample.len(), PREVIEW_SAMPLE_ITEMS);
    assert_eq!(p.sample[0], item_for_prompt(&items[0]));
    assert_eq!(p.sample[2]["id"], json!("id2"));
    assert_eq!(p.total_bytes_to_send, user_message(&items).len() as u64);
    assert_eq!(p.est_cost_usd, Some(0.01));

    let empty = RequestPreview::new(&info, &[], None);
    assert_eq!(empty.item_count, 0);
    assert!(empty.sample.is_empty());
    assert_eq!(empty.est_cost_usd, None);
}
