//! The built-in `rules` collector of the pipeline (SPEC-04 T-04-13).
//!
//! The rules are loaded again at the start of every run (built-in rules and
//! the user folder `rules.d`), so edited user rules are picked up without a
//! restart; the load issues go to the report like the issues of a collector.

use std::path::PathBuf;
use std::sync::Arc;

use sk_core::collector::Collector;
use sk_core::model::ScanIssue;
use sk_core::registry::RegistryReader;
use sk_rules::{RuleSet, RulesCollector};
use tokio::task::JoinError;

/// `Collector::id` of the rules collector.
pub(crate) const RULES_ID: &str = "rules";

/// The rules collector that `ScanPipeline::new` registers.
pub(crate) struct BuiltinRules {
    /// `false` once a collector with the id `rules` replaced it.
    pub(crate) enabled: bool,
    /// The user rule folder (`DataDir::rules`); `None`: built-in rules only.
    pub(crate) dir: Option<PathBuf>,
    /// `None`: the registry of this machine (`SystemRegistry`).
    pub(crate) registry: Option<Arc<dyn RegistryReader>>,
}

impl Default for BuiltinRules {
    fn default() -> Self {
        Self {
            enabled: true,
            dir: None,
            registry: None,
        }
    }
}

impl BuiltinRules {
    /// Loads the rules on a blocking thread (they read files) and builds the
    /// collector of this run; the load issues come with it.
    pub(crate) async fn load(&self) -> Result<(Arc<dyn Collector>, Vec<ScanIssue>), JoinError> {
        let dir = self.dir.clone();
        let (set, issues) =
            tokio::task::spawn_blocking(move || RuleSet::load(true, dir.as_deref())).await?;
        let mut collector = RulesCollector::new(Arc::new(set));
        if let Some(registry) = &self.registry {
            collector = collector.with_registry(Arc::clone(registry));
        }
        Ok((Arc::new(collector), issues))
    }
}
