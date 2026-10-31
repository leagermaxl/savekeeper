use std::collections::BTreeMap;
use std::path::PathBuf;

use sk_core::model::{
    AppKind, AppRef, Category, Evidence, EvidenceSource, Finding, FindingId, IssueSeverity,
    ScanIssue, Sensitivity, Target, TargetStats,
};
use sk_core::template::PathTemplate;

use crate::merge::merge_by_id_reporting;
use crate::{merge_by_id, CATEGORY_CONFLICT_KEY, CONFLICT_TAG};
use crate::{BAD_NUMBER_KEY, ISSUE_SOURCE};

fn ev(source: EvidenceSource, key: &str, confidence: f32) -> Evidence {
    Evidence {
        source,
        message_key: key.to_owned(),
        message_args: BTreeMap::new(),
        confidence,
        importance: None,
    }
}

fn rule(id: &str) -> EvidenceSource {
    EvidenceSource::Rule {
        rule_id: id.to_owned(),
    }
}

fn heuristic(id: &str) -> EvidenceSource {
    EvidenceSource::Heuristic {
        heuristic_id: id.to_owned(),
    }
}

fn llm() -> EvidenceSource {
    EvidenceSource::Llm {
        provider: "ollama".to_owned(),
        model: "m".to_owned(),
    }
}

fn fileset(template: &str, include: &[&str], exclude: &[&str]) -> Target {
    Target::FileSet {
        root: PathTemplate::parse(template).unwrap(),
        resolved: PathBuf::from(template),
        include: include.iter().map(|s| (*s).to_owned()).collect(),
        exclude: exclude.iter().map(|s| (*s).to_owned()).collect(),
    }
}

