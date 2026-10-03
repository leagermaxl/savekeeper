//! Game save discovery: Ludusavi manifest and launchers (SPEC-05).

mod embedded;
mod error;
pub mod launchers;
pub mod manifest;
mod matching;
mod store;
mod translate;
mod when;

pub use error::GamesError;
pub use launchers::{
    enrich, enrich_with_registry, BattleNetDetector, EaDetector, EpicDetector, GogDetector,
    LauncherDetector, SteamDetector, UbisoftDetector, XboxDetector, STEAM_ID64_BASE,
};
pub use manifest::{
    CloudFlags, FileRule, GameEntry, GogRef, Ids, Manifest, ManifestMeta, ManifestSource, Os,
    RegRule, SteamRef, Store, When,
};
/// Launcher types of `Environment.launchers`, defined in `sk-core` (SPEC-02 §3.3).
pub use sk_core::env::{InstalledGame, LauncherInfo, StoreUser};
pub use store::{ManifestStore, UpdateOutcome};
