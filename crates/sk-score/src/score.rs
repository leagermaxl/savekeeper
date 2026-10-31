//! The importance score of a finding (SPEC-09 §4.5).

use std::collections::BTreeMap;

use sk_core::config::{default_weights, CategoryWeights, ScoringConfig};
use sk_core::env::Environment;
use sk_core::model::{Category, Evidence, EvidenceSource, Finding, ScanIssue, Score};
use time::OffsetDateTime;

use crate::authority::{authority, source_priority};
use crate::issues::{target_path, warning, BAD_NUMBER_KEY};
use crate::nested::VIA_ARG;

/// `components` key of I, the irreplaceability of the category.
pub const IRREPLACEABILITY: &str = "irreplaceability";
/// `components` key of A, the share created by the user.
pub const USER_AUTHORED: &str = "user_authored";
/// `components` key of R, the recency of the newest file.
pub const RECENCY: &str = "recency";
/// `components` key of C, the confidence of the most authoritative evidence.
pub const CONFIDENCE: &str = "confidence";
/// `components` key of S, the size penalty.
pub const SIZE_PENALTY: &str = "size_penalty";
/// `components` key of Adj, the signed sum of bonuses and penalties.
pub const ADJUSTMENTS: &str = "adjustments";
/// `components` key of `base = 0.55·I + 0.25·A + 0.20·R`.
pub const BASE: &str = "base";
/// `components` key of the final value.
pub const VALUE: &str = "value";

/// `message_args` key of the LLM importance carried as a string (SPEC-08 §9).
pub const IMPORTANCE_ARG: &str = "importance";

/// Tags that make the user's share A = 1.0.
const UNSAVED_GIT_TAGS: [&str; 2] = ["git-unsaved", "git-no-remote"];
/// Tag of a folder synced to a cloud.
const CLOUD_SYNCED_TAG: &str = "cloud-synced";

/// C of an `Unknown` category that does not come from the LLM: no source
/// vouches for the category, so neither 0 nor 1 is right.
const UNKNOWN_CONFIDENCE: f64 = 0.5;

const SECONDS_PER_DAY: f64 = 86_400.0;

/// Computes the score of a finding with all formula terms (SPEC-09 §4.5).
///
/// `value = clamp01((0.5 + 0.5·C)·(0.55·I + 0.25·A + 0.20·R) − S + Adj)`.
/// `components` holds `irreplaceability, user_authored, recency, confidence,
/// size_penalty, adjustments, base, value`; every value is rounded to three
/// decimals after `value` is computed. `Cache`/`Reinstallable` are scored too.
///
/// A is 1.0 for the tags `git-unsaved`/`git-no-remote`; otherwise the LLM
/// importance when the category comes from the LLM (see below); otherwise the
/// `authored` weight of the category. The category comes from the LLM when
/// the finding has its own `Llm` evidence with an importance (not one taken
/// over from a nested child, `message_args["via"]`) and no evidence from a
/// source that assigns categories itself (`User`, `Rule`, `Ludusavi`,
/// `Launcher`, `System`): the LLM never overrides those (SPEC-08 §4.6). The
/// importance is `Evidence.importance` (SPEC-02 §2.5), else the number in
/// `message_args["importance"]`; of several, the one of the most confident
/// evidence (ties: the higher importance). NaN, infinite or unparsable
/// importance counts as 0 (§5); its `scoring.bad_number` issue is reported by
/// `run()`, this function drops it.
///
/// C, first applicable: the confidence of that same LLM evidence when the
/// category comes from the LLM; 0.5 for an `Unknown` category not from the
/// LLM; otherwise `authority(f).confidence` (§4.2). Heuristic candidates the
/// LLM classified keep their own low-confidence evidence, which outranks the
/// LLM in §4.2 but no longer describes the category.
pub fn score(f: &Finding, env: &Environment, now: OffsetDateTime, cfg: &ScoringConfig) -> Score {
    let mut issues = Vec::new();
    score_reporting(f, env, now, cfg, &mut issues)
}

