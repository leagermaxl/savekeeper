//! The heuristics of SPEC-07 in the pipeline (T-07-01): the built-in
//! `heuristics` post-collector of the `Heuristics` phase and its replacement
//! by [`ScanPipeline::with_post_collector`].

use std::sync::Arc;

use sk_core::collector::PostCollector;
use sk_core::config::Config;
use sk_heuristics::HeuristicsCollector;

use super::ScanPipeline;

/// `PostCollector::id` of the heuristics collector.
pub(crate) const HEURISTICS_ID: &str = "heuristics";

/// The built-in heuristics collector for `config` (`config.heuristics`).
pub(super) fn builtin(config: &Config) -> Option<Arc<dyn PostCollector>> {
    Some(Arc::new(HeuristicsCollector::new(
        config.heuristics.clone(),
    )))
}

impl ScanPipeline {
    /// Adds a post-collector. A post-collector with the id `heuristics`
    /// replaces the built-in heuristics collector.
    pub fn with_post_collector(mut self, collector: Arc<dyn PostCollector>) -> Self {
        if collector.id() == HEURISTICS_ID {
            self.heuristics = None;
        }
        self.post_collectors.push(collector);
        self
    }

    /// The post-collectors of the `Heuristics` phase in the order they run:
    /// the built-in heuristics collector (unless replaced), then the added ones.
    pub(super) fn all_post_collectors(&self) -> impl Iterator<Item = &Arc<dyn PostCollector>> {
        self.heuristics.iter().chain(&self.post_collectors)
    }
}
