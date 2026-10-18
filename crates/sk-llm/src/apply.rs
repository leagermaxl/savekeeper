//! Applying a classification to a finding (SPEC-08 §4.6 step 8, FR-08-07).

use std::collections::BTreeMap;

use sk_core::model::{
    AppKind, AppRef, Category, Evidence, EvidenceSource, Finding, Sensitivity, Target,
};

use crate::{Classification, ClassifierInfo};

/// i18n key of the LLM evidence (SPEC-02 §2.5).
pub const LLM_EVIDENCE_KEY: &str = "evidence.llm";

/// Applies a validated [`Classification`] to `finding` (FR-08-07, §4.6 step 8).
///
/// Always adds an [`Evidence`] with source [`EvidenceSource::Llm`], key
/// [`LLM_EVIDENCE_KEY`], args `reason` (and `app_guess` when there is one),
/// the classification confidence and importance: the reason is visible to
/// the user even when nothing else changes.
///
/// The finding is changed only when it is still [`Category::Unknown`] (the
/// LLM never overrides rules or Ludusavi), the classification category is
/// not `Unknown` and `confidence >= min_confidence_to_apply`. Then:
/// - `category` is set; `Credentials` raises `sensitivity` to `High`;
/// - `app` is set from `app_guess` if the finding has no app;
/// - for a [`Target::FileSet`], an empty `include` takes the suggested
///   include globs, and the suggested exclude globs are added to `exclude`
///   (without duplicates).
pub fn apply_classification(
    finding: &mut Finding,
    classification: &Classification,
    info: &ClassifierInfo,
    min_confidence_to_apply: f32,
) {
    finding.evidence.push(evidence(classification, info));

    let applies = finding.category == Category::Unknown
        && classification.category != Category::Unknown
        && classification.confidence >= min_confidence_to_apply;
    if !applies {
        return;
    }

    finding.category = classification.category;
    if classification.category == Category::Credentials {
        finding.sensitivity = Sensitivity::High;
    }
    if finding.app.is_none() {
        finding.app = classification
            .app_guess
            .as_deref()
            .and_then(|name| app_ref(name, classification.category));
    }
    if let Target::FileSet {
        include, exclude, ..
    } = &mut finding.target
    {
        if include.is_empty() {
            include.clone_from(&classification.suggested_include);
        }
        for glob in &classification.suggested_exclude {
            if !exclude.contains(glob) {
                exclude.push(glob.clone());
            }
        }
    }
}

fn evidence(c: &Classification, info: &ClassifierInfo) -> Evidence {
    let mut message_args = BTreeMap::new();
    message_args.insert("reason".to_owned(), c.reason.clone());
    if let Some(app) = &c.app_guess {
        message_args.insert("app_guess".to_owned(), app.clone());
    }
    Evidence {
        source: EvidenceSource::Llm {
            provider: info.provider.to_owned(),
            model: info.model.clone(),
        },
        message_key: LLM_EVIDENCE_KEY.to_owned(),
        message_args,
        confidence: c.confidence,
        importance: Some(c.importance),
    }
}

/// `AppRef` for a guessed name; `None` if the name has no letters or digits.
fn app_ref(name: &str, category: Category) -> Option<AppRef> {
    let id = slug(name);
    if id.is_empty() {
        return None;
    }
    let kind = match category {
        Category::GameSave | Category::GameConfig => AppKind::Game,
        Category::DevEnvironment => AppKind::DevTool,
        Category::SystemSettings => AppKind::System,
        _ => AppKind::Application,
    };
    Some(AppRef {
        id,
        name: name.to_owned(),
        kind,
        source_ids: BTreeMap::new(),
        installed: None,
        process_names: Vec::new(),
    })
}

/// `AppRef` id of a name: lowercase letters and digits, every other run of
/// characters becomes one `-`, without `-` at the ends.
fn slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_owned()
}

#[cfg(test)]
#[path = "apply_tests.rs"]
mod tests;
