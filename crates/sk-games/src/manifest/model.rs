//! Serde model of a Ludusavi manifest entry (SPEC-05 §4.2).
//!
//! Only the subset read by SaveKeeper is modelled; unknown fields (`launch`,
//! `notes`, `id.lutris`, ...) are ignored because the manifest evolves. Every
//! collection field accepts `null` as empty, and unknown `os` / `store` values
//! are kept as [`Os::Unknown`] / [`Store::Unknown`] instead of failing the
//! whole file.

use std::collections::BTreeMap;

use serde::de::IgnoredAny;
use serde::{Deserialize, Deserializer};

/// One game of the manifest (top-level key = game name).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameEntry {
    /// File and folder paths with Ludusavi placeholders (`<winAppData>/Game`), `/`-separated.
    #[serde(default, deserialize_with = "null_as_default")]
    pub files: BTreeMap<String, FileRule>,
    /// Registry keys, `HKEY_CURRENT_USER/Software/...`, `/`-separated.
    #[serde(default, deserialize_with = "null_as_default")]
    pub registry: BTreeMap<String, RegRule>,
    /// Names of the install folder (`<base>` is `<library>/<installDir>`).
    #[serde(default, deserialize_with = "install_dir")]
    pub install_dir: BTreeMap<String, ()>,
    /// Steam app; `None` also when the manifest gives `steam` without an `id`.
    #[serde(default, deserialize_with = "steam_ref")]
    pub steam: Option<SteamRef>,
    /// GOG product; `None` also when the manifest gives `gog` without an `id`.
    #[serde(default, deserialize_with = "gog_ref")]
    pub gog: Option<GogRef>,
    /// Stores that sync the saves of this game (FR-05-08).
    #[serde(default)]
    pub cloud: Option<CloudFlags>,
    /// This entry only points to another entry by name; it has no data of its own.
    #[serde(default)]
    pub alias: Option<String>,
    /// Extra store ids.
    #[serde(default)]
    pub id: Option<Ids>,
}

impl GameEntry {
    /// `true` for an alias entry (`alias: <other game>`), which is skipped by the collector.
    pub fn is_alias(&self) -> bool {
        self.alias.is_some()
    }
}

/// Conditions and tags of one `files` entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(from = "Option<RawRule>")]
pub struct FileRule {
    /// Ludusavi tags: `save`, `config`, ... (FR-05-06).
    pub tags: Vec<String>,
    /// Alternatives: the entry applies when any item matches; empty means always.
    pub when: Vec<When>,
}

/// Conditions and tags of one `registry` entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(from = "Option<RawRule>")]
pub struct RegRule {
    /// Ludusavi tags: `save`, `config`, ...
    pub tags: Vec<String>,
    /// Alternatives: the entry applies when any item matches; empty means always.
    pub when: Vec<When>,
}

/// One `when` condition; a missing field means "any".
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct When {
    /// Operating system.
    #[serde(default)]
    pub os: Option<Os>,
    /// Store (launcher) the game was installed from.
    #[serde(default)]
    pub store: Option<Store>,
}

/// Operating system of a `when` condition.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize)]
#[serde(from = "String")]
pub enum Os {
    /// `windows`.
    Windows,
    /// `linux`.
    Linux,
    /// `mac`.
    Mac,
    /// `dos`.
    Dos,
    /// A value this version does not know, as written.
    Unknown(String),
}

impl From<String> for Os {
    fn from(s: String) -> Self {
        match s.as_str() {
            "windows" => Self::Windows,
            "linux" => Self::Linux,
            "mac" => Self::Mac,
            "dos" => Self::Dos,
            _ => Self::Unknown(s),
        }
    }
}