/// [`score`] that appends a `scoring.bad_number` issue to `issues` when the
/// LLM importance it uses is not a finite number.
pub(crate) fn score_reporting(
    f: &Finding,
    _env: &Environment,
    now: OffsetDateTime,
    cfg: &ScoringConfig,
    issues: &mut Vec<ScanIssue>,
) -> Score {
    let weights = category_weights(cfg, f.category);

    let i = unit(f64::from(weights.irreplaceability));
    let verdict = llm_verdict(f);
    let a = user_authored(f, verdict, &weights, issues);
    let r = recency(f, now);
    let c = confidence(f, verdict);
    let s = size_penalty(f, &weights, cfg);
    let adj = adjustments(f, cfg);

    let c_eff = 0.5 + 0.5 * c;
    let base = 0.55 * i + 0.25 * a + 0.20 * r;
    let value = unit(c_eff * base - s + adj);

    let components = BTreeMap::from([
        (IRREPLACEABILITY.to_owned(), round3(i)),
        (USER_AUTHORED.to_owned(), round3(a)),
        (RECENCY.to_owned(), round3(r)),
        (CONFIDENCE.to_owned(), round3(c)),
        (SIZE_PENALTY.to_owned(), round3(s)),
        (ADJUSTMENTS.to_owned(), round3(adj)),
        (BASE.to_owned(), round3(base)),
        (VALUE.to_owned(), round3(value)),
    ]);
    Score {
        value: round3(value),
        components,
    }
}

/// Weights of a category; a category missing from the config takes the
/// default of §4.6.
fn category_weights(cfg: &ScoringConfig, category: Category) -> CategoryWeights {
    cfg.category_weights
        .get(&category)
        .copied()
        .or_else(|| default_weights().get(&category).copied())
        .unwrap_or(CategoryWeights {
            irreplaceability: 0.0,
            authored: 0.0,
            size_tolerant: false,
        })
}

/// A: the share created by the user.
fn user_authored(
    f: &Finding,
    verdict: Option<&Evidence>,
    weights: &CategoryWeights,
    issues: &mut Vec<ScanIssue>,
) -> f64 {
    if f.tags
        .iter()
        .any(|t| UNSAVED_GIT_TAGS.contains(&t.as_str()))
    {
        return 1.0;
    }
    match verdict.and_then(Importance::of) {
        Some(importance) => {
            if let Importance::Bad(raw) = &importance {
                issues.push(warning(
                    BAD_NUMBER_KEY,
                    target_path(&f.target),
                    &[
                        ("finding_id", f.id.as_str().to_owned()),
                        ("field", IMPORTANCE_ARG.to_owned()),
                        ("value", raw.clone()),
                    ],
                ));
            }
            importance.value()
        }
        None => unit(f64::from(weights.authored)),
    }
}

/// C: the confidence of the LLM verdict when the category comes from the LLM;
/// 0.5 for an `Unknown` category not from the LLM; otherwise the confidence
/// of the most authoritative evidence (§4.2).
fn confidence(f: &Finding, verdict: Option<&Evidence>) -> f64 {
    if let Some(e) = verdict {
        return unit(f64::from(e.confidence));
    }
    if f.category == Category::Unknown {
        return UNKNOWN_CONFIDENCE;
    }
    unit(f64::from(authority(f).confidence))
}

/// Importance read from one evidence: the typed field, else the string arg.
enum Importance {
    Number(f64),
    /// Not a finite number; the raw text for the issue.
    Bad(String),
}

impl Importance {
    fn of(evidence: &Evidence) -> Option<Importance> {
        if let Some(v) = evidence.importance {
            return Some(if v.is_finite() {
                Importance::Number(f64::from(v))
            } else {
                Importance::Bad(v.to_string())
            });
        }
        let raw = evidence.message_args.get(IMPORTANCE_ARG)?;
        Some(match raw.trim().parse::<f64>() {
            Ok(v) if v.is_finite() => Importance::Number(v),
            _ => Importance::Bad(raw.clone()),
        })
    }

