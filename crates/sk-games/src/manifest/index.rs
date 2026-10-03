//! Binary cache of a parsed manifest, `cache/ludusavi-index.bin` (SPEC-05 NFR-05-01).
//!
//! Layout: [`MAGIC`], then postcard of [`IndexKey`], then postcard of the
//! games. The key names the YAML the index was built from (etag, size and
//! modification time of the cached file, or the snapshot date); any mismatch,
//! a damaged file or a newer program version means "no index" and the YAML is
//! parsed again. The model types have YAML-specific `Deserialize` impls, so the
//! file uses its own mirror types, generic over borrowed (`&str`, writing) and
//! owned (`String`, reading) strings to avoid copying the manifest.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::model::{
    CloudFlags, FileRule, GameEntry, GogRef, Ids, Os, RegRule, SteamRef, Store, When,
};

/// File signature; bump the digit when the layout below changes.
const MAGIC: &[u8; 8] = b"SKLDIX01";

/// What the index was built from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct IndexKey {
    /// `CARGO_PKG_VERSION` of the writer: the model may change between versions.
    pub program: String,
    /// `"cache"` or `"embedded"`.
    pub source: String,
    /// `ETag` of the cached download.
    pub etag: Option<String>,
    /// Snapshot date of the embedded manifest.
    pub snapshot_date: Option<String>,
    /// Size of the source YAML (compressed size for the snapshot).
    pub len: u64,
    /// Modification time of the cached YAML, nanoseconds since the Unix epoch.
    pub modified_ns: Option<u128>,
}

impl IndexKey {
    /// Key of `cache/ludusavi-manifest.yaml`.
    pub(crate) fn cache(etag: Option<String>, len: u64, modified_ns: Option<u128>) -> Self {
        Self {
            program: env!("CARGO_PKG_VERSION").to_owned(),
            source: "cache".to_owned(),
            etag,
            snapshot_date: None,
            len,
            modified_ns,
        }
    }

