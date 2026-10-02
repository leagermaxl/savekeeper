//! Profile description format of `fixtures/profiles/*.yaml` (SPEC-12 §4.3).
//!
//! This module parses a description and expands it into a flat list of files.
//! Supported keys: `known_folders` and `tree` entries with `path`, `size`,
//! `mtime` and `repeat`. Unknown keys are rejected.

use std::collections::BTreeMap;
use std::io::Write;

use serde::Deserialize;
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};

/// A parsed profile description.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProfileSpec {
    /// Known folder overrides: token name without braces → folder relative to the root.
    #[serde(default)]
    pub known_folders: BTreeMap<String, String>,
    /// Files of the profile.
    #[serde(default)]
    pub tree: Vec<TreeEntry>,
}

/// One `tree` entry.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TreeEntry {
    /// Path template with `/` or `\` separators.
    pub path: String,
    /// File size; the content is pseudo-random, seeded by the path.
    #[serde(default)]
    pub size: Option<Size>,
    /// Modification time: relative to "now" (`-1d`) or RFC 3339.
    #[serde(default)]
    pub mtime: Option<String>,
    /// Number of files: the last number in the file name is incremented.
    #[serde(default)]
    pub repeat: Option<u32>,
}

/// A size: a number of bytes or a string such as `12 KiB`.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum Size {
    /// Bytes.
    Bytes(u64),
    /// `<number> <unit>`, unit one of B, KiB, MiB, GiB.
    Text(String),
}

/// A file to create, after `repeat` expansion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileSpec {
    /// Path template of this file.
    pub path: String,
    /// Size in bytes.
    pub size: u64,
    /// Modification time to set, if any.
    pub mtime: Option<OffsetDateTime>,
}

impl ProfileSpec {
    /// Parses a profile description.
    pub fn parse(src: &str) -> Result<Self, String> {
        serde_saphyr::from_str(src).map_err(|e| e.to_string())
    }

    /// Expands `tree` into files; relative `mtime` values count from `now`.
    pub fn files(&self, now: OffsetDateTime) -> Result<Vec<FileSpec>, String> {
        let mut files = Vec::new();
        for entry in &self.tree {
            let size = match &entry.size {
                None => 0,
                Some(Size::Bytes(n)) => *n,
                Some(Size::Text(s)) => parse_size(s)?,
            };
            let mtime = entry
                .mtime
                .as_deref()
                .map(|s| parse_mtime(s, now))
                .transpose()?;
            let paths = match entry.repeat {
                None => vec![entry.path.clone()],
                Some(n) => expand_repeat(&entry.path, n)?,
            };
            files.extend(paths.into_iter().map(|path| FileSpec { path, size, mtime }));
        }
        Ok(files)
    }
}

/// Parses `28311552`, `12 KiB`, `1MiB`, `3 GiB`, `10 B`.
pub(crate) fn parse_size(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (number, unit) = s.split_at(split);
    let number: u64 = number
        .parse()
        .map_err(|_| format!("invalid size {s:?}: expected `<number> <unit>`"))?;
    let factor: u64 = match unit.trim() {
        "" | "B" => 1,
        "KiB" => 1 << 10,
        "MiB" => 1 << 20,
        "GiB" => 1 << 30,
        other => return Err(format!("invalid size unit {other:?} in {s:?}")),
    };
    number
        .checked_mul(factor)
        .ok_or_else(|| format!("size {s:?} is too large"))
}

/// Parses `-1d`, `-2h`, `-30m`, `-10s`, `-1w` (also with `+`) relative to `now`,
/// or an RFC 3339 timestamp.
pub(crate) fn parse_mtime(s: &str, now: OffsetDateTime) -> Result<OffsetDateTime, String> {
    let s = s.trim();
    let Some(sign) = s.chars().next().filter(|c| matches!(c, '-' | '+')) else {
        return OffsetDateTime::parse(s, &Rfc3339).map_err(|e| format!("invalid mtime {s:?}: {e}"));
    };
    let rest = &s[1..];
    let split = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    let (number, unit) = rest.split_at(split);
    let number: i64 = number
        .parse()
        .map_err(|_| format!("invalid mtime {s:?}: expected e.g. `-1d`"))?;
    let unit = match unit {
        "s" => Duration::SECOND,
        "m" => Duration::MINUTE,
        "h" => Duration::HOUR,
        "d" => Duration::DAY,
        "w" => Duration::WEEK,
        other => return Err(format!("invalid mtime unit {other:?} in {s:?}")),
    };
    let offset = unit
        .checked_mul(i32::try_from(number).map_err(|_| format!("mtime {s:?} is too large"))?)
        .ok_or_else(|| format!("mtime {s:?} is too large"))?;
    let t = if sign == '-' {
        now.checked_sub(offset)
    } else {
        now.checked_add(offset)
    };
    t.ok_or_else(|| format!("mtime {s:?} is out of range"))
}