fn finding(target: Target, category: Category, title: &str, evidence: Vec<Evidence>) -> Finding {
    Finding {
        id: FindingId::for_target(&target),
        target,
        category,
        app: None,
        title: title.to_owned(),
        evidence,
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

/// A finding on `{APPDATA}\Foo`; all calls share one id.
fn foo(category: Category, evidence: Vec<Evidence>) -> Finding {
    finding(
        fileset(r"{APPDATA}\Foo", &[], &[]),
        category,
        "Foo",
        evidence,
    )
}

fn app(id: &str) -> AppRef {
    AppRef {
        id: id.to_owned(),
        name: id.to_owned(),
        kind: AppKind::Application,
        source_ids: BTreeMap::new(),
        installed: Some(true),
        process_names: vec![],
    }
}

fn stats(bytes: u64) -> TargetStats {
    TargetStats {
        total_bytes: bytes,
        file_count: 1,
        dir_count: 0,
        newest_mtime: None,
        oldest_mtime: None,
        locked_files: 0,
        cloud_only_bytes: 0,
        cloud_only_files: 0,
        largest_file_bytes: None,
        truncated: false,
    }
}

fn merge(findings: Vec<Finding>) -> (Vec<Finding>, Vec<ScanIssue>) {
    let mut issues = Vec::new();
    let out = merge_by_id_reporting(findings, &mut issues);
    (out, issues)
}

#[test]
fn empty_input_gives_empty_output() {
    let (out, issues) = merge(vec![]);
    assert!(out.is_empty());
    assert!(issues.is_empty());
}

#[test]
fn rule_beats_llm_and_cache_loser_is_not_a_conflict() {
    // SPEC-09 §6.2: Rule AppConfig C=0.9 + Llm Cache C=0.8.
    let a = foo(
        Category::AppConfig,
        vec![ev(rule("r"), "evidence.rule", 0.9)],
    );
    let b = foo(Category::Cache, vec![ev(llm(), "evidence.llm", 0.8)]);
    let (out, issues) = merge(vec![b, a]);

    assert_eq!(out.len(), 1);
    let f = &out[0];
    assert_eq!(f.category, Category::AppConfig);
    assert_eq!(f.evidence.len(), 2);
    assert_eq!(f.evidence[0].source, rule("r"));
    assert_eq!(f.evidence[1].source, llm());
    assert!(!f.tags.iter().any(|t| t == CONFLICT_TAG));
    assert!(issues.is_empty());
}

#[test]
fn reinstallable_winner_over_game_save_is_a_conflict() {
    // SPEC-09 §6.2: Rule Reinstallable + Heuristic GameSave C=0.8.
    let a = foo(
        Category::Reinstallable,
        vec![ev(rule("r"), "evidence.rule", 0.5)],
    );
    let b = foo(
        Category::GameSave,
        vec![ev(heuristic("h"), "evidence.h", 0.8)],
    );
    let id = a.id.clone();
    let (out, issues) = merge(vec![a, b]);

    assert_eq!(out.len(), 1);
    assert_eq!(out[0].category, Category::Reinstallable);
    assert_eq!(out[0].tags, vec![CONFLICT_TAG.to_owned()]);
    assert_eq!(out[0].evidence.len(), 2);

    assert_eq!(issues.len(), 1);
    let issue = &issues[0];
    assert_eq!(issue.severity, IssueSeverity::Warning);
    assert_eq!(issue.source, ISSUE_SOURCE);
    assert_eq!(issue.message_key, CATEGORY_CONFLICT_KEY);
    assert_eq!(issue.path.as_deref(), Some(r"{APPDATA}\Foo"));
    assert_eq!(issue.message_args["finding_id"], id.as_str());
    assert_eq!(issue.message_args["winner"], "reinstallable");
    assert_eq!(issue.message_args["loser"], "game_save");
}

#[test]
fn conflict_needs_confidence_of_at_least_0_7() {
    let winner = || foo(Category::Cache, vec![ev(rule("r"), "evidence.rule", 1.0)]);
    let loser = |c| {
        foo(
            Category::DevEnvironment,
            vec![ev(heuristic("h"), "evidence.h", c)],
        )
    };

    let (out, issues) = merge(vec![winner(), loser(0.69)]);
    assert!(out[0].tags.is_empty());
    assert!(issues.is_empty());

    let (out, issues) = merge(vec![winner(), loser(0.7)]);
    assert_eq!(out[0].tags, vec![CONFLICT_TAG.to_owned()]);
    assert_eq!(issues.len(), 1);
}

#[test]
fn important_winner_over_cache_loser_is_not_a_conflict() {
    let a = foo(
        Category::UserFiles,
        vec![ev(rule("r"), "evidence.rule", 1.0)],
    );
    let b = foo(Category::Cache, vec![ev(heuristic("h"), "evidence.h", 1.0)]);
    let (out, issues) = merge(vec![a, b]);
    assert_eq!(out[0].category, Category::UserFiles);
    assert!(out[0].tags.is_empty());
    assert!(issues.is_empty());
}

#[test]
fn authority_ties_go_to_category_order_then_title() {
    // Credentials comes before GameSave in §4.8 (unlike the enum order).
    let a = foo(Category::GameSave, vec![ev(heuristic("a"), "k", 0.8)]);
    let b = foo(Category::Credentials, vec![ev(heuristic("b"), "k", 0.8)]);
    let (out, _) = merge(vec![a, b]);
    assert_eq!(out[0].category, Category::Credentials);

    let mut a = foo(Category::AppData, vec![ev(heuristic("a"), "k", 0.8)]);
    a.title = "Beta".to_owned();
    let mut b = foo(Category::AppData, vec![ev(heuristic("b"), "k", 0.8)]);
    b.title = "Alpha".to_owned();
    let (out, _) = merge(vec![a, b]);
    assert_eq!(out[0].title, "Alpha");
}

#[test]
fn app_stats_and_notes_fall_back_to_the_next_authority() {
    let winner = foo(Category::AppData, vec![ev(rule("r"), "k", 0.9)]);
    let mut weak = foo(Category::AppData, vec![ev(llm(), "k", 0.9)]);
    weak.app = Some(app("weak"));
    weak.stats = Some(stats(1));
    let mut middle = foo(Category::AppData, vec![ev(heuristic("h"), "k", 0.9)]);
    middle.app = Some(app("middle"));
    middle.stats = Some(stats(2));
    middle.notes_key = Some("notes.middle".to_owned());

    let (out, _) = merge(vec![weak, winner, middle]);
    let f = &out[0];
    assert_eq!(f.app.as_ref().map(|a| a.id.as_str()), Some("middle"));
    assert_eq!(f.stats.as_ref().map(|s| s.total_bytes), Some(2));
    assert_eq!(f.notes_key.as_deref(), Some("notes.middle"));
}

#[test]
fn winner_app_and_stats_are_kept() {
    let mut winner = foo(Category::AppData, vec![ev(rule("r"), "k", 0.9)]);
    winner.app = Some(app("winner"));
    winner.stats = Some(stats(7));
    let mut loser = foo(Category::AppData, vec![ev(llm(), "k", 0.9)]);
    loser.app = Some(app("loser"));
    loser.stats = Some(stats(9));
    let (out, _) = merge(vec![loser, winner]);
    assert_eq!(out[0].app.as_ref().map(|a| a.id.as_str()), Some("winner"));
    assert_eq!(out[0].stats.as_ref().map(|s| s.total_bytes), Some(7));
}

#[test]
fn evidence_is_deduplicated_by_source_and_key_keeping_higher_confidence() {
    let a = foo(
        Category::AppData,
        vec![ev(heuristic("h"), "k", 0.4), ev(llm(), "evidence.llm", 0.9)],
    );
    let b = foo(
        Category::AppData,
        vec![
            ev(heuristic("h"), "k", 0.7),
            ev(heuristic("h"), "other", 0.1),
            ev(rule("r"), "k", 0.2),
        ],
    );
    let (out, _) = merge(vec![a, b]);
    let got: Vec<(EvidenceSource, &str, f32)> = out[0]
        .evidence
        .iter()
        .map(|e| (e.source.clone(), e.message_key.as_str(), e.confidence))
        .collect();
    assert_eq!(
        got,
        vec![
            (rule("r"), "k", 0.2),
            (heuristic("h"), "k", 0.7),
            (heuristic("h"), "other", 0.1),
            (llm(), "evidence.llm", 0.9),
        ]
    );
}

#[test]
fn tags_children_sensitivity_and_elevation_are_combined() {
    let mut a = foo(Category::AppData, vec![ev(rule("r"), "k", 0.9)]);
    a.tags = vec!["steam".to_owned(), "b".to_owned()];
    a.sensitivity = Sensitivity::Low;
    a.children = vec![FindingId::for_target(&fileset(r"{APPDATA}\X", &[], &[]))];
    let mut b = foo(Category::AppData, vec![ev(llm(), "k", 0.9)]);
    b.tags = vec!["b".to_owned(), "a".to_owned()];
    b.sensitivity = Sensitivity::High;
    b.requires_elevation = true;
    b.children = a.children.clone();

    let (out, _) = merge(vec![a, b]);
    let f = &out[0];
    assert_eq!(f.tags, vec!["a", "b", "steam"]);
    assert_eq!(f.sensitivity, Sensitivity::High);
    assert!(f.requires_elevation);
    assert_eq!(f.children.len(), 1);
}

fn globs(f: &Finding) -> (Vec<String>, Vec<String>) {
    match &f.target {
        Target::FileSet {
            include, exclude, ..
        } => (include.clone(), exclude.clone()),
        other => panic!("not a file set: {other:?}"),
    }
}

/// Findings with forced equal ids but different globs (an id collision).
fn colliding(include: &[&str], exclude: &[&str], source: EvidenceSource) -> Finding {
    let mut f = finding(
        fileset(r"{APPDATA}\Foo", include, exclude),
        Category::AppData,
        "Foo",
        vec![ev(source, "k", 0.9)],
    );
    f.id = FindingId::for_target(&fileset(r"{APPDATA}\Foo", &[], &[]));
    f
}

#[test]
fn include_is_united_when_all_are_non_empty_and_exclude_intersected() {
    let a = colliding(&["*.sav", "*.ini"], &["tmp/**", "*.log"], rule("r"));
    let b = colliding(&["*.cfg", "*.ini"], &["*.log", "cache/**"], llm());
    let (out, _) = merge(vec![a, b]);
    assert_eq!(out.len(), 1);
    let (include, exclude) = globs(&out[0]);
    assert_eq!(include, vec!["*.cfg", "*.ini", "*.sav"]);
    assert_eq!(exclude, vec!["*.log"]);
}

#[test]
fn empty_include_anywhere_means_everything() {
    let a = colliding(&["*.sav"], &[], rule("r"));
    let b = colliding(&[], &[], llm());
    let (out, _) = merge(vec![a, b]);
    assert_eq!(globs(&out[0]), (vec![], vec![]));
}

#[test]
fn different_ids_are_not_merged_and_come_out_by_id() {
    let fs: Vec<Finding> = ["A", "B", "C", "D"]
        .iter()
        .map(|n| {
            finding(
                fileset(&format!(r"{{APPDATA}}\{n}"), &[], &[]),
                Category::AppData,
                n,
                vec![ev(rule("r"), "k", 0.9)],
            )
        })
        .collect();
    let (out, issues) = merge(fs.clone());
    assert_eq!(out.len(), 4);
    assert!(issues.is_empty());
    let ids: Vec<&FindingId> = out.iter().map(|f| &f.id).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted);
    for f in &out {
        assert!(fs.contains(f));
    }
}

#[test]
fn non_finite_confidence_is_zero_with_bad_number_issue() {
    let a = foo(Category::Cache, vec![ev(heuristic("a"), "k", f32::NAN)]);
    let b = foo(Category::AppData, vec![ev(heuristic("b"), "k", 0.1)]);
    let (out, issues) = merge(vec![a, b]);
    assert_eq!(out[0].category, Category::AppData);
    assert!(out[0].evidence.iter().all(|e| e.confidence.is_finite()));
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].message_key, BAD_NUMBER_KEY);
    assert_eq!(issues[0].message_args["field"], "confidence");
    assert_eq!(issues[0].message_args["value"], "NaN");
}

