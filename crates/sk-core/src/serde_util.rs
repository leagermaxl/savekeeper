//! Serde helpers shared by the domain types.

/// `PathBuf` as a UTF-8 string with lossy replacement (SPEC-02 §5).
///
/// Plain serde fails on a path without a UTF-8 representation; the JSON contract
/// requires a lossy string instead.
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

/// `Option<PathBuf>` as an optional lossy UTF-8 string.
pub(crate) mod lossy_path_opt {
    use std::path::PathBuf;

    use serde::{Deserialize, Deserializer, Serializer};

    pub(crate) fn serialize<S: Serializer>(
        path: &Option<PathBuf>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match path {
            Some(path) => serializer.serialize_some(&path.to_string_lossy()),
            None => serializer.serialize_none(),
        }
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<PathBuf>, D::Error> {
        Option::<String>::deserialize(deserializer).map(|s| s.map(PathBuf::from))
    }
}

/// `Vec<PathBuf>` as lossy UTF-8 strings.
pub(crate) mod lossy_path_vec {
    use std::path::PathBuf;

    use serde::{Deserialize, Deserializer, Serializer};

    pub(crate) fn serialize<S: Serializer>(
        paths: &[PathBuf],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(paths.iter().map(|p| p.to_string_lossy()))
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<PathBuf>, D::Error> {
        Vec::<String>::deserialize(deserializer).map(|v| v.into_iter().map(PathBuf::from).collect())
    }
}

/// `BTreeMap<K, PathBuf>` with lossy UTF-8 string values.
pub(crate) mod lossy_path_map {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub(crate) fn serialize<K: Serialize, S: Serializer>(
        map: &BTreeMap<K, PathBuf>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_map(map.iter().map(|(k, v)| (k, v.to_string_lossy())))
    }

    pub(crate) fn deserialize<'de, K, D>(deserializer: D) -> Result<BTreeMap<K, PathBuf>, D::Error>
    where
        K: Deserialize<'de> + Ord,
        D: Deserializer<'de>,
    {
        let map = BTreeMap::<K, String>::deserialize(deserializer)?;
        Ok(map
            .into_iter()
            .map(|(k, v)| (k, PathBuf::from(v)))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde::{Deserialize, Serialize};

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Wrapper(#[serde(with = "super::lossy_path")] PathBuf);

    #[test]
    fn path_round_trips_as_string() {
        let path = PathBuf::from(r"C:\Users\Макс\AppData\Roaming\Code");
        let json = serde_json::to_string(&Wrapper(path.clone())).unwrap();
        assert_eq!(json, r#""C:\\Users\\Макс\\AppData\\Roaming\\Code""#);
        assert_eq!(
            serde_json::from_str::<Wrapper>(&json).unwrap(),
            Wrapper(path)
        );
    }

    #[cfg(windows)]
    #[test]
    fn non_utf8_path_serializes_lossy() {
        use std::ffi::OsString;
        use std::os::windows::ffi::OsStringExt;

        // "a" + unpaired surrogate + "b": valid on NTFS, not valid UTF-8.
        let path = PathBuf::from(OsString::from_wide(&[0x61, 0xD800, 0x62]));
        let json = serde_json::to_string(&Wrapper(path)).unwrap();
        assert_eq!(json, "\"a\u{FFFD}b\"");
    }
}