/// `f_000001` with `repeat: 3` → `f_000001`, `f_000002`, `f_000003`: the last
/// run of digits in the file name counts up from its value, keeping its width.
pub(crate) fn expand_repeat(path: &str, count: u32) -> Result<Vec<String>, String> {
    let name_start = path.rfind(['/', '\\']).map_or(0, |i| i + 1);
    let name = &path[name_start..];
    let end = name
        .rfind(|c: char| c.is_ascii_digit())
        .map(|i| name_start + i + 1)
        .ok_or_else(|| format!("`repeat` needs a number in the file name: {path:?}"))?;
    let start = path[..end]
        .rfind(|c: char| !c.is_ascii_digit())
        .map_or(0, |i| i + 1)
        .max(name_start);
    let digits = &path[start..end];
    let first: u64 = digits
        .parse()
        .map_err(|_| format!("number in {path:?} is too large"))?;
    let width = digits.len();
    Ok((0..u64::from(count))
        .map(|k| {
            format!(
                "{}{:0width$}{}",
                &path[..start],
                first + k,
                &path[end..],
                width = width
            )
        })
        .collect())
}

/// Writes `size` bytes of deterministic pseudo-random content seeded by `seed`.
pub(crate) fn write_content(out: &mut impl Write, seed: &str, size: u64) -> std::io::Result<()> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(seed.as_bytes());
    let mut reader = hasher.finalize_xof();
    let mut buf = vec![0u8; 64 * 1024];
    let mut left = size;
    while left > 0 {
        let n = usize::try_from(left).map_or(buf.len(), |l| l.min(buf.len()));
        reader.fill(&mut buf[..n]);
        out.write_all(&buf[..n])?;
        left -= n as u64;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(parse_size("28311552"), Ok(28_311_552));
        assert_eq!(parse_size("12 KiB"), Ok(12 * 1024));
        assert_eq!(parse_size("1MiB"), Ok(1024 * 1024));
        assert_eq!(parse_size("2 GiB"), Ok(2 << 30));
        assert_eq!(parse_size("10 B"), Ok(10));
        assert!(parse_size("12 KB").is_err());
        assert!(parse_size("KiB").is_err());
        assert!(parse_size("99999999999999999999 GiB").is_err());
    }

    #[test]
    fn mtimes() {
        let now = datetime!(2026-10-02 12:00 UTC);
        assert_eq!(parse_mtime("-1d", now), Ok(datetime!(2026-10-01 12:00 UTC)));
        assert_eq!(parse_mtime("-2h", now), Ok(datetime!(2026-10-02 10:00 UTC)));
        assert_eq!(
            parse_mtime("+30m", now),
            Ok(datetime!(2026-10-02 12:30 UTC))
        );
        assert_eq!(parse_mtime("-1w", now), Ok(datetime!(2026-09-25 12:00 UTC)));
        assert_eq!(
            parse_mtime("2020-01-02T03:04:05Z", now),
            Ok(datetime!(2020-01-02 03:04:05 UTC))
        );
        assert!(parse_mtime("-1y", now).is_err());
        assert!(parse_mtime("-d", now).is_err());
        assert!(parse_mtime("yesterday", now).is_err());
    }

    #[test]
    fn repeat_keeps_width_and_counts_from_value() {
        assert_eq!(
            expand_repeat("{LOCALAPPDATA}/discord/Cache/f_000001", 3).unwrap(),
            [
                "{LOCALAPPDATA}/discord/Cache/f_000001",
                "{LOCALAPPDATA}/discord/Cache/f_000002",
                "{LOCALAPPDATA}/discord/Cache/f_000003",
            ]
        );
        assert_eq!(
            expand_repeat("{HOME}/v2/save9.dat", 2).unwrap(),
            ["{HOME}/v2/save9.dat", "{HOME}/v2/save10.dat"]
        );
        assert!(expand_repeat("{HOME}/dir2/file.dat", 2).is_err());
    }

    #[test]
    fn content_is_deterministic_and_sized() {
        let mut a = Vec::new();
        let mut b = Vec::new();
        write_content(&mut a, "{APPDATA}/x", 100_000).unwrap();
        write_content(&mut b, "{APPDATA}/x", 100_000).unwrap();
        assert_eq!(a.len(), 100_000);
        assert_eq!(a, b);
        let mut c = Vec::new();
        write_content(&mut c, "{APPDATA}/y", 100_000).unwrap();
        assert_ne!(a, c);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let err = ProfileSpec::parse("tree:\n  - path: \"{HOME}/a\"\n    color: red\n");
        assert!(err.is_err());
    }

    #[test]
    fn expands_tree() {
        let spec = ProfileSpec::parse(
            r#"
known_folders:
  DOCUMENTS: "OneDrive/Documents"
tree:
  - path: "{APPDATA}/Game/save.sl2"
    size: 12 KiB
    mtime: "-1d"
  - path: "{LOCALAPPDATA}/c/f_01"
    size: 3
    repeat: 2
  - path: "{HOME}/empty.txt"
"#,
        )
        .unwrap();
        assert_eq!(spec.known_folders["DOCUMENTS"], "OneDrive/Documents");
        let now = datetime!(2026-10-02 12:00 UTC);
        let files = spec.files(now).unwrap();
        let summary: Vec<_> = files
            .iter()
            .map(|f| (f.path.as_str(), f.size, f.mtime))
            .collect();
        assert_eq!(
            summary,
            [
                (
                    "{APPDATA}/Game/save.sl2",
                    12 * 1024,
                    Some(datetime!(2026-10-01 12:00 UTC))
                ),
                ("{LOCALAPPDATA}/c/f_01", 3, None),
                ("{LOCALAPPDATA}/c/f_02", 3, None),
                ("{HOME}/empty.txt", 0, None),
            ]
        );
    }
}