    /// Key of the embedded snapshot.
    pub(crate) fn embedded(snapshot_date: &str, zst_len: u64) -> Self {
        Self {
            program: env!("CARGO_PKG_VERSION").to_owned(),
            source: "embedded".to_owned(),
            etag: None,
            snapshot_date: Some(snapshot_date.to_owned()),
            len: zst_len,
            modified_ns: None,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct IGame<S> {
    files: Vec<(S, IRule<S>)>,
    registry: Vec<(S, IRule<S>)>,
    install_dir: Vec<S>,
    steam: Option<u32>,
    gog: Option<u64>,
    /// origin, epic, gog, steam, uplay.
    cloud: Option<[bool; 5]>,
    alias: Option<S>,
    id: Option<IIds<S>>,
}

#[derive(Serialize, Deserialize)]
struct IRule<S> {
    tags: Vec<S>,
    when: Vec<IWhen<S>>,
}

#[derive(Serialize, Deserialize)]
struct IWhen<S> {
    os: Option<S>,
    store: Option<S>,
}

#[derive(Serialize, Deserialize)]
struct IIds<S> {
    flatpak: Option<S>,
    gog_extra: Vec<u64>,
    steam_extra: Vec<u32>,
}

/// Writes the index atomically (temporary file + rename).
pub(crate) fn write(
    path: &Path,
    key: &IndexKey,
    games: &HashMap<String, GameEntry>,
) -> io::Result<()> {
    let borrowed: Vec<(&str, IGame<&str>)> = games
        .iter()
        .map(|(k, g)| (k.as_str(), borrow_game(g)))
        .collect();
    let mut buf = MAGIC.to_vec();
    buf = postcard::to_extend(key, buf).map_err(io::Error::other)?;
    buf = postcard::to_extend(&borrowed, buf).map_err(io::Error::other)?;
    let tmp = path.with_extension("bin.tmp");
    fs::write(&tmp, &buf)?;
    fs::rename(&tmp, path)
}

/// Reads the index if it exists, is intact and was built for `key`.
pub(crate) fn read(path: &Path, key: &IndexKey) -> Option<HashMap<String, GameEntry>> {
    let bytes = fs::read(path).ok()?;
    let rest = bytes.strip_prefix(MAGIC.as_slice())?;
    let (stored, rest) = postcard::take_from_bytes::<IndexKey>(rest).ok()?;
    if &stored != key {
        tracing::debug!("ludusavi index is stale");
        return None;
    }
    let games: Vec<(String, IGame<String>)> = match postcard::from_bytes(rest) {
        Ok(g) => g,
        Err(e) => {
            tracing::debug!("ludusavi index is damaged: {e}");
            return None;
        }
    };
    let mut out = HashMap::with_capacity(games.len());
    for (name, game) in games {
        if out.insert(name, own_game(game)).is_some() {
            return None;
        }
    }
    Some(out)
}

fn borrow_game(g: &GameEntry) -> IGame<&str> {
    IGame {
        files: g
            .files
            .iter()
            .map(|(k, r)| (k.as_str(), borrow_rule(&r.tags, &r.when)))
            .collect(),
        registry: (g.registry.iter())
            .map(|(k, r)| (k.as_str(), borrow_rule(&r.tags, &r.when)))
            .collect(),
        install_dir: g.install_dir.keys().map(String::as_str).collect(),
        steam: g.steam.map(|s| s.id),
        gog: g.gog.map(|s| s.id),
        cloud: g.cloud.map(|c| [c.origin, c.epic, c.gog, c.steam, c.uplay]),
        alias: g.alias.as_deref(),
        id: g.id.as_ref().map(|i| IIds {
            flatpak: i.flatpak.as_deref(),
            gog_extra: i.gog_extra.clone(),
            steam_extra: i.steam_extra.clone(),
        }),
    }
}

fn borrow_rule<'a>(tags: &'a [String], when: &'a [When]) -> IRule<&'a str> {
    IRule {
        tags: tags.iter().map(String::as_str).collect(),
        when: (when.iter())
            .map(|w| IWhen {
                os: w.os.as_ref().map(os_name),
                store: w.store.as_ref().map(store_name),
            })
            .collect(),
    }
}

fn own_game(g: IGame<String>) -> GameEntry {
    GameEntry {
        files: g.files.into_iter().map(|(k, r)| (k, own_rule(r))).collect(),
        registry: (g.registry.into_iter())
            .map(|(k, r)| {
                let FileRule { tags, when } = own_rule(r);
                (k, RegRule { tags, when })
            })
            .collect(),
        install_dir: g.install_dir.into_iter().map(|k| (k, ())).collect(),
        steam: g.steam.map(|id| SteamRef { id }),
        gog: g.gog.map(|id| GogRef { id }),
        cloud: g.cloud.map(|[origin, epic, gog, steam, uplay]| CloudFlags {
            origin,
            epic,
            gog,
            steam,
            uplay,
        }),
        alias: g.alias,
        id: g.id.map(|i| Ids {
            flatpak: i.flatpak,
            gog_extra: i.gog_extra,
            steam_extra: i.steam_extra,
        }),
    }
}

fn own_rule(r: IRule<String>) -> FileRule {
    FileRule {
        tags: r.tags,
        when: (r.when.into_iter())
            .map(|w| When {
                os: w.os.map(Os::from),
                store: w.store.map(Store::from),
            })
            .collect(),
    }
}

/// Manifest spelling of an [`Os`]; `Os::from` maps it back.
fn os_name(os: &Os) -> &str {
    match os {
        Os::Windows => "windows",
        Os::Linux => "linux",
        Os::Mac => "mac",
        Os::Dos => "dos",
        Os::Unknown(s) => s,
    }
}

/// Manifest spelling of a [`Store`]; `Store::from` maps it back.
fn store_name(store: &Store) -> &str {
    match store {
        Store::Steam => "steam",
        Store::Epic => "epic",
        Store::Gog => "gog",
        Store::GogGalaxy => "gogGalaxy",
        Store::Ea => "ea",
        Store::Origin => "origin",
        Store::Uplay => "uplay",
        Store::Microsoft => "microsoft",
        Store::Prime => "prime",
        Store::Heroic => "heroic",
        Store::Legendary => "legendary",
        Store::Lutris => "lutris",
        Store::Other => "other",
        Store::Unknown(s) => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Manifest, ManifestSource};

    const SAMPLE: &str = include_str!("../../../../fixtures/samples/ludusavi/manifest-sample.yaml");
    const MINI: &str = include_str!("../../../../fixtures/samples/ludusavi/manifest-mini.yaml");

    fn parse(yaml: &str) -> HashMap<String, GameEntry> {
        let m = Manifest::parse(yaml.as_bytes(), ManifestSource::Cache);
        m.unwrap_or_else(|e| panic!("{e}")).games
    }

    #[test]
    fn round_trip_keeps_every_field() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let path = dir.path().join("ludusavi-index.bin");
        for yaml in [SAMPLE, MINI] {
            let games = parse(yaml);
            let key = IndexKey::cache(Some("\"e1\"".into()), 42, Some(7));
            write(&path, &key, &games).unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(read(&path, &key), Some(games));
        }
    }

    #[test]
    fn names_round_trip_for_every_os_and_store() {
        for s in ["windows", "linux", "mac", "dos", "beos"] {
            assert_eq!(os_name(&Os::from(s.to_owned())), s);
        }
        for s in [
            "steam",
            "epic",
            "gog",
            "gogGalaxy",
            "ea",
            "origin",
            "uplay",
            "microsoft",
            "prime",
            "heroic",
            "legendary",
            "lutris",
            "other",
            "itch",
        ] {
            assert_eq!(store_name(&Store::from(s.to_owned())), s);
        }
    }

    #[test]
    fn other_key_damaged_or_missing_file_is_no_index() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let path = dir.path().join("ludusavi-index.bin");
        let key = IndexKey::cache(Some("a".into()), 1, None);
        assert_eq!(read(&path, &key), None);
        write(&path, &key, &parse(SAMPLE)).unwrap_or_else(|e| panic!("{e}"));
        for other in [
            IndexKey::cache(Some("b".into()), 1, None),
            IndexKey::cache(Some("a".into()), 2, None),
            IndexKey::cache(Some("a".into()), 1, Some(5)),
            IndexKey::embedded("2026-10-03", 1),
        ] {
            assert_eq!(read(&path, &other), None, "{other:?}");
        }
        let mut bytes = fs::read(&path).unwrap_or_else(|e| panic!("{e}"));
        let len = bytes.len();
        bytes.truncate(len - 3);
        fs::write(&path, &bytes).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(read(&path, &key), None);
        fs::write(&path, b"garbage").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(read(&path, &key), None);
    }
}
