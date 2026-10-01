//! Evidence: why something is a finding (SPEC-02 §2.5).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use specta::Type;

/// One reason for a finding.
///
/// For the LLM, `message_key = "evidence.llm"` and the explanation is in
/// `message_args["reason"]` (not translated).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub struct Evidence {
    /// Who produced the evidence.
    pub source: EvidenceSource,
    /// i18n key, e.g. "evidence.rule_match".
    pub message_key: String,
    /// Arguments for the message.
    pub message_args: BTreeMap<String, String>,
    /// Confidence, `0.0..=1.0`.
    pub confidence: f32,
    /// How painful the loss would be, `0.0..=1.0`; only the LLM sets it now
    /// (SPEC-08). Input for SPEC-09.
    pub importance: Option<f32>,
}

/// Source of the evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EvidenceSource {
    /// A known-location rule (SPEC-04).
    Rule {
        /// Rule identifier.
        rule_id: String,
    },
    /// The Ludusavi manifest (SPEC-05).
    Ludusavi {
        /// Game name in the manifest.
        game: String,
        /// Manifest version.
        manifest_version: String,
    },
    /// A game launcher (SPEC-05): steam, epic, gog ...
    Launcher {
        /// Launcher identifier.
        launcher: String,
    },
    /// A system exporter (SPEC-06).
    System {
        /// Exporter identifier.
        exporter_id: String,
    },
    /// A heuristic (SPEC-07).
    Heuristic {
        /// Heuristic identifier.
        heuristic_id: String,
    },
    /// The LLM classifier (SPEC-08).
    Llm {
        /// Provider identifier.
        provider: String,
        /// Model name.
        model: String,
    },
    /// Added by the user manually.
    User,
}
