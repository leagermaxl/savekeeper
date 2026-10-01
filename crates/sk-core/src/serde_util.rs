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
