//! `HeuristicsCollector`: the `heuristics` post-collector (SPEC-07 §4.1).

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use sk_core::collector::{CollectContext, CollectOutput, PostCollector, PriorResults};
use sk_core::config::{HeuristicToggles, HeuristicsConfig};
use sk_core::error::CollectorError;
use sk_core::events::{Event, ScanPhase, ThrottledSink};
use sk_core::model::{Finding, IssueSeverity, ScanIssue};
use sk_core::path::PathSet;
use sk_scan::ExcludeSet;

use crate::data::{self, DataError};
use crate::heuristic::{Heuristic, HeuristicContext};
use crate::unk::HUnk;
use crate::walk::RootsWalkCache;
use crate::web::HWeb;

#[cfg(test)]
#[path = "collector_tests.rs"]
mod tests;

/// `PostCollector::id` of the collector.
pub(crate) const COLLECTOR_ID: &str = "heuristics";

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
        // h-usr and h-git come with T-07-08 and T-07-09.
        Self::with_heuristics(cfg, vec![Box::new(HWeb::new()), Box::new(HUnk::new())])
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
            Ok(output) => Ok(output),
            // The engine reports a panicking post-collector (`collector.panicked`).
            Err(err) if err.is_panic() => std::panic::resume_unwind(err.into_panic()),
            Err(err) => Err(CollectorError::Other(format!(
                "heuristics task did not finish: {err}"
            ))),
        }
    }
}

/// Runs the heuristics switched on in `cfg`, in order. Each one sees the
/// findings and claimed paths of the collectors (`findings`, `claimed`)
/// extended by those of the heuristics before it.
///
/// The output holds the findings, claimed paths and issues of all heuristics
/// and the summaries of their `Unknown` findings (FR-07-08).
pub(crate) fn run(
    heuristics: &[Box<dyn Heuristic>],
    cfg: &HeuristicsConfig,
    ctx: &CollectContext,
    mut findings: Vec<Finding>,
    mut claimed: PathSet,
) -> CollectOutput {
    let sink = ThrottledSink::new(ctx.events.clone());
    let progress = |done: u64, current: Option<&str>| {
        sink.send(Event::Progress {
            phase: ScanPhase::Heuristics,
            done,
            total: None,
            current: current.map(str::to_owned),
        });
    };
    // Broken user globs are reported by the Measure phase (SPEC-01 §4.4).
    let excludes = Arc::new(
        ExcludeSet::with_user(&ctx.env, &ctx.config.scan.exclude_globs)
            .unwrap_or_else(|_| ExcludeSet::builtin(&ctx.env)),
    );
    // Walked by the first heuristic that needs it (SPEC-07 §4.1). The roots
    // of `ScanOptions` do not reach the collectors; `scan.extra_roots` does.
    let roots_walk = RootsWalkCache::new(
        ctx.config.scan.extra_roots.clone(),
        ctx.config.scan.max_depth,
    );
    let prior_count = findings.len();
    let mut output = CollectOutput::default();
    // The built-in data is parsed and validated once, when the first scan starts.
    if let Err(err) = data::builtin() {
        output.issues.push(data_issue(err));
    }
    for heuristic in heuristics.iter().filter(|h| enabled(&cfg.enabled, h.id())) {
        if ctx.cancel.is_cancelled() {
            break;
        }
        let hctx = HeuristicContext {
            env: &ctx.env,
            scanner: ctx.scanner.as_ref(),
            excludes: &excludes,
            claimed: &claimed,
            prior: &findings,
            cfg,
            cancel: &ctx.cancel,
            progress: &progress,
            roots_walk: &roots_walk,
        };
        let out = heuristic.run(&hctx);
        claimed.extend(out.claimed.iter().cloned());
        output.claimed_paths.extend(out.claimed);
        findings.extend(out.findings);
        output.issues.extend(out.issues);
        output.unknown_summaries.extend(out.unknown);
    }
    output.findings = findings.split_off(prior_count);
    output
}

/// The issue `heuristic.data_invalid` for broken built-in data (`vendors.txt`,
/// `extensions.yaml`); the heuristics still run, without that data.
pub(crate) fn data_issue(err: &DataError) -> ScanIssue {
    ScanIssue {
        severity: IssueSeverity::Error,
        source: COLLECTOR_ID.to_owned(),
        path: None,
        message_key: "heuristic.data_invalid".to_owned(),
        message_args: BTreeMap::from([("error".to_owned(), err.to_string())]),
    }
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
