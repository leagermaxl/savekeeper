//! Serde helpers of the backup types.

/// `PathBuf` as a UTF-8 string with lossy replacement (SPEC-00 §7.1, SPEC-02 §5).
pub(crate) mod lossy_path {
    use std::path::{Path, PathBuf};

    use serde::{Deserialize, Deserializer, Serializer};

    pub(crate) fn serialize<S: Serializer>(path: &Path, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&path.to_string_lossy())
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<PathBuf, D::Error> {
        String::deserialize(deserializer).map(PathBuf::from)
    }
}
