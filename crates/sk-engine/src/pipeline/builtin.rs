//! The built-in collectors of a run (SPEC-01 §4.4 `Collect`): `rules`
//! (SPEC-04 T-04-13), loaded first, `games` (SPEC-05 T-05-12) and `system`
//! (SPEC-06 T-06-14), and their replacement by [`ScanPipeline::with_collector`].

use std::sync::Arc;

use sk_core::collector::{CollectOutput, Collector};
use sk_core::model::CollectorToggles;
use sk_games::GamesCollector;

use super::games::GAMES_ID;
use super::system::SYSTEM_ID;
use super::{enabled, Outcome, ScanPipeline};
use crate::rules::RULES_ID;

impl ScanPipeline {
    /// Adds a collector. A collector with the id `rules`, `games` or
    /// `system` replaces the built-in collector with that id.
    pub fn with_collector(mut self, collector: Arc<dyn Collector>) -> Self {
        match collector.id() {
            RULES_ID => self.rules.enabled = false,
            GAMES_ID => self.games.enabled = false,
            SYSTEM_ID => self.system.collector = None,
            _ => {}
        }
        self.collectors.push(collector);
        self
    }

    /// The built-in collectors switched on by `toggles`: the rules, loaded
    /// here (their load issues or the panic of the load go to `outcomes`),
    /// `games`, built with its manifest load by `preload_games`, and `system`.
    pub(super) async fn builtin_collectors(
        &self,
        toggles: &CollectorToggles,
        games: Option<Arc<GamesCollector>>,
        outcomes: &mut Vec<Outcome>,
    ) -> Vec<Arc<dyn Collector>> {
        let mut collectors: Vec<Arc<dyn Collector>> = Vec::new();
        if self.rules.enabled && enabled(toggles, RULES_ID) {
            match self.rules.load().await {
                Ok((collector, issues)) => {
                    collectors.push(collector);
                    if !issues.is_empty() {
                        outcomes.push(Outcome::Output(CollectOutput {
                            issues,
                            ..CollectOutput::default()
                        }));
                    }
                }
                Err(error) => outcomes.extend(Outcome::from_join(RULES_ID, &error)),
            }
        }
        if let Some(games) = games {
            collectors.push(games);
        }
        if let Some(system) = &self.system.collector {
            if enabled(toggles, SYSTEM_ID) {
                collectors.push(Arc::clone(system));
            }
        }
        collectors
    }
}
