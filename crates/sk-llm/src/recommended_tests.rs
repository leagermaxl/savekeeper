use std::collections::HashSet;

use sk_core::config::{CloudLlmConfig, LocalLlmConfig};

use super::*;

fn models_of(provider: RecommendedProvider) -> Vec<String> {
    recommended_models()
        .into_iter()
        .filter(|m| m.provider == provider)
        .filter_map(|m| m.model)
        .collect()
}

#[test]
fn builtin_list_parses_and_matches_the_spec_table() {
    assert!(parse_recommended(RECOMMENDED_YAML).is_ok());
    assert_eq!(
        models_of(RecommendedProvider::Ollama),
        [
            "qwen2.5:7b-instruct",
            "qwen2.5:3b-instruct",
            "llama3.2:3b",
            "qwen2.5:14b-instruct",
            "qwen3.5:9b",
            "qwen3.5:4b",
        ]
    );
    assert_eq!(
        models_of(RecommendedProvider::Anthropic),
        ["claude-haiku-4-5", "claude-sonnet-5"]
    );
    let compat: Vec<RecommendedModel> = recommended_models()
        .into_iter()
        .filter(|m| m.provider == RecommendedProvider::OpenaiCompat)
        .collect();
    assert_eq!(compat.len(), 1);
    assert_eq!(compat[0].model, None, "any model with JSON schema support");
}

#[test]
fn defaults_match_the_config_defaults() {
    let defaults: Vec<(RecommendedProvider, Option<String>)> = recommended_models()
        .into_iter()
        .filter(|m| m.default)
        .map(|m| (m.provider, m.model))
        .collect();
    assert_eq!(
        defaults,
        vec![
            (
                RecommendedProvider::Ollama,
                Some(LocalLlmConfig::default().model)
            ),
            (
                RecommendedProvider::Anthropic,
                Some(CloudLlmConfig::default().model)
            ),
        ]
    );
}

#[test]
fn entries_are_unique_and_text_keys_are_valid() {
    let models = recommended_models();
    let mut seen = HashSet::new();
    for m in &models {
        assert!(
            seen.insert((m.provider, m.model.clone())),
            "duplicate {m:?}"
        );
        assert!(!m.when.is_empty());
        assert!(
            m.when
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "`when` is one i18n key segment: {}",
            m.when
        );
        if m.provider != RecommendedProvider::OpenaiCompat {
            assert!(m.model.is_some(), "{m:?}");
        }
    }
    assert_eq!(RecommendedProvider::Ollama.as_str(), "ollama");
    assert_eq!(RecommendedProvider::OpenaiCompat.as_str(), "openai_compat");
    assert_eq!(RecommendedProvider::Anthropic.as_str(), "anthropic");
}

#[test]
fn unknown_fields_are_rejected() {
    let text = "models:\n  - provider: ollama\n    model: a\n    when: gpu_6gb\n    size: 1\n";
    assert!(parse_recommended(text).is_err());
    assert!(parse_recommended("models:\n  - provider: other\n    when: x\n").is_err());
}

#[test]
fn pull_command_is_the_ollama_cli() {
    assert_eq!(
        ollama_pull_command(" qwen3.5:9b "),
        "ollama pull qwen3.5:9b"
    );
}

#[test]
fn recommendations_mark_installed_ollama_models() {
    let installed = vec!["Qwen3.5:9B".to_owned(), "llama3.2:3b".to_owned()];
    let recs = recommendations(Some(&installed));
    assert_eq!(recs.len(), recommended_models().len());
    let find = |name: &str| {
        recs.iter()
            .find(|r| r.model.model.as_deref() == Some(name))
            .cloned()
    };
    let qwen = find("qwen3.5:9b");
    assert_eq!(qwen.as_ref().and_then(|r| r.installed), Some(true));
    assert_eq!(qwen.and_then(|r| r.pull_command), None);
    let small = find("qwen3.5:4b");
    assert_eq!(small.as_ref().and_then(|r| r.installed), Some(false));
    assert_eq!(
        small.and_then(|r| r.pull_command).as_deref(),
        Some("ollama pull qwen3.5:4b")
    );
    let haiku = find("claude-haiku-4-5");
    assert_eq!(
        haiku
            .as_ref()
            .map(|r| (r.installed, r.pull_command.clone())),
        Some((None, None))
    );
}

#[test]
fn without_a_model_list_every_ollama_entry_gets_the_pull_hint() {
    for r in recommendations(None) {
        assert_eq!(r.installed, None);
        let expected = (r.model.provider == RecommendedProvider::Ollama)
            .then(|| r.model.model.as_deref().map(ollama_pull_command))
            .flatten();
        assert_eq!(r.pull_command, expected, "{r:?}");
    }
}

#[test]
fn recommendation_serializes_flat() {
    let recs = recommendations(Some(&[]));
    let value = match serde_json::to_value(&recs[0]) {
        Ok(v) => v,
        Err(e) => panic!("serialize: {e}"),
    };
    assert_eq!(
        value,
        serde_json::json!({
            "provider": "ollama", "model": "qwen2.5:7b-instruct", "default": true,
            "when": "gpu_6gb", "min_vram_gb": 6,
            "installed": false, "pull_command": "ollama pull qwen2.5:7b-instruct"
        })
    );
}
