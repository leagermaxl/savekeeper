//! Recommended models for the settings screen (SPEC-08 §4.11): the built-in
//! list `data/recommended.yaml` and the `ollama pull` hint.

use serde::{Deserialize, Serialize};

use crate::ollama::same_model;
use crate::{ANTHROPIC_PROVIDER, OLLAMA_PROVIDER, OPENAI_COMPAT_PROVIDER};

/// The built-in list (§4.11).
pub const RECOMMENDED_YAML: &str = include_str!("../data/recommended.yaml");

/// Prefix of the UI text of [`RecommendedModel::when`]: `llm.recommended.<when>`.
pub const RECOMMENDED_TEXT_PREFIX: &str = "llm.recommended.";

/// Provider of a recommended model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecommendedProvider {
    /// Ollama (`llm.local.kind = ollama`).
    Ollama,
    /// LM Studio, llama.cpp `llama-server` (`llm.local.kind = openai_compat`).
    OpenaiCompat,
    /// Anthropic (`llm.cloud.kind = anthropic`).
    Anthropic,
}

impl RecommendedProvider {
    /// The matching [`ClassifierInfo::provider`](crate::ClassifierInfo::provider).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ollama => OLLAMA_PROVIDER,
            Self::OpenaiCompat => OPENAI_COMPAT_PROVIDER,
            Self::Anthropic => ANTHROPIC_PROVIDER,
        }
    }
}

/// One entry of `recommended.yaml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct RecommendedModel {
    /// Provider of the model.
    pub provider: RecommendedProvider,
    /// Exact model id; `None` when any model fits (the `openai_compat` entry).
    #[serde(default)]
    pub model: Option<String>,
    /// The default model of the provider (§4.4).
    #[serde(default)]
    pub default: bool,
    /// When to pick it; the UI text is `llm.recommended.<when>`.
    pub when: String,
    /// VRAM the model needs on a GPU, in GB; `None` when a CPU is enough or it does not apply.
    #[serde(default)]
    pub min_vram_gb: Option<u32>,
}

/// A recommended model together with what the UI shows next to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct Recommendation {
    /// The entry of the list.
    #[serde(flatten)]
    pub model: RecommendedModel,
    /// Ollama entries: the model is installed on the server. `None` when this
    /// is unknown (no model list was given, or not an Ollama entry).
    pub installed: Option<bool>,
    /// Ollama entries that are not known to be installed: `ollama pull <model>`.
    pub pull_command: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecommendedFile {
    models: Vec<RecommendedModel>,
}

/// Parses a list in the format of `recommended.yaml`.
pub(crate) fn parse_recommended(text: &str) -> Result<Vec<RecommendedModel>, String> {
    serde_saphyr::from_str::<RecommendedFile>(text)
        .map(|file| file.models)
        .map_err(|e| e.to_string())
}

/// The built-in list in file order; empty (with a log line) if it does not
/// parse, which the unit tests rule out.
pub fn recommended_models() -> Vec<RecommendedModel> {
    parse_recommended(RECOMMENDED_YAML).unwrap_or_else(|error| {
        tracing::warn!(%error, "recommended.yaml not parsed");
        Vec::new()
    })
}

/// The command that installs an Ollama model: `ollama pull <model>`.
/// The program never runs it (§4.11).
pub fn ollama_pull_command(model: &str) -> String {
    format!("ollama pull {}", model.trim())
}

/// The built-in list with install state and the `ollama pull` hint.
///
/// `ollama_models` are the models of the Ollama server (`/api/tags`, e.g.
/// [`DetectedProvider::models`](crate::DetectedProvider::models)), or `None`
/// if the server was not reached. Names without a tag mean `:latest` and are
/// compared case-insensitively. Every Ollama entry that is not known to be
/// installed gets a [`Recommendation::pull_command`].
pub fn recommendations(ollama_models: Option<&[String]>) -> Vec<Recommendation> {
    recommended_models()
        .into_iter()
        .map(|model| annotate(model, ollama_models))
        .collect()
}

fn annotate(model: RecommendedModel, ollama_models: Option<&[String]>) -> Recommendation {
    let (installed, pull_command) = match (&model.provider, &model.model) {
        (RecommendedProvider::Ollama, Some(name)) => {
            let installed = ollama_models.map(|models| models.iter().any(|m| same_model(m, name)));
            let pull = (installed != Some(true)).then(|| ollama_pull_command(name));
            (installed, pull)
        }
        _ => (None, None),
    };
    Recommendation {
        model,
        installed,
        pull_command,
    }
}

#[cfg(test)]
#[path = "recommended_tests.rs"]
mod tests;
