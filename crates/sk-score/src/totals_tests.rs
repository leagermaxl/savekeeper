//! Totals over findings (SPEC-09 §4.9, §6.2 "Totals are not doubled").

use sk_core::model::{Category, CategoryTotals, Finding, Sensitivity, Totals};

use crate::nested_tests::{by_title, env, file, fileset, finding, is_nested};
use crate::score_table_tests::stats;
use crate::{merge_nested, totals};

const MIB: u64 = 1024 * 1024;

fn sized(template: &str, category: Category, title: &str, bytes: Option<u64>) -> Finding {
    let mut f = finding(fileset(template, &[], &[]), category, title);
    f.stats = bytes.map(|b| stats(b, 1));
    f
}

fn all_selected(_: &Finding) -> bool {
    true
}

#[test]
fn empty_input_gives_zero_totals() {
    assert_eq!(totals(&[], all_selected), Totals::default());
}

#[test]
fn nested_findings_are_not_doubled() {
    let mut parent = sized("{APPDATA}\\Foo", Category::AppData, "Foo", Some(10 * MIB));
    parent.default_selected = true;
    let mut child = finding(
        file("{APPDATA}\\Foo\\settings.json"),
        Category::AppConfig,
        "settings",
    );
    child.stats = Some(stats(MIB, 1));
    child.default_selected = true;
    let mut findings = vec![parent, child];
    merge_nested(&mut findings, &env());
    assert!(is_nested(by_title(&findings, "settings")));

    let t = totals(&findings, |f| f.default_selected);
    let expected = CategoryTotals {
        count: 1,
        bytes: 10 * MIB,
        selected_count: 1,
        selected_bytes: 10 * MIB,
    };
    assert_eq!(t.all, expected);
    assert_eq!(t.by_category.len(), 1);
    assert_eq!(t.by_category[&Category::AppData], expected);
    assert!(!t.by_category.contains_key(&Category::AppConfig));
}

#[test]
fn alternative_findings_are_not_counted() {
    let repo = sized(
        "{HOME}\\src\\repo",
        Category::DevEnvironment,
        "repo",
        Some(MIB),
    );
    let mut bundle = sized(
        "{HOME}\\src\\repo-bundle",
        Category::DevEnvironment,
        "repo",
        Some(MIB),
    );
    bundle.tags = vec!["alternative".to_owned(), "git".to_owned()];
    bundle.requires_elevation = true;
    bundle.sensitivity = Sensitivity::High;

    let t = totals(&[repo, bundle], all_selected);
    assert_eq!(t.all.count, 1);
    assert_eq!(t.all.bytes, MIB);
    assert_eq!(t.by_category[&Category::DevEnvironment].count, 1);
    assert_eq!(t.sensitive_selected, 0);
    assert_eq!(t.needs_elevation_count, 0);
}

#[test]
fn missing_stats_count_as_zero_bytes() {
    let f = sized("{HOME}\\x", Category::SystemSettings, "x", None);
    let t = totals(&[f], all_selected);
    assert_eq!(
        t.all,
        CategoryTotals {
            count: 1,
            bytes: 0,
            selected_count: 1,
            selected_bytes: 0,
        }
    );
}

#[test]
fn predicate_decides_selection() {
    let a = sized("{HOME}\\a", Category::UserFiles, "a", Some(3 * MIB));
    let b = sized("{HOME}\\b", Category::UserFiles, "b", Some(5 * MIB));
    let c = sized("{HOME}\\c", Category::GameSave, "c", Some(7 * MIB));
    let findings = [a, b, c];

    let t = totals(&findings, |f| f.title != "b");
    assert_eq!(
        t.by_category[&Category::UserFiles],
        CategoryTotals {
            count: 2,
            bytes: 8 * MIB,
            selected_count: 1,
            selected_bytes: 3 * MIB,
        }
    );
    assert_eq!(
        t.all,
        CategoryTotals {
            count: 3,
            bytes: 15 * MIB,
            selected_count: 2,
            selected_bytes: 10 * MIB,
        }
    );

    let none = totals(&findings, |_| false);
    assert_eq!(none.all.selected_count, 0);
    assert_eq!(none.all.selected_bytes, 0);
    assert_eq!(none.all.count, 3);
}

#[test]
fn category_sums_equal_all() {
    let findings = [
        sized("{HOME}\\a", Category::UserFiles, "a", Some(3)),
        sized("{HOME}\\b", Category::Cache, "b", Some(5)),
        sized("{HOME}\\c", Category::GameSave, "c", None),
        sized("{HOME}\\d", Category::GameSave, "d", Some(11)),
    ];
    let t = totals(&findings, |f| f.category != Category::Cache);
    let sum = |g: fn(&CategoryTotals) -> u64| t.by_category.values().map(g).sum::<u64>();
    assert_eq!(sum(|c| c.selected_bytes), t.all.selected_bytes);
    assert_eq!(sum(|c| c.bytes), t.all.bytes);
    assert_eq!(sum(|c| u64::from(c.count)), u64::from(t.all.count));
    assert_eq!(
        sum(|c| u64::from(c.selected_count)),
        u64::from(t.all.selected_count)
    );
    assert_eq!(t.all.selected_bytes, 14);
}

#[test]
fn special_counters() {
    let creds = sized("{HOME}\\kp", Category::Credentials, "kp", Some(1));
    let mut wifi = sized("{HOME}\\wifi", Category::SystemSettings, "wifi", None);
    wifi.sensitivity = Sensitivity::High;
    wifi.requires_elevation = true;
    let mut high_unselected = sized("{HOME}\\h", Category::AppData, "h", Some(1));
    high_unselected.sensitivity = Sensitivity::High;
    let unknown = sized("{HOME}\\u", Category::Unknown, "u", Some(1));
    let mut photos = sized("{HOME}\\p", Category::UserFiles, "p", Some(1));
    photos.tags = vec!["too_large_default".to_owned()];

    let findings = [creds, wifi, high_unselected, unknown, photos];
    let t = totals(&findings, |f| f.title != "h");
    assert_eq!(t.sensitive_selected, 2);
    assert_eq!(t.unknown_count, 1);
    assert_eq!(t.needs_elevation_count, 1);
    assert_eq!(t.too_large_count, 1);
    assert_eq!(t.all.count, 5);
}
