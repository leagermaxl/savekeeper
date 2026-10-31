//! Result order of findings (SPEC-09 §4.8).

use sk_core::model::{Category, Finding, FindingId, Score};

use crate::nested_tests::{fileset, finding};
use crate::order::sort_findings;

fn scored(template: &str, category: Category, title: &str, value: Option<f32>) -> Finding {
    let mut f = finding(fileset(template, &[], &[]), category, title);
    f.score = value.map(|value| Score {
        value,
        components: Default::default(),
    });
    f
}

fn titles(findings: &[Finding]) -> Vec<&str> {
    findings.iter().map(|f| f.title.as_str()).collect()
}

#[test]
fn categories_follow_spec_order() {
    let order = [
        Category::Credentials,
        Category::GameSave,
        Category::UserFiles,
        Category::DevEnvironment,
        Category::AppData,
        Category::AppConfig,
        Category::GameConfig,
        Category::SystemSettings,
        Category::Unknown,
        Category::Reinstallable,
        Category::Cache,
    ];
    // Reverse input, higher scores on later categories: category still wins.
    let mut findings: Vec<Finding> = order
        .iter()
        .enumerate()
        .rev()
        .map(|(i, c)| {
            let title = format!("{c:?}");
            let template = format!("{{HOME}}\\{i}");
            scored(&template, *c, &title, Some(i as f32 / 10.0))
        })
        .collect();
    sort_findings(&mut findings);
    let got: Vec<Category> = findings.iter().map(|f| f.category).collect();
    assert_eq!(got, order);
}

#[test]
fn score_descending_within_category() {
    let mut findings = vec![
        scored("{HOME}\\a", Category::AppData, "a", Some(0.2)),
        scored("{HOME}\\b", Category::AppData, "b", Some(0.9)),
        scored("{HOME}\\c", Category::AppData, "c", None),
        scored("{HOME}\\d", Category::AppData, "d", Some(0.5)),
        scored("{HOME}\\e", Category::AppData, "e", Some(f32::NAN)),
    ];
    sort_findings(&mut findings);
    // Missing and NaN scores count as 0 and fall back to the title order.
    assert_eq!(titles(&findings), ["b", "d", "a", "c", "e"]);
}

#[test]
fn title_case_insensitive_then_id() {
    let mut findings = vec![
        scored("{HOME}\\3", Category::GameSave, "beta", Some(0.5)),
        scored("{HOME}\\2", Category::GameSave, "Alpha", Some(0.5)),
        scored("{HOME}\\1", Category::GameSave, "alpha", Some(0.5)),
        scored("{HOME}\\4", Category::GameSave, "ALPHA", Some(0.5)),
    ];
    sort_findings(&mut findings);
    assert_eq!(&titles(&findings)[3..], ["beta"]);
    let alpha_ids: Vec<&FindingId> = findings[..3].iter().map(|f| &f.id).collect();
    let mut sorted = alpha_ids.clone();
    sorted.sort();
    assert_eq!(alpha_ids, sorted);
}

#[test]
fn order_does_not_depend_on_input_order() {
    let make = || {
        vec![
            scored("{HOME}\\a", Category::Cache, "x", Some(0.1)),
            scored("{HOME}\\b", Category::GameSave, "Y", Some(0.7)),
            scored("{HOME}\\c", Category::GameSave, "y", Some(0.7)),
            scored("{HOME}\\d", Category::Credentials, "z", Some(0.3)),
        ]
    };
    let mut forward = make();
    let mut backward = make();
    backward.reverse();
    sort_findings(&mut forward);
    sort_findings(&mut backward);
    let ids = |v: &[Finding]| v.iter().map(|f| f.id.clone()).collect::<Vec<_>>();
    assert_eq!(ids(&forward), ids(&backward));
    assert_eq!(forward[0].title, "z");
    assert_eq!(forward[3].title, "x");
}
