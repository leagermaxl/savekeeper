use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sk_core::env::Environment;
use sk_core::model::{
    Category, Evidence, EvidenceSource, Finding, FindingId, RegHive, Sensitivity, Target,
};
use sk_core::template::{PathTemplate, ResolveContext};

use crate::{merge_nested, NESTED_TAG, VIA_ARG};

pub(crate) fn env() -> Environment {
    Environment::fake(Path::new("/fake"))
}

pub(crate) fn resolve(template: &PathTemplate) -> PathBuf {
    template
        .resolve(&env(), &ResolveContext::default())
        .into_iter()
        .next()
        .unwrap()
}

pub(crate) fn fileset(template: &str, include: &[&str], exclude: &[&str]) -> Target {
    let root = PathTemplate::parse(template).unwrap();
    Target::FileSet {
        resolved: resolve(&root),
        root,
        include: include.iter().map(|s| (*s).to_owned()).collect(),
        exclude: exclude.iter().map(|s| (*s).to_owned()).collect(),
    }
}

pub(crate) fn file(template: &str) -> Target {
    let path = PathTemplate::parse(template).unwrap();
    Target::File {
        resolved: resolve(&path),
        path,
    }
}

pub(crate) fn ev(source: EvidenceSource, key: &str) -> Evidence {
    Evidence {
        source,
        message_key: key.to_owned(),
        message_args: BTreeMap::new(),
        confidence: 0.8,
        importance: None,
    }
}

pub(crate) fn rule(id: &str) -> EvidenceSource {
    EvidenceSource::Rule {
        rule_id: id.to_owned(),
    }
}

pub(crate) fn heuristic(id: &str) -> EvidenceSource {
    EvidenceSource::Heuristic {
        heuristic_id: id.to_owned(),
    }
}

pub(crate) fn finding(target: Target, category: Category, title: &str) -> Finding {
    Finding {
        id: FindingId::for_target(&target),
        target,
        category,
        app: None,
        title: title.to_owned(),
        evidence: vec![ev(rule(title), "evidence.rule_match")],
        stats: None,
        sensitivity: Sensitivity::None,
        score: None,
        default_selected: false,
        requires_elevation: false,
        tags: vec![],
        children: vec![],
        notes_key: None,
    }
}

pub(crate) fn run(mut findings: Vec<Finding>) -> Vec<Finding> {
    merge_nested(&mut findings, &env());
    findings
}

pub(crate) fn by_title<'a>(findings: &'a [Finding], title: &str) -> &'a Finding {
    findings.iter().find(|f| f.title == title).unwrap()
}

pub(crate) fn is_nested(f: &Finding) -> bool {
    f.tags.iter().any(|t| t == NESTED_TAG)
}

pub(crate) fn excludes(f: &Finding) -> Vec<String> {
    match &f.target {
        Target::FileSet { exclude, .. } => exclude.clone(),
        _ => panic!("not a FileSet"),
    }
}

#[test]
fn file_inside_a_fileset_becomes_its_child() {
    let parent = finding(
        fileset(r"{APPDATA}\Foo", &[], &[]),
        Category::AppData,
        "Foo",
    );
    let child = finding(
        file(r"{APPDATA}\Foo\settings.json"),
        Category::AppConfig,
        "Foo settings",
    );
    let child_id = child.id.clone();
    let out = run(vec![child, parent]);

    let parent = by_title(&out, "Foo");
    let child = by_title(&out, "Foo settings");
    assert_eq!(parent.children, vec![child_id]);
    assert!(is_nested(child));
    assert!(!is_nested(parent));
    assert_eq!(parent.category, Category::AppData);
    assert!(child.children.is_empty());
    // The order of the input is kept.
    assert_eq!(out[0].title, "Foo settings");
}

#[test]
fn important_child_of_reinstallable_parent_is_excluded_not_absorbed() {
    let parent = finding(
        fileset(r"{LOCALAPPDATA}\Bar", &[], &[]),
        Category::Reinstallable,
        "Bar",
    );
    let parent_id = parent.id.clone();
    let child = finding(
        fileset(r"{LOCALAPPDATA}\Bar\Saves", &[], &[]),
        Category::GameSave,
        "Bar saves",
    );
    let out = run(vec![parent, child]);

    let parent = by_title(&out, "Bar");
    let child = by_title(&out, "Bar saves");
    assert_eq!(excludes(parent), vec!["Saves/**".to_owned()]);
    assert!(parent.children.is_empty());
    assert!(!is_nested(child));
    assert_eq!(parent.id, parent_id, "the id is not recomputed");
}

#[test]
fn important_file_in_cache_parent_is_excluded_by_its_path() {
    let parent = finding(
        fileset(r"{LOCALAPPDATA}\Bar", &[], &["*.tmp"]),
        Category::Cache,
        "Bar",
    );
    let child = finding(
        file(r"{LOCALAPPDATA}\Bar\cfg\[1].ini"),
        Category::AppConfig,
        "Bar config",
    );
    let out = run(vec![parent, child]);
    let parent = by_title(&out, "Bar");
    assert_eq!(
        excludes(parent),
        vec!["*.tmp".to_owned(), "cfg/[[]1[]].ini".to_owned()]
    );
    assert!(!is_nested(by_title(&out, "Bar config")));
}

