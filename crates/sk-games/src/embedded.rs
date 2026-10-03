//! The Ludusavi manifest snapshot embedded in the binary (SPEC-05 FR-05-02, FR-05-11).
//!
//! `build.rs` compresses `third_party/ludusavi/manifest.yaml` with zstd; the
//! data keeps its own license (CC BY-NC-SA, FR-05-11) and is only compressed,
//! not filtered.

use std::io;

/// Compressed snapshot produced by `build.rs`.
const BUILTIN_ZST: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ludusavi-manifest.yaml.zst"));
/// Date of the snapshot, `YYYY-MM-DD` (from `third_party/ludusavi/manifest.date`).
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
    /// The snapshot built into this binary.
    pub(crate) const BUILTIN: Self = Self {
        zst: BUILTIN_ZST,
        date: BUILTIN_DATE,
    };

    /// Unpacks the YAML.
    pub(crate) fn decompress(&self) -> io::Result<Vec<u8>> {
        zstd::stream::decode_all(self.zst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_snapshot_unpacks_to_a_manifest() {
        let e = Embedded::BUILTIN;
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
        let yaml = Embedded::BUILTIN
            .decompress()
            .unwrap_or_else(|err| panic!("{err}"));
        let m = crate::Manifest::parse(&yaml, crate::ManifestSource::Cache);
        let m = m.unwrap_or_else(|err| panic!("{err}"));
        assert!(m.meta.games > 10_000, "{} games", m.meta.games);
    }
}
