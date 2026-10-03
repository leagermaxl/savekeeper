//! The `Environment` phase (SPEC-01 §4.4): `Environment::detect()` (or the
//! environment of `with_environment`), then the game launchers of SPEC-05
//! (`sk_games::enrich`, T-05-05) into `Environment.launchers`, while the
//! manifest of the `games` collector loads (T-05-15).

use std::sync::Arc;

use sk_core::env::Environment;
use sk_core::error::EngineError;
use sk_core::fs::FsScanner;
use sk_core::model::{CollectorToggles, ScanIssue};
use sk_core::registry::{RegistryReader, SystemRegistry};
use sk_core::CancellationToken;
use sk_games::GamesCollector;
use sk_scan::RealFs;

use super::games::GAMES_ID;
use super::{issue, Accumulated, Run, ScanPipeline};

/// The result of the `Environment` phase: the environment, its scanner and
/// the built-in games collector with its manifest load started.
pub(super) type EnvironmentPhase = (Environment, Arc<dyn FsScanner>, Option<Arc<GamesCollector>>);

impl ScanPipeline {
    /// Replaces the registry the launcher detectors of the `Environment`
    /// phase and the built-in `games` collector read (`MemRegistry` in
    /// tests); without it, `SystemRegistry`.
    pub fn with_games_registry(mut self, registry: Arc<dyn RegistryReader>) -> Self {
        self.games_registry = Some(registry);
        self
    }

    /// The environment of this run with its launchers, the scanner for it,
    /// and the built-in games collector (if switched on by `toggles`), whose
    /// manifest load starts first and runs alongside the detection with the
    /// collectors' token `cancel` (SPEC-05 T-05-15, NFR-05-01). The detector
    /// issues go to the report and to the events.
    pub(super) async fn environment(
        &self,
        toggles: &CollectorToggles,
        cancel: &CancellationToken,
        acc: &mut Accumulated,
        run: &Run<'_>,
    ) -> Result<EnvironmentPhase, EngineError> {
        let games = self.preload_games(toggles, cancel);
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
        Ok((env, scanner, games))
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