/// Store of a `when` condition (Ludusavi `Store` enum).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize)]
#[serde(from = "String")]
pub enum Store {
    /// `steam`.
    Steam,
    /// `epic`.
    Epic,
    /// `gog`.
    Gog,
    /// `gogGalaxy`.
    GogGalaxy,
    /// `ea`.
    Ea,
    /// `origin`.
    Origin,
    /// `uplay` (Ubisoft Connect).
    Uplay,
    /// `microsoft` (Xbox / Microsoft Store).
    Microsoft,
    /// `prime` (Amazon Games).
    Prime,
    /// `heroic`.
    Heroic,
    /// `legendary`.
    Legendary,
    /// `lutris`.
    Lutris,
    /// `other`: Ludusavi's own "any other store".
    Other,
    /// A value this version does not know, as written.
    Unknown(String),
}

impl From<String> for Store {
    fn from(s: String) -> Self {
        match s.as_str() {
            "steam" => Self::Steam,
            "epic" => Self::Epic,
            "gog" => Self::Gog,
            "gogGalaxy" => Self::GogGalaxy,
            "ea" => Self::Ea,
            "origin" => Self::Origin,
            "uplay" => Self::Uplay,
            "microsoft" => Self::Microsoft,
            "prime" => Self::Prime,
            "heroic" => Self::Heroic,
            "legendary" => Self::Legendary,
            "lutris" => Self::Lutris,
            "other" => Self::Other,
            _ => Self::Unknown(s),
        }
    }
}

/// `steam: { id }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SteamRef {
    /// Steam app id.
    pub id: u32,
}

/// `gog: { id }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GogRef {
    /// GOG product id.
    pub id: u64,
}

/// `cloud`: stores that sync the saves (FR-05-08). Missing flags are `false`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub struct CloudFlags {
    /// EA (Origin) cloud.
    #[serde(default)]
    pub origin: bool,
    /// Epic Games cloud.
    #[serde(default)]
    pub epic: bool,
    /// GOG Galaxy cloud.
    #[serde(default)]
    pub gog: bool,
    /// Steam Cloud.
    #[serde(default)]
    pub steam: bool,
    /// Ubisoft Connect cloud.
    #[serde(default)]
    pub uplay: bool,
}

/// `id`: secondary store ids of a game.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ids {
    /// Flatpak application id (Linux; kept for completeness).
    #[serde(default)]
    pub flatpak: Option<String>,
    /// More GOG product ids of the same game (editions, DLC bundles).
    #[serde(default, deserialize_with = "null_as_default")]
    pub gog_extra: Vec<u64>,
    /// More Steam app ids of the same game.
    #[serde(default, deserialize_with = "null_as_default")]
    pub steam_extra: Vec<u32>,
}

/// Wire form of [`FileRule`] / [`RegRule`].
#[derive(Deserialize)]
struct RawRule {
    #[serde(default, deserialize_with = "null_as_default")]
    tags: Vec<String>,
    #[serde(default, deserialize_with = "null_as_default")]
    when: Vec<When>,
}

impl From<Option<RawRule>> for FileRule {
    fn from(raw: Option<RawRule>) -> Self {
        let raw = raw.unwrap_or(RawRule {
            tags: Vec::new(),
            when: Vec::new(),
        });
        Self {
            tags: raw.tags,
            when: raw.when,
        }
    }
}

impl From<Option<RawRule>> for RegRule {
    fn from(raw: Option<RawRule>) -> Self {
        let FileRule { tags, when } = raw.into();
        Self { tags, when }
    }
}

/// Wire form of `steam` / `gog`.
#[derive(Deserialize)]
struct RawStoreRef<T> {
    #[serde(default = "Option::default")]
    id: Option<T>,
}

fn null_as_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

fn install_dir<'de, D>(d: D) -> Result<BTreeMap<String, ()>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw: BTreeMap<String, IgnoredAny> = null_as_default(d)?;
    Ok(raw.into_keys().map(|k| (k, ())).collect())
}

fn steam_ref<'de, D>(d: D) -> Result<Option<SteamRef>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Option::<RawStoreRef<u32>>::deserialize(d)?;
    Ok(raw.and_then(|r| r.id).map(|id| SteamRef { id }))
}

fn gog_ref<'de, D>(d: D) -> Result<Option<GogRef>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Option::<RawStoreRef<u64>>::deserialize(d)?;
    Ok(raw.and_then(|r| r.id).map(|id| GogRef { id }))
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
