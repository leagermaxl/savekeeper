//! The built-in `games` collector of the pipeline (SPEC-05 T-05-12):
//! `GamesCollector` over a `ManifestStore` built from the `Config` and the
//! data folder (`DataDir::root`, cache in `cache/`) of each run. Its manifest
//! load starts with the `Environment` phase (T-05-15, NFR-05-01).

use std::path::PathBuf;
use std::sync::Arc;

use sk_core::config::Config;
use sk_core::model::CollectorToggles;
use sk_core::registry::RegistryReader;
use sk_core::CancellationToken;
use sk_games::{GamesCollector, ManifestStore};

use super::{enabled, ScanPipeline};

/// `Collector::id` of the games collector; also the source of the issue of
/// a panicking launcher detection.
pub(crate) const GAMES_ID: &str = "games";

/// The games collector that `ScanPipeline` registers.
pub(crate) struct BuiltinGames {
    /// `false` once a collector with the id `games` replaced it.
    pub(crate) enabled: bool,
    /// `DataDir::root`; `None`: no built-in games collector (no manifest cache).
    pub(crate) data_dir: Option<PathBuf>,
    /// `allow_network` of `ManifestStore::load` (`!offline`, SPEC-05 §4.7 step 1).
    pub(crate) allow_network: bool,
}

impl Default for BuiltinGames {
    fn default() -> Self {
        Self {
            enabled: true,
            data_dir: None,
            allow_network: true,
        }
    }
}

impl BuiltinGames {
    /// The collector of one run: a fresh `ManifestStore` for `config`, the
    /// registry `registry` (`None`: `SystemRegistry`). `None` when replaced
    /// or without a data folder.
    pub(crate) fn collector(
        &self,
        config: &Config,
        registry: Option<&Arc<dyn RegistryReader>>,
    ) -> Option<Arc<GamesCollector>> {
        if !self.enabled {
            return None;
        }
        let store = ManifestStore::new(config, self.data_dir.as_deref()?);
        let mut collector = GamesCollector::new(store).with_network(self.allow_network);
        if let Some(registry) = registry {
            collector = collector.with_registry(Arc::clone(registry));
        }
        Some(Arc::new(collector))
    }
}

impl ScanPipeline {
    /// The built-in games collector of one run, switched on by `toggles`,
    /// with its manifest load already started (`GamesCollector::preload`
    /// with `cancel`, the token of the collectors). Called at the start of
    /// the `Environment` phase; dropping the collector aborts the load.
    pub(super) fn preload_games(
        &self,
        toggles: &CollectorToggles,
        cancel: &CancellationToken,
    ) -> Option<Arc<GamesCollector>> {
        if !enabled(toggles, GAMES_ID) {
            return None;
        }
        let collector = self
            .games
            .collector(&self.config, self.games_registry.as_ref())?;
        collector.preload(cancel);
        Some(collector)
    }

    /// Registers the built-in `games` collector (SPEC-05) with the manifest
    /// cache in `<dir>/cache`, where `dir` is the `savekeeper-data` folder
    /// (`DataDir::root`). Without this call there is no built-in games
    /// collector. Its registry is the one of [`Self::with_games_registry`].
    pub fn with_data_dir(mut self, dir: PathBuf) -> Self {
        self.games.data_dir = Some(dir);
        self
    }

    /// `false`: scans do not download the Ludusavi manifest, the cache or
    /// the embedded snapshot is used (`GamesCollector::with_network`). By
    /// default the network is allowed, as far as `games.auto_update` allows.
    pub fn with_network(mut self, allow: bool) -> Self {
        self.games.allow_network = allow;
        self
    }
}