#[test]
fn unknown_parent_keeps_its_category_and_excludes_the_important_child() {
    let parent = finding(
        fileset(r"{DOCUMENTS}\Stuff", &[], &[]),
        Category::Unknown,
        "Stuff",
    );
    let child = finding(
        fileset(r"{DOCUMENTS}\Stuff\Thesis", &[], &[]),
        Category::UserFiles,
        "Thesis",
    );
    let out = run(vec![parent, child]);
    let parent = by_title(&out, "Stuff");
    assert_eq!(parent.category, Category::Unknown);
    assert_eq!(excludes(parent), vec!["Thesis/**".to_owned()]);
    assert!(parent.children.is_empty());
    assert!(!is_nested(by_title(&out, "Thesis")));
}

#[test]
fn important_child_of_unknown_is_excluded_from_covering_ancestors_too() {
    let top = finding(
        fileset(r"{APPDATA}\Foo", &["*.ini"], &[]),
        Category::AppConfig,
        "top",
    );
    let data = finding(
        fileset(r"{APPDATA}\Foo\Data", &[], &[]),
        Category::AppData,
        "data",
    );
    let misc = finding(
        fileset(r"{APPDATA}\Foo\Data\Misc", &[], &[]),
        Category::Unknown,
        "misc",
    );
    let save = finding(
        fileset(r"{APPDATA}\Foo\Data\Misc\Saves", &[], &[]),
        Category::GameSave,
        "save",
    );
    let file_save = finding(
        file(r"{APPDATA}\Foo\Data\Misc\slot.sav"),
        Category::GameSave,
        "slot",
    );
    let out = run(vec![save, file_save, misc, data, top]);

    assert_eq!(
        excludes(by_title(&out, "misc")),
        vec!["Saves/**".to_owned(), "slot.sav".to_owned()]
    );
    assert_eq!(
        excludes(by_title(&out, "data")),
        vec!["Misc/Saves/**".to_owned(), "Misc/slot.sav".to_owned()]
    );
    // `*.ini` does not cover the saves: no exclude needed.
    assert!(excludes(by_title(&out, "top")).is_empty());
    assert!(!is_nested(by_title(&out, "save")));
    assert!(!is_nested(by_title(&out, "slot")));
    // The Unknown itself still nests into the AppData parent.
    assert!(is_nested(by_title(&out, "misc")));
    assert_eq!(by_title(&out, "data").children.len(), 1);
}

#[test]
fn unimportant_child_of_unimportant_parent_is_absorbed() {
    let parent = finding(
        fileset(r"{LOCALAPPDATA}\Bar", &[], &[]),
        Category::Unknown,
        "Bar",
    );
    let child = finding(
        fileset(r"{LOCALAPPDATA}\Bar\Cache", &[], &[]),
        Category::Cache,
        "Bar cache",
    );
    let out = run(vec![parent, child]);
    assert!(is_nested(by_title(&out, "Bar cache")));
    assert!(excludes(by_title(&out, "Bar")).is_empty());
}

#[test]
fn closest_enclosing_fileset_is_the_parent() {
    let outer = finding(
        fileset(r"{APPDATA}\Foo", &[], &[]),
        Category::AppData,
        "outer",
    );
    let inner = finding(
        fileset(r"{APPDATA}\Foo\Profiles", &[], &[]),
        Category::AppData,
        "inner",
    );
    let leaf = finding(
        file(r"{APPDATA}\Foo\Profiles\default\prefs.js"),
        Category::AppConfig,
        "leaf",
    );
    let (inner_id, leaf_id) = (inner.id.clone(), leaf.id.clone());
    let out = run(vec![leaf, inner, outer]);
    assert_eq!(by_title(&out, "outer").children, vec![inner_id]);
    assert_eq!(by_title(&out, "inner").children, vec![leaf_id]);
    assert!(is_nested(by_title(&out, "inner")));
    assert!(is_nested(by_title(&out, "leaf")));
}

