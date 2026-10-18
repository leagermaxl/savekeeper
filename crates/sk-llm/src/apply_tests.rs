use std::path::PathBuf;

use sk_core::model::{
    AppKind, AppRef, Category, EvidenceSource, Finding, FindingId, Sensitivity, Target,
};
use sk_core::template::PathTemplate;

use super::*;
use crate::{Classification, ClassifierInfo};

fn info() -> ClassifierInfo {
    ClassifierInfo {
        provider: "ollama",
        model: "qwen2.5:7b-instruct".to_owned(),
        is_local: true,
        max_batch: 8,
        max_concurrency: 1,
    }
}

fn unknown_finding(exclude: &[&str]) -> Finding {
    let root = match PathTemplate::parse("{APPDATA}/obs-studio") {
        Ok(t) => t,
        Err(e) => panic!("bad template: {e}"),
    };
    let target = Target::FileSet {
        root,
        resolved: PathBuf::from("C:/Users/u/AppData/Roaming/obs-studio"),
        include: Vec::new(),
        exclude: exclude.iter().map(|s| (*s).to_owned()).collect(),
    };
    Finding {
        id: FindingId::for_target(&target),
        target,
        category: Category::Unknown,
        app: None,
        title: "obs-studio".to_owned(),
        evidence: Vec::new(),
        stats: None,
        sensitivity: Sensitivity::None,
        score: None,
        default_selected: false,
        requires_elevation: false,
        tags: Vec::new(),
        children: Vec::new(),
        notes_key: None,
    }
}

fn classification(category: Category, confidence: f32) -> Classification {
    Classification {
        id: "f".to_owned(),
        category,
        app_guess: Some("OBS Studio".to_owned()),
        importance: 0.6,
        confidence,
        reason: "Scene collections and profiles.".to_owned(),
        suggested_include: vec!["basic/**".to_owned()],
        suggested_exclude: vec!["logs/**".to_owned(), "crashes/**".to_owned()],
        from_cache: false,
    }
}

fn file_set(f: &Finding) -> (&[String], &[String]) {
    match &f.target {
        Target::FileSet {
            include, exclude, ..
        } => (include, exclude),
        other => panic!("not a file set: {other:?}"),
    }
}

#[test]
fn confident_answer_updates_the_finding() {
    let mut f = unknown_finding(&["logs/**"]);
    apply_classification(
        &mut f,
        &classification(Category::AppData, 0.9),
        &info(),
        0.5,
    );

    assert_eq!(f.category, Category::AppData);
    let app = f
        .app
        .as_ref()
        .map(|a: &AppRef| (a.id.as_str(), a.name.as_str(), a.kind));
    assert_eq!(
        app,
        Some(("obs-studio", "OBS Studio", AppKind::Application))
    );
    let (include, exclude) = file_set(&f);
    assert_eq!(include, ["basic/**".to_owned()]);
    assert_eq!(exclude, ["logs/**".to_owned(), "crashes/**".to_owned()]);

    assert_eq!(f.evidence.len(), 1);
    let ev = &f.evidence[0];
    assert_eq!(
        ev.source,
        EvidenceSource::Llm {
            provider: "ollama".to_owned(),
            model: "qwen2.5:7b-instruct".to_owned(),
        }
    );
    assert_eq!(ev.message_key, "evidence.llm");
    assert_eq!(
        ev.message_args.get("reason").map(String::as_str),
        Some("Scene collections and profiles.")
    );
    assert_eq!(
        ev.message_args.get("app_guess").map(String::as_str),
        Some("OBS Studio")
    );
    assert_eq!(ev.confidence, 0.9);
    assert_eq!(ev.importance, Some(0.6));
}

#[test]
fn low_confidence_keeps_unknown_but_adds_evidence() {
    let mut f = unknown_finding(&[]);
    apply_classification(
        &mut f,
        &classification(Category::AppData, 0.49),
        &info(),
        0.5,
    );
    assert_eq!(f.category, Category::Unknown);
    assert_eq!(f.app, None);
    assert_eq!(file_set(&f), (&[][..], &[][..]));
    assert_eq!(f.evidence.len(), 1);
    assert_eq!(f.evidence[0].confidence, 0.49);
}

#[test]
fn confidence_equal_to_threshold_applies() {
    let mut f = unknown_finding(&[]);
    apply_classification(&mut f, &classification(Category::Cache, 0.5), &info(), 0.5);
    assert_eq!(f.category, Category::Cache);
}

#[test]
fn unknown_answer_changes_nothing_but_evidence() {
    let mut f = unknown_finding(&[]);
    let mut c = classification(Category::Unknown, 1.0);
    c.app_guess = None;
    apply_classification(&mut f, &c, &info(), 0.5);
    assert_eq!(f.category, Category::Unknown);
    assert_eq!(f.app, None);
    assert_eq!(f.evidence.len(), 1);
    assert!(!f.evidence[0].message_args.contains_key("app_guess"));
}

#[test]
fn recognized_finding_is_not_overridden() {
    let mut f = unknown_finding(&[]);
    f.category = Category::GameSave;
    apply_classification(&mut f, &classification(Category::Cache, 1.0), &info(), 0.5);
    assert_eq!(f.category, Category::GameSave);
    assert_eq!(f.app, None);
    assert_eq!(file_set(&f), (&[][..], &[][..]));
    assert_eq!(f.evidence.len(), 1);
}

#[test]
fn existing_include_and_app_are_kept() {
    let mut f = unknown_finding(&[]);
    if let Target::FileSet { include, .. } = &mut f.target {
        include.push("profiles/**".to_owned());
    }
    let existing = AppRef {
        id: "obs".to_owned(),
        name: "OBS".to_owned(),
        kind: AppKind::Application,
        source_ids: Default::default(),
        installed: Some(true),
        process_names: Vec::new(),
    };
    f.app = Some(existing.clone());
    apply_classification(
        &mut f,
        &classification(Category::AppConfig, 0.9),
        &info(),
        0.5,
    );
    assert_eq!(f.app, Some(existing));
    assert_eq!(file_set(&f).0, ["profiles/**".to_owned()]);
}

#[test]
fn credentials_raise_sensitivity_and_game_kind_follows_category() {
    let mut f = unknown_finding(&[]);
    apply_classification(
        &mut f,
        &classification(Category::Credentials, 0.9),
        &info(),
        0.5,
    );
    assert_eq!(f.sensitivity, Sensitivity::High);

    let mut g = unknown_finding(&[]);
    let mut c = classification(Category::GameSave, 0.9);
    c.app_guess = Some("  Elden Ring!  ".to_owned());
    apply_classification(&mut g, &c, &info(), 0.5);
    let app = g.app.as_ref().map(|a| (a.id.as_str(), a.kind));
    assert_eq!(app, Some(("elden-ring", AppKind::Game)));
}

#[test]
fn guess_without_letters_gives_no_app() {
    let mut f = unknown_finding(&[]);
    let mut c = classification(Category::AppData, 0.9);
    c.app_guess = Some("???".to_owned());
    apply_classification(&mut f, &c, &info(), 0.5);
    assert_eq!(f.category, Category::AppData);
    assert_eq!(f.app, None);
}
