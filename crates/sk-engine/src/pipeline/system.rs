//! The system exports of SPEC-06 in the pipeline (T-06-14): the built-in
//! `system` collector (`SystemCollector::new()`) and the installed programs
//! of the `Environment` phase (`sk_system::enrich_with_registry`), which run
//! even with the `system` collector switched off, because the `installed`
//! conditions of the rules (SPEC-04) depend on them.

use std::sync::Arc;

use sk_core::collector::Collector;
use sk_core::env::Environment;
use sk_core::model::ScanIssue;
use sk_core::registry::{RegistryReader, SystemRegistry};
use sk_system::SystemCollector;

use super::{issue, ScanPipeline};

/// `Collector::id` of the system collector; also the source of the issue of
/// a panicking `enrich_with_registry`.
pub(crate) const SYSTEM_ID: &str = "system";

/// The system collector that `ScanPipeline::new` registers, and the registry
/// of the installed programs.
pub(crate) struct BuiltinSystem {
    /// `None` once a collector with the id `system` replaced it.
    pub(crate) collector: Option<Arc<dyn Collector>>,
    /// Registry of `enrich_with_registry`; `None`: `SystemRegistry`.
    pub(crate) registry: Option<Arc<dyn RegistryReader>>,
}

impl Default for BuiltinSystem {
    fn default() -> Self {
        Self {
            collector: Some(Arc::new(SystemCollector::new())),
            registry: None,
        }
    }
}

impl BuiltinSystem {
    /// The registry the installed programs are read from.
    fn registry(&self) -> Arc<dyn RegistryReader> {
        match &self.registry {
            Some(registry) => Arc::clone(registry),
            None => Arc::new(SystemRegistry),
        }
    }
}

impl ScanPipeline {
    /// Replaces the registry `sk_system::enrich_with_registry` of the
    /// `Environment` phase reads the installed programs from (`MemRegistry`
    /// in tests); without it, `SystemRegistry`.
    pub fn with_system_registry(mut self, registry: Arc<dyn RegistryReader>) -> Self {
        self.system.registry = Some(registry);
        self
    }

    /// Fills `env.installed_programs` (see [`enrich`]).
    pub(super) async fn enrich_system(&self, env: Environment) -> (Environment, Option<ScanIssue>) {
        enrich(env, self.system.registry()).await
    }
}

/// `sk_system::enrich_with_registry` on a blocking thread (it reads the
/// registry). If it panics, `installed_programs` is empty and an Error issue
/// `collector.panicked` with source `system` is returned.
async fn enrich(
    env: Environment,
    registry: Arc<dyn RegistryReader>,
) -> (Environment, Option<ScanIssue>) {
    let mut backup = env.clone();
    backup.installed_programs = Vec::new();
    let task = tokio::task::spawn_blocking(move || {
        let mut env = env;
        sk_system::enrich_with_registry(&mut env, &*registry);
        env
    });
    match task.await {
        Ok(env) => (env, None),
        Err(error) => {
            let issue = error
                .is_panic()
                .then(|| issue(SYSTEM_ID, "collector.panicked", &error.to_string()));
            (backup, issue)
        }
    }
}
