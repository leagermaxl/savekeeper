//! The `Environment` phase (SPEC-01 §4.4): `Environment::detect()` (or the
//! environment of `with_environment`), then the game launchers of SPEC-05
//! (`sk_games::enrich`, T-05-05) into `Environment.launchers`.

use std::sync::Arc;

use sk_core::env::Environment;
use sk_core::error::EngineError;
use sk_core::fs::FsScanner;
use sk_core::model::ScanIssue;
use sk_core::registry::{RegistryReader, SystemRegistry};
use sk_scan::RealFs;

use super::{issue, Accumulated, Run, ScanPipeline};

/// Source of the issue of a panicking launcher detection.
const GAMES_ID: &str = "games";

impl ScanPipeline {
    /// Replaces the registry the launcher detectors of the `Environment`
    /// phase read (`MemRegistry` in tests); without it, `SystemRegistry`.
    pub fn with_games_registry(mut self, registry: Arc<dyn RegistryReader>) -> Self {
        self.games_registry = Some(registry);
        self
    }

    /// The environment of this run with its launchers, and the scanner for
    /// it. The detector issues go to the report and to the events.
    pub(super) async fn environment(
        &self,
        acc: &mut Accumulated,
        run: &Run<'_>,
    ) -> Result<(Environment, Arc<dyn FsScanner>), EngineError> {
        let env = match &self.environment {
            Some(env) => env.clone(),
            None => Environment::detect()?,
        };
        let scanner = match &self.scanner {
            Some(scanner) => Arc::clone(scanner),
            None => Arc::new(RealFs::new(&env)),
        };
        let registry = match &self.games_registry {
            Some(registry) => Arc::clone(registry),
            None => Arc::new(SystemRegistry),
        };
        let (env, issues) = enrich(env, Arc::clone(&scanner), registry).await;
        for issue in issues {
            acc.push_issue(issue, run);
        }
        Ok((env, scanner))
    }
}

/// `sk_games::enrich` on a blocking thread (the detectors read files and the
/// registry). If it panics, the launchers stay as they were and an
/// Error issue `collector.panicked` with source `games` is returned.
async fn enrich(
    env: Environment,
    scanner: Arc<dyn FsScanner>,
    registry: Arc<dyn RegistryReader>,
) -> (Environment, Vec<ScanIssue>) {
    let backup = env.clone();
    let task = tokio::task::spawn_blocking(move || {
        let mut env = env;
        let issues = sk_games::enrich_with_registry(&mut env, &*scanner, registry);
        (env, issues)
    });
    match task.await {
        Ok(out) => out,
        Err(error) => {
            let issues = if error.is_panic() {
                vec![issue(GAMES_ID, "collector.panicked", &error.to_string())]
            } else {
                Vec::new()
            };
            (backup, issues)
        }
    }
}
