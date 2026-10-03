//! The Ludusavi manifest snapshot embedded in the binary (SPEC-05 FR-05-02, FR-05-11).
//!
//! With the cargo feature `embedded-manifest` (on by default), `build.rs`
//! compresses `third_party/ludusavi/manifest.yaml` with zstd; the data keeps
//! its own license (CC BY-NC-SA 3.0, FR-05-11) and is only compressed, not
//! filtered (`THIRD_PARTY_NOTICES.md`). Without the feature the binary holds
//! no snapshot and the manifest is only downloaded (and cached).

use std::io;

/// Compressed snapshot produced by `build.rs`.
#[cfg(feature = "embedded-manifest")]
const BUILTIN_ZST: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ludusavi-manifest.yaml.zst"));
/// Date of the snapshot, `YYYY-MM-DD` (from `third_party/ludusavi/manifest.date`).
#[cfg(feature = "embedded-manifest")]
const BUILTIN_DATE: &str = env!("SK_LUDUSAVI_SNAPSHOT_DATE");

/// A zstd-compressed manifest with its date.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Embedded {
    /// zstd frame of the YAML file.
    pub zst: &'static [u8],
    /// Snapshot date, `YYYY-MM-DD`.
    pub date: &'static str,
}

impl Embedded {
    /// The snapshot built into this binary; `None` without the feature
    /// `embedded-manifest` (FR-05-11).
    #[cfg(feature = "embedded-manifest")]
    pub(crate) const BUILTIN: Option<Self> = Some(Self {
        zst: BUILTIN_ZST,
        date: BUILTIN_DATE,
    });

    /// The snapshot built into this binary; `None` without the feature
    /// `embedded-manifest` (FR-05-11).
    #[cfg(not(feature = "embedded-manifest"))]
    pub(crate) const BUILTIN: Option<Self> = None;

    /// Unpacks the YAML.
    pub(crate) fn decompress(&self) -> io::Result<Vec<u8>> {
        zstd::stream::decode_all(self.zst)
    }
}

#[cfg(all(test, feature = "embedded-manifest"))]
mod tests {
    use super::*;

    fn builtin() -> Embedded {
        Embedded::BUILTIN.unwrap_or_else(|| panic!("the feature embeds a snapshot"))
    }

    #[test]
    fn builtin_snapshot_unpacks_to_a_manifest() {
        let e = builtin();
        assert_eq!(e.date.len(), 10, "{}", e.date);
        assert!(e.zst.len() < 16 * 1024 * 1024, "{} bytes", e.zst.len());
        let yaml = e.decompress().unwrap_or_else(|err| panic!("{err}"));
        assert!(yaml.len() > e.zst.len());
        // The real manifest is a top-level mapping of game names.
        let text = String::from_utf8_lossy(&yaml[..yaml.len().min(4096)]);
        assert!(text.contains("\n  ") && text.contains(':'), "{text}");
    }

    #[test]
    fn builtin_snapshot_parses() {
        let yaml = builtin().decompress().unwrap_or_else(|err| panic!("{err}"));
        let m = crate::Manifest::parse(&yaml, crate::ManifestSource::Cache);
        let m = m.unwrap_or_else(|err| panic!("{err}"));
        assert!(m.meta.games > 10_000, "{} games", m.meta.games);
    }
}

#[cfg(all(test, not(feature = "embedded-manifest")))]
mod tests {
    use super::*;

    #[test]
    fn no_snapshot_without_the_feature() {
        assert!(Embedded::BUILTIN.is_none());
    }
}
