//! `SystemCollector`: detection and planning of the system exports during a scan
//! (SPEC-06 §4.1, §4.2).
//!
//! One collection runs [`detect`](SystemExporter::detect) and
//! [`plan`](SystemExporter::plan) of every exporter in parallel, each in its own blocking
//! task, so the whole collector takes about as long as the slowest detection
//! (NFR-06-01: ≤ 3 s, FR-06-01: ≤ 500 ms per exporter). Nothing is exported here: the
//! export itself is run by the backup (SPEC-10).

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use sk_core::collector::{CollectContext, CollectOutput, Collector};
use sk_core::env::Environment;
use sk_core::error::CollectorError;
use sk_core::model::Finding;

use crate::exporter::{Availability, SystemExporter};
use crate::registry::registry;

/// `Collector::id` of the system collector.
const COLLECTOR_ID: &str = "system";

/// i18n key of the collector name.
const DISPLAY_KEY: &str = "collector.system";

/// Time budget of one exporter's `detect` (FR-06-01); exceeding it is logged.
const DETECT_BUDGET: Duration = Duration::from_millis(500);

/// Time budget of the whole collector (NFR-06-01); exceeding it is logged.
const COLLECT_BUDGET: Duration = Duration::from_secs(3);

/// Collector of the system exports (SPEC-06 §4.1): one `Target::SystemExport` finding per
/// exporter that is available on this machine.
pub struct SystemCollector {
    exporters: Vec<Arc<dyn SystemExporter>>,
}

impl SystemCollector {
    /// A collector over all exporters of SPEC-06 §4.3 ([`registry`]).
    pub fn new() -> Self {
        Self::with_exporters(registry())
    }

    /// A collector over the given exporters, e.g. fakes in tests.
    pub fn with_exporters(exporters: Vec<Arc<dyn SystemExporter>>) -> Self {
        Self { exporters }
    }
}

impl Default for SystemCollector {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Collector for SystemCollector {
    fn id(&self) -> &'static str {
        COLLECTOR_ID
    }

    fn display_key(&self) -> &'static str {
        DISPLAY_KEY
    }

    /// Runs `detect` + `plan` of every exporter in parallel and returns the planned
    /// findings in exporter order. Exporters listed in `config.system.disabled_exporters`
    /// are skipped; [`Availability::Unavailable`] gives no finding. No paths are claimed.
    /// When the scan is cancelled the output is empty. A panicking exporter is passed on
    /// to the engine (`collector.panicked`, SPEC-01 §4.4).
    async fn collect(&self, ctx: &CollectContext) -> Result<CollectOutput, CollectorError> {
        if ctx.cancel.is_cancelled() {
            return Ok(CollectOutput::default());
        }
        let started = Instant::now();
        let disabled = &ctx.config.system.disabled_exporters;
        let tasks: Vec<_> = self
            .exporters
            .iter()
            .filter(|e| !disabled.iter().any(|id| id == e.id()))
            .map(|exporter| {
                let exporter = Arc::clone(exporter);
                let env = Arc::clone(&ctx.env);
                tokio::task::spawn_blocking(move || plan_one(exporter.as_ref(), &env))
            })
            .collect();
        let joined = async {
            let mut results = Vec::with_capacity(tasks.len());
            for task in tasks {
                results.push(task.await);
            }
            results
        };
        let results = tokio::select! {
            biased;
            () = ctx.cancel.cancelled() => return Ok(CollectOutput::default()),
            results = joined => results,
        };

        let mut findings = Vec::new();
        for result in results {
            match result {
                Ok(finding) => findings.extend(finding),
                Err(err) if err.is_panic() => std::panic::resume_unwind(err.into_panic()),
                Err(err) => {
                    return Err(CollectorError::Other(format!(
                        "system exporter task did not finish: {err}"
                    )));
                }
            }
        }
        let elapsed = started.elapsed();
        if elapsed > COLLECT_BUDGET {
            tracing::warn!(?elapsed, budget = ?COLLECT_BUDGET, "system collector over budget");
        } else {
            tracing::debug!(?elapsed, findings = findings.len(), "system collector done");
        }
        Ok(CollectOutput {
            findings,
            ..CollectOutput::default()
        })
    }
}

/// `detect` and, unless the export is unavailable, `plan` of one exporter.
fn plan_one(exporter: &dyn SystemExporter, env: &Environment) -> Option<Finding> {
    let id = exporter.id();
    let started = Instant::now();
    let availability = exporter.detect(env);
    let elapsed = started.elapsed();
    if elapsed > DETECT_BUDGET {
        tracing::warn!(exporter = id, ?elapsed, "exporter detect over budget");
    }
    if let Availability::Unavailable { reason_key } = &availability {
        tracing::debug!(exporter = id, reason = %reason_key, "system export unavailable");
        return None;
    }
    exporter.plan(env, &availability)
}

#[cfg(test)]
mod tests;
