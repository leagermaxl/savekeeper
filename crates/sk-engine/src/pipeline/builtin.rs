//! The built-in collectors of a run (SPEC-01 §4.4 `Collect`): `rules`
//! (SPEC-04 T-04-13), loaded first, and `games` (SPEC-05 T-05-12).

use std::sync::Arc;

use sk_core::collector::{CollectOutput, Collector};
use sk_core::model::CollectorToggles;
use sk_games::GamesCollector;

use super::{enabled, Outcome, ScanPipeline};
use crate::rules::RULES_ID;

impl ScanPipeline {
    /// The built-in collectors switched on by `toggles`: the rules, loaded
    /// here (their load issues or the panic of the load go to `outcomes`),
    /// and `games`, built with its manifest load by `preload_games`.
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
        collectors
    }
}
