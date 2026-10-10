//! `HeuristicsCollector`: the `heuristics` post-collector (SPEC-07 §4.1).

use std::sync::Arc;

use async_trait::async_trait;
use sk_core::collector::{CollectContext, CollectOutput, PostCollector, PriorResults};
use sk_core::config::{HeuristicToggles, HeuristicsConfig};
use sk_core::error::CollectorError;
use sk_core::events::{Event, ScanPhase, ThrottledSink};
use sk_core::model::{Finding, FolderSummary};
use sk_core::path::PathSet;

use crate::heuristic::{Heuristic, HeuristicContext};

#[cfg(test)]
#[path = "collector_tests.rs"]
mod tests;

/// `PostCollector::id` of the collector.
const COLLECTOR_ID: &str = "heuristics";

/// Order in which the heuristics run (SPEC-07 §4.1).
const ORDER: [&str; 5] = ["h-git", "h-web", "h-junk", "h-unk", "h-usr"];

/// Finds what the other collectors did not explain (SPEC-07): runs the
/// heuristics one after another, each seeing the paths claimed by the
/// collectors and by the heuristics before it.
pub struct HeuristicsCollector {
    cfg: HeuristicsConfig,
    /// Sorted by [`ORDER`].
    heuristics: Arc<[Box<dyn Heuristic>]>,
}

impl HeuristicsCollector {
    /// The collector with the built-in heuristics, configured by `cfg`.
    pub fn new(cfg: HeuristicsConfig) -> Self {
        // The heuristics come with T-07-05 (h-unk), T-07-06 (h-web),
        // T-07-08 (h-usr) and T-07-09 (h-git).
        Self::with_heuristics(cfg, Vec::new())
    }

    /// The collector with the given heuristics, put in the order of SPEC-07
    /// §4.1; heuristics with other ids run last, in the given order.
    pub(crate) fn with_heuristics(
        cfg: HeuristicsConfig,
        mut heuristics: Vec<Box<dyn Heuristic>>,
    ) -> Self {
        heuristics.sort_by_key(|h| rank(h.id()));
        Self {
            cfg,
            heuristics: heuristics.into(),
        }
    }
}

#[async_trait]
impl PostCollector for HeuristicsCollector {
    fn id(&self) -> &'static str {
        COLLECTOR_ID
    }

    /// Runs the switched-on heuristics on a blocking thread. When the scan is
    /// cancelled the remaining heuristics are skipped and the output holds
    /// what was found so far.
    async fn collect(
        &self,
        ctx: &CollectContext,
        prior: &PriorResults<'_>,
    ) -> Result<CollectOutput, CollectorError> {
        let heuristics = Arc::clone(&self.heuristics);
        let cfg = self.cfg.clone();
        let ctx = ctx.clone();
        let findings = prior.findings.to_vec();
        let claimed = prior.claimed.clone();
        let task =
            tokio::task::spawn_blocking(move || run(&heuristics, &cfg, &ctx, findings, claimed));
        match task.await {
            // The summaries of the `Unknown` findings are not carried by
            // `CollectOutput` yet; they reach `ScanReport::unknown_summaries`
            // with H-UNK (FR-07-08, T-07-05).
            Ok(collected) => Ok(collected.output),
            // The engine reports a panicking post-collector (`collector.panicked`).
            Err(err) if err.is_panic() => std::panic::resume_unwind(err.into_panic()),
            Err(err) => Err(CollectorError::Other(format!(
                "heuristics task did not finish: {err}"
            ))),
        }
    }
}

/// Merged output of the heuristics.
#[derive(Debug, Default)]
pub(crate) struct Collected {
    /// Findings, claimed paths and issues of all heuristics.
    pub(crate) output: CollectOutput,
    /// Summaries of the `Unknown` findings, for the LLM.
    pub(crate) unknown: Vec<FolderSummary>,
}

/// Runs the heuristics switched on in `cfg`, in order. Each one sees the
/// findings and claimed paths of the collectors (`findings`, `claimed`)
/// extended by those of the heuristics before it.
pub(crate) fn run(
    heuristics: &[Box<dyn Heuristic>],
    cfg: &HeuristicsConfig,
    ctx: &CollectContext,
    mut findings: Vec<Finding>,
    mut claimed: PathSet,
) -> Collected {
    let sink = ThrottledSink::new(ctx.events.clone());
    let progress = |done: u64, current: Option<&str>| {
        sink.send(Event::Progress {
            phase: ScanPhase::Heuristics,
            done,
            total: None,
            current: current.map(str::to_owned),
        });
    };
    let prior_count = findings.len();
    let mut collected = Collected::default();
    for heuristic in heuristics.iter().filter(|h| enabled(&cfg.enabled, h.id())) {
        if ctx.cancel.is_cancelled() {
            break;
        }
        let hctx = HeuristicContext {
            env: &ctx.env,
            scanner: ctx.scanner.as_ref(),
            claimed: &claimed,
            prior: &findings,
            cfg,
            cancel: &ctx.cancel,
            progress: &progress,
        };
        let out = heuristic.run(&hctx);
        claimed.extend(out.claimed.iter().cloned());
        collected.output.claimed_paths.extend(out.claimed);
        findings.extend(out.findings);
        collected.output.issues.extend(out.issues);
        collected.unknown.extend(out.unknown);
    }
    collected.output.findings = findings.split_off(prior_count);
    collected
}

/// Position of the heuristic `id` in [`ORDER`]; unknown ids go last.
fn rank(id: &str) -> usize {
    ORDER.iter().position(|o| *o == id).unwrap_or(ORDER.len())
}

/// Whether the heuristic `id` is switched on (FR-07-09); ids other than the
/// five of SPEC-07 always run.
fn enabled(toggles: &HeuristicToggles, id: &str) -> bool {
    match id {
        "h-unk" => toggles.unk,
        "h-usr" => toggles.usr,
        "h-git" => toggles.git,
        "h-junk" => toggles.junk,
        "h-web" => toggles.web,
        _ => true,
    }
}
