//! The Ludusavi manifest: model (SPEC-05 §4.2) and parsing of the full file.

pub(crate) mod index;
mod model;
mod parse;

use std::collections::HashMap;

use time::OffsetDateTime;

use crate::GamesError;

pub use model::{CloudFlags, FileRule, GameEntry, GogRef, Ids, Os, RegRule, SteamRef, Store, When};

/// Parsed Ludusavi manifest (SPEC-05 §4.1).
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    /// Entries by game name as written in the manifest, alias entries included.
    pub games: HashMap<String, GameEntry>,
    /// Where the manifest came from.
    pub meta: ManifestMeta,
}

/// Origin and version of a manifest (SPEC-05 §4.1).
#[derive(Debug, Clone, PartialEq)]
pub struct ManifestMeta {
    /// Download, cache or embedded snapshot.
    pub source: ManifestSource,
    /// HTTP `ETag` of the downloaded file, when known.
    pub etag: Option<String>,
    /// When the file was downloaded, when known.
    pub fetched_at: Option<OffsetDateTime>,
    /// Number of entries in [`Manifest::games`], alias entries included.
    pub games: usize,
}

/// Where a manifest was loaded from (SPEC-05 §4.1, FR-05-02).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestSource {
    /// Downloaded during this run.
    Downloaded,
    /// Read from `cache/ludusavi-manifest.yaml`.
    Cache,
    /// The snapshot embedded in the binary.
    Embedded {
        /// Date of the snapshot, `YYYY-MM-DD`.
        snapshot_date: String,
    },
}

impl Manifest {
    /// Parses a full manifest file (`data/manifest.yaml` of ludusavi-manifest).
    ///
    /// `meta.etag` and `meta.fetched_at` are left empty for the caller to
    /// fill; `meta.games` is the number of parsed entries. Unknown fields are
    /// ignored (SPEC-05 §4.2); a game entry written as `null` is an empty
    /// entry, and an empty document is an empty manifest.
    ///
    /// A large file is split at top-level entries and parsed on several
    /// threads (NFR-05-01); the result is the same as a sequential parse,
    /// which is used whenever the split is not safe.
    ///
    /// # Errors
    /// [`GamesError::ManifestParse`] when the input is not UTF-8, not YAML,
    /// does not match the model, or has a duplicate game name.
    pub fn parse(yaml: &[u8], source: ManifestSource) -> Result<Self, GamesError> {
        let games = parse::parse_games(yaml)?;
        let meta = ManifestMeta {
            source,
            etag: None,
            fetched_at: None,
            games: games.len(),
        };
        Ok(Self { games, meta })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_game_entry_is_empty() {
        let m = Manifest::parse(b"Some Game:\nOther: {}\n", ManifestSource::Cache);
        let m = m.unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(m.meta.games, 2);
        assert_eq!(m.games["Some Game"], GameEntry::default());
        assert_eq!(m.games["Other"], GameEntry::default());
    }

    #[test]
    fn empty_document_is_an_empty_manifest() {
        // Rejecting a suspiciously small download is up to `ManifestStore`.
        for input in [&b""[..], b"# comment only\n", b"{}", b"~"] {
            let m = Manifest::parse(input, ManifestSource::Cache);
            let m = m.unwrap_or_else(|e| panic!("{e}"));
            assert!(m.games.is_empty());
            assert_eq!(m.meta.games, 0);
        }
    }

    #[test]
    fn invalid_input_is_a_parse_error() {
        for bad in [
            &b"just a string"[..],
            b"Game:\n  steam:\n    id: not-a-number\n",
            b"Game: {}\nGame: {}\n",
            b"\xff\xfe",
        ] {
            let r = Manifest::parse(bad, ManifestSource::Cache);
            assert!(
                matches!(r, Err(GamesError::ManifestParse(_))),
                "{:?} -> {r:?}",
                String::from_utf8_lossy(bad)
            );
        }
    }

    #[test]
    fn meta_records_source_and_count() {
        let source = ManifestSource::Embedded {
            snapshot_date: "2026-10-01".to_owned(),
        };
        let m = Manifest::parse(b"A: {}\nB: {}\nC: { alias: A }\n", source.clone());
        let m = m.unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(m.meta.source, source);
        assert_eq!(m.meta.games, 3);
        assert_eq!(m.meta.etag, None);
        assert_eq!(m.meta.fetched_at, None);
    }
}
