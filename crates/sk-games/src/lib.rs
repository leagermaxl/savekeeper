//! Game save discovery: Ludusavi manifest and launchers (SPEC-05).

mod error;
pub mod launchers;
pub mod manifest;
pub mod registry;
mod win;

pub use error::GamesError;
pub use launchers::{LauncherDetector, SteamDetector, STEAM_ID64_BASE};
pub use manifest::{
    CloudFlags, FileRule, GameEntry, GogRef, Ids, Manifest, ManifestMeta, ManifestSource, Os,
    RegRule, SteamRef, Store, When,
};
pub use registry::{MemRegistry, RegistryReader, SystemRegistry};
/// Launcher types of `Environment.launchers`, defined in `sk-core` (SPEC-02 §3.3).
pub use sk_core::env::{InstalledGame, LauncherInfo, StoreUser};
