//! Game save discovery: Ludusavi manifest and launchers (SPEC-05).

mod error;
pub mod manifest;

pub use error::GamesError;
pub use manifest::{
    CloudFlags, FileRule, GameEntry, GogRef, Ids, Manifest, ManifestMeta, ManifestSource, Os,
    RegRule, SteamRef, Store, When,
};