#[test]
fn merge_by_id_drops_issues_but_keeps_the_tag() {
    let a = foo(Category::Cache, vec![ev(rule("r"), "k", 1.0)]);
    let b = foo(Category::Credentials, vec![ev(heuristic("h"), "k", 0.9)]);
    let out = merge_by_id(vec![a, b]);
    assert_eq!(out[0].tags, vec![CONFLICT_TAG.to_owned()]);
}

/// Heap's algorithm: every permutation of `items`.
fn permutations(items: Vec<Finding>) -> Vec<Vec<Finding>> {
    fn go(k: usize, items: &mut Vec<Finding>, out: &mut Vec<Vec<Finding>>) {
        if k <= 1 {
            out.push(items.clone());
            return;
        }
        for i in 0..k {
            go(k - 1, items, out);
            let j = if k.is_multiple_of(2) { i } else { 0 };
            items.swap(j, k - 1);
        }
    }
    let mut items = items;
    let mut out = Vec::new();
    let n = items.len();
    go(n, &mut items, &mut out);
    out
}

#[test]
fn result_does_not_depend_on_input_order() {
    let mut tie_a = foo(Category::AppData, vec![ev(heuristic("x"), "k", 0.8)]);
    tie_a.app = Some(app("a"));
    let mut tie_b = foo(Category::AppData, vec![ev(heuristic("y"), "k", 0.8)]);
    tie_b.app = Some(app("b"));
    tie_b.stats = Some(stats(3));
    let mut tie_c = foo(Category::AppData, vec![ev(heuristic("x"), "k", 0.8)]);
    tie_c.evidence[0]
        .message_args
        .insert("n".to_owned(), "1".to_owned());
    let save = foo(Category::GameSave, vec![ev(heuristic("z"), "k", 0.8)]);
    let other = finding(
        fileset(r"{APPDATA}\Bar", &[], &[]),
        Category::Cache,
        "Bar",
        vec![ev(rule("r"), "k", 0.5)],
    );
    let all = vec![tie_a, tie_b, tie_c, save, other];

    let perms = permutations(all);
    assert_eq!(perms.len(), 120);
    let expected = merge(perms[0].clone());
    for p in perms {
        assert_eq!(merge(p), expected);
    }
}