    fn value(&self) -> f64 {
        match self {
            Importance::Number(v) => unit(*v),
            Importance::Bad(_) => 0.0,
        }
    }
}

/// The LLM evidence the category comes from, see [`score`]; `None` when the
/// category does not come from the LLM.
fn llm_verdict(f: &Finding) -> Option<&Evidence> {
    let llm_priority = source_priority(&EvidenceSource::Llm {
        provider: String::new(),
        model: String::new(),
    });
    let heuristic_priority = source_priority(&EvidenceSource::Heuristic {
        heuristic_id: String::new(),
    });
    let own = || {
        f.evidence
            .iter()
            .filter(|e| !e.message_args.contains_key(VIA_ARG))
    };
    if own().any(|e| source_priority(&e.source) > heuristic_priority) {
        return None;
    }
    own()
        .filter(|e| source_priority(&e.source) == llm_priority)
        .filter_map(|e| Importance::of(e).map(|i| (e, i.value())))
        .max_by(|(ea, ia), (eb, ib)| {
            unit(f64::from(ea.confidence))
                .total_cmp(&unit(f64::from(eb.confidence)))
                .then_with(|| ia.total_cmp(ib))
        })
        .map(|(e, _)| e)
}

/// R: 1.0 up to 30 days, then `0.3 + 0.7·e^(−(d−30)/365)`; 0.5 without data.
/// A modification time in the future counts as `d = 0`.
fn recency(f: &Finding, now: OffsetDateTime) -> f64 {
    let Some(mtime) = f.stats.as_ref().and_then(|s| s.newest_mtime) else {
        return 0.5;
    };
    let days = ((now - mtime).as_seconds_f64() / SECONDS_PER_DAY).max(0.0);
    if days <= 30.0 {
        1.0
    } else {
        0.3 + 0.7 * (-(days - 30.0) / 365.0).exp()
    }
}

/// S: `min(cap, 0.1·log2(b / size_free_bytes))` above the free size, halved
/// for size-tolerant categories; 0 without stats.
fn size_penalty(f: &Finding, weights: &CategoryWeights, cfg: &ScoringConfig) -> f64 {
    let Some(stats) = &f.stats else {
        return 0.0;
    };
    if stats.total_bytes <= cfg.size_free_bytes {
        return 0.0;
    }
    let cap = unit(f64::from(cfg.size_penalty_cap));
    // A free size of 0 gives an infinite ratio, so the penalty is the cap.
    let ratio = stats.total_bytes as f64 / cfg.size_free_bytes as f64;
    let penalty = (0.1 * ratio.log2()).min(cap);
    if weights.size_tolerant {
        penalty * 0.5
    } else {
        penalty
    }
}

/// Adj: installed bonus, uninstalled penalty (app categories only) and the
/// cloud-synced penalty.
fn adjustments(f: &Finding, cfg: &ScoringConfig) -> f64 {
    let mut adj = 0.0;
    match f.app.as_ref().and_then(|a| a.installed) {
        Some(true) => adj += unit(f64::from(cfg.installed_bonus)),
        Some(false)
            if matches!(
                f.category,
                Category::AppConfig | Category::AppData | Category::GameConfig
            ) =>
        {
            adj -= unit(f64::from(cfg.uninstalled_penalty));
        }
        _ => {}
    }
    if f.tags.iter().any(|t| t == CLOUD_SYNCED_TAG) {
        adj -= unit(f64::from(cfg.cloud_synced_penalty));
    }
    adj
}

/// Clamps to `[0, 1]`; NaN becomes 0.
fn unit(x: f64) -> f64 {
    if x.is_nan() {
        0.0
    } else {
        x.clamp(0.0, 1.0)
    }
}

/// Rounds to three decimals: `(x·1000).round()/1000`.
fn round3(x: f64) -> f32 {
    ((x * 1000.0).round() / 1000.0) as f32
}