#[test]
fn sensitivity_and_new_evidence_bubble_up_with_via() {
    let outer = finding(
        fileset(r"{APPDATA}\Foo", &[], &[]),
        Category::AppData,
        "outer",
    );
    let mut inner = finding(
        fileset(r"{APPDATA}\Foo\Profiles", &[], &[]),
        Category::AppData,
        "inner",
    );
    inner.evidence.push(ev(rule("outer"), "evidence.other"));
    let mut leaf = finding(
        file(r"{APPDATA}\Foo\Profiles\logins.json"),
        Category::Credentials,
        "leaf",
    );
    leaf.sensitivity = Sensitivity::High;
    leaf.evidence = vec![ev(heuristic("secrets"), "evidence.secret")];
    let out = run(vec![outer, inner, leaf]);

    let outer = by_title(&out, "outer");
    let inner = by_title(&out, "inner");
    assert_eq!(inner.sensitivity, Sensitivity::High);
    assert_eq!(outer.sensitivity, Sensitivity::High);

    let via = |f: &Finding, source: &EvidenceSource| {
        f.evidence
            .iter()
            .find(|e| &e.source == source)
            .and_then(|e| e.message_args.get(VIA_ARG).cloned())
    };
    assert_eq!(via(inner, &heuristic("secrets")).as_deref(), Some("leaf"));
    // The origin is kept when the evidence goes further up.
    assert_eq!(via(outer, &heuristic("secrets")).as_deref(), Some("leaf"));
    assert_eq!(via(outer, &rule("inner")).as_deref(), Some("inner"));
    // `rule:outer` is already present in the parent: not copied again.
    assert_eq!(
        outer
            .evidence
            .iter()
            .filter(|e| e.source == rule("outer"))
            .count(),
        1
    );
    assert_eq!(via(outer, &rule("outer")), None);
}

#[test]
fn file_and_fileset_on_one_path_nest_the_file() {
    let set = finding(
        fileset(r"{APPDATA}\Foo", &[], &[]),
        Category::AppData,
        "set",
    );
    let one = finding(file(r"{APPDATA}\Foo"), Category::AppConfig, "file");
    let out = run(vec![one, set]);
    assert!(is_nested(by_title(&out, "file")));
    assert!(!is_nested(by_title(&out, "set")));
}

#[test]
fn filesets_on_one_path_nest_by_id() {
    let a = finding(
        fileset(r"{APPDATA}\Foo", &[], &[]),
        Category::AppData,
        "all",
    );
    let b = finding(
        fileset(r"{APPDATA}\Foo", &["*.ini"], &[]),
        Category::AppConfig,
        "ini",
    );
    let all_is_smaller = a.id < b.id;
    let out = run(vec![a, b]);
    // Only the set with every file can be the parent, and only if its id is smaller.
    assert_eq!(is_nested(by_title(&out, "ini")), all_is_smaller);
    assert!(!is_nested(by_title(&out, "all")));

    // Same globs: the smaller id is the parent.
    let c = finding(fileset(r"{APPDATA}\Foo", &[], &[]), Category::AppData, "c");
    let mut d = c.clone();
    d.title = "d".to_owned();
    d.id = FindingId::for_target(&fileset(r"{APPDATA}\Bar", &[], &[]));
    let small = if c.id < d.id { "c" } else { "d" };
    let big = if small == "c" { "d" } else { "c" };
    let out = run(vec![c.clone(), d.clone()]);
    assert!(!is_nested(by_title(&out, small)));
    assert!(is_nested(by_title(&out, big)));
    let out = run(vec![d, c]);
    assert!(is_nested(by_title(&out, big)));
}

#[test]
fn result_does_not_depend_on_input_order() {
    let make = || {
        vec![
            finding(
                fileset(r"{APPDATA}\Foo", &[], &[]),
                Category::AppData,
                "outer",
            ),
            finding(
                fileset(r"{APPDATA}\Foo\P", &[], &[]),
                Category::Unknown,
                "unk",
            ),
            finding(
                fileset(r"{APPDATA}\Foo\P\S", &[], &[]),
                Category::GameSave,
                "save",
            ),
            finding(file(r"{APPDATA}\Foo\P\S\a.sav"), Category::GameSave, "a"),
            finding(file(r"{APPDATA}\Foo\b.ini"), Category::AppConfig, "b"),
        ]
    };
    let mut expected = run(make());
    expected.sort_by(|a, b| a.id.cmp(&b.id));
    let mut reversed = make();
    reversed.reverse();
    let mut got = run(reversed);
    got.sort_by(|a, b| a.id.cmp(&b.id));
    assert_eq!(format!("{expected:?}"), format!("{got:?}"));
    // `save` is excluded from the Unknown `unk` and from `outer` above it,
    // `a` nests into `save`.
    assert_eq!(excludes(by_title(&got, "unk")), vec!["S/**".to_owned()]);
    assert_eq!(excludes(by_title(&got, "outer")), vec!["P/S/**".to_owned()]);
    assert!(is_nested(by_title(&got, "a")));
    assert!(!is_nested(by_title(&got, "save")));
    assert!(is_nested(by_title(&got, "unk")));
}

#[test]
fn registry_and_system_exports_are_ignored() {
    let set = finding(
        fileset(r"{APPDATA}\Foo", &[], &[]),
        Category::AppData,
        "set",
    );
    let reg = finding(
        Target::Registry {
            hive: RegHive::Hkcu,
            key: r"Software\Foo".to_owned(),
            recursive: true,
        },
        Category::AppConfig,
        "reg",
    );
    let out = run(vec![set, reg]);
    assert!(out.iter().all(|f| !is_nested(f) && f.children.is_empty()));
}

#[test]
fn empty_input_is_fine() {
    assert!(run(vec![]).is_empty());
}
