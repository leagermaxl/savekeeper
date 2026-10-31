//! Source priority and finding authority (SPEC-09 §4.2).

use std::cmp::Ordering;

use sk_core::model::{Evidence, EvidenceSource, Finding};

/// Priority of an evidence source (SPEC-09 §4.2): `User` 6, `Rule` 5,
/// `Ludusavi`/`Launcher` 4, `System` 3, `Heuristic` 2, `Llm` 1.
pub fn source_priority(source: &EvidenceSource) -> u8 {
    match source {
        EvidenceSource::User => 6,
        EvidenceSource::Rule { .. } => 5,
        EvidenceSource::Ludusavi { .. } | EvidenceSource::Launcher { .. } => 4,
        EvidenceSource::System { .. } => 3,
        EvidenceSource::Heuristic { .. } => 2,
        EvidenceSource::Llm { .. } => 1,
    }
}

/// The `(priority, confidence)` tuple of a piece of evidence, compared
/// lexicographically (SPEC-09 §4.2).
///
/// `confidence` is normalised by [`Authority::new`]: NaN and infinities become
/// 0 (SPEC-09 §5), so the ordering is total. Values are otherwise kept as is.
/// Build it with [`Authority::new`] rather than a struct literal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Authority {
    /// Source priority, see [`source_priority`].
    pub priority: u8,
    /// Evidence confidence; finite.
    pub confidence: f32,
}

impl Authority {
    /// The lowest authority: a finding without evidence.
    pub const NONE: Authority = Authority {
        priority: 0,
        confidence: 0.0,
    };

    /// Builds an authority, treating a non-finite confidence as 0.
    pub fn new(priority: u8, confidence: f32) -> Authority {
        Authority {
            priority,
            // `-0.0` is folded into `0.0` so `Eq` and `Ord` agree.
            confidence: if confidence.is_finite() && confidence != 0.0 {
                confidence
            } else {
                0.0
            },
        }
    }
}

impl Eq for Authority {}

impl PartialOrd for Authority {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Authority {
    fn cmp(&self, other: &Self) -> Ordering {
        self.priority
            .cmp(&other.priority)
            .then_with(|| self.confidence.total_cmp(&other.confidence))
    }
}

/// Authority of one piece of evidence.
pub fn evidence_authority(evidence: &Evidence) -> Authority {
    Authority::new(source_priority(&evidence.source), evidence.confidence)
}

/// `authority(f)` = the maximum over the evidence of `(priority, confidence)`:
/// the most authoritative evidence of the finding (SPEC-09 §4.2).
///
/// A finding without evidence (a P3 violation) gets [`Authority::NONE`].
pub fn authority(finding: &Finding) -> Authority {
    finding
        .evidence
        .iter()
        .map(evidence_authority)
        .max()
        .unwrap_or(Authority::NONE)
}
