//! Profile description format of `fixtures/profiles/*.yaml` (SPEC-12 §4.3).
//!
//! This module parses a description and expands it into a flat list of items
//! to create. Keys: `known_folders`, `launchers` and `tree` entries with
//! `path` plus either content (`size` or `sample`, with `mtime`, `repeat`,
//! `attrs`) or a git repository (`git`). Unknown keys are rejected.

use std::collections::{BTreeMap, BTreeSet};
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
    /// Game launchers: launcher id (`steam`, `epic` ...) → its description.
    #[serde(default)]
    pub launchers: BTreeMap<String, LauncherSpec>,
    /// Files and repositories of the profile.
    #[serde(default)]
    pub tree: Vec<TreeEntry>,
}

/// One `launchers` entry.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LauncherSpec {
    /// Launcher root folder relative to the profile root; `{STEAM}` for Steam.
    #[serde(default)]
    pub root: Option<String>,
    /// Store account ids; for Steam the id3 (`{STEAM_USERID}`).
    #[serde(default)]
    pub users: Vec<String>,
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
    /// Content copied from `fixtures/samples/<sample>`.
    #[serde(default)]
    pub sample: Option<String>,
    /// Modification time: relative to "now" (`-1d`) or RFC 3339.
    #[serde(default)]
    pub mtime: Option<String>,
    /// Number of files: the last number in the file name is incremented.
    #[serde(default)]
    pub repeat: Option<u32>,
    /// File attributes, applied on Windows only.
    #[serde(default)]
    pub attrs: Vec<Attr>,
    /// A git repository; `path` is its `.git` folder.
    #[serde(default)]
    pub git: Option<GitSpec>,
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

/// A file attribute (`attrs: [hidden, readonly]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Attr {
    /// `FILE_ATTRIBUTE_HIDDEN`.
    Hidden,
    /// `FILE_ATTRIBUTE_READONLY`.
    Readonly,
    /// `FILE_ATTRIBUTE_SYSTEM`.
    System,
}

/// A generated git repository (`git: { commits: 3, dirty: true, remote: null }`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GitSpec {
    /// Number of commits on `main`.
    #[serde(default)]
    pub commits: u32,
    /// The work tree has an uncommitted change of a tracked file
    /// (an untracked file if there are no commits).
    #[serde(default)]
    pub dirty: bool,
    /// URL of the `origin` remote; `null` for no remote.
    #[serde(default)]
    pub remote: Option<String>,
    /// How many of the last commits are not on `origin/main`.
    #[serde(default)]
    pub unpushed: u32,
}

/// Content of a generated file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Content {
    /// Pseudo-random bytes of this size, seeded by the path template.
    Random(u64),
    /// A copy of `fixtures/samples/<name>`.
    Sample(String),
}

/// A file to create, after `repeat` expansion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileSpec {
    /// Path template of this file.
    pub path: String,
    /// Its content.
    pub content: Content,
    /// Modification time to set, if any.
    pub mtime: Option<OffsetDateTime>,
    /// Attributes, sorted and without duplicates.
    pub attrs: Vec<Attr>,
}

/// A git repository to create.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepoSpec {
    /// Path template of the `.git` folder; the work tree is its parent.
    pub path: String,
    /// Repository state.
    pub git: GitSpec,
}

/// Something to create in the profile, in description order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Item {
    /// A file.
    File(FileSpec),
    /// A git repository.
    Repo(RepoSpec),
}

impl ProfileSpec {
    /// Parses a profile description.
    pub fn parse(src: &str) -> Result<Self, String> {
        serde_saphyr::from_str(src).map_err(|e| e.to_string())
    }

    /// Names of the samples the description uses, sorted.
    pub fn samples(&self) -> BTreeSet<&str> {
        self.tree
            .iter()
            .filter_map(|e| e.sample.as_deref())
            .collect()
    }

    /// Expands `tree` into items; relative `mtime` values count from `now`.
    pub fn items(&self, now: OffsetDateTime) -> Result<Vec<Item>, String> {
        let mut items = Vec::new();
        for entry in &self.tree {
            match &entry.git {
                Some(git) => items.push(Item::Repo(repo_item(entry, git)?)),
                None => items.extend(file_items(entry, now)?.into_iter().map(Item::File)),
            }
        }
        Ok(items)
    }
}

fn repo_item(entry: &TreeEntry, git: &GitSpec) -> Result<RepoSpec, String> {
    let path = &entry.path;
    let has_other_keys = entry.size.is_some()
        || entry.sample.is_some()
        || entry.mtime.is_some()
        || entry.repeat.is_some()
        || !entry.attrs.is_empty();
    if has_other_keys {
        return Err(format!(
            "{path:?}: `git` cannot be combined with `size`, `sample`, `mtime`, `repeat` or `attrs`"
        ));
    }
    let name = path.rsplit(['/', '\\']).next().unwrap_or_default();
    if name != ".git" {
        return Err(format!("{path:?}: a `git` entry must be a `.git` folder"));
    }
    if git.remote.is_none() && git.unpushed > 0 {
        return Err(format!("{path:?}: `unpushed` needs a `remote`"));
    }
    if git.unpushed > git.commits {
        return Err(format!("{path:?}: `unpushed` is greater than `commits`"));
    }
    Ok(RepoSpec {
        path: path.clone(),
        git: git.clone(),
    })
}

fn file_items(entry: &TreeEntry, now: OffsetDateTime) -> Result<Vec<FileSpec>, String> {
    let content = match (&entry.size, &entry.sample) {
        (Some(_), Some(_)) => {
            return Err(format!(
                "{:?}: `size` and `sample` are mutually exclusive",
                entry.path
            ))
        }
        (None, None) => Content::Random(0),
        (Some(Size::Bytes(n)), None) => Content::Random(*n),
        (Some(Size::Text(s)), None) => Content::Random(parse_size(s)?),
        (None, Some(sample)) => Content::Sample(sample_name(sample)?),
    };
    let mtime = entry
        .mtime
        .as_deref()
        .map(|s| parse_mtime(s, now))
        .transpose()?;
    let mut attrs = entry.attrs.clone();
    attrs.sort();
    attrs.dedup();
    let paths = match entry.repeat {
        None => vec![entry.path.clone()],
        Some(n) => expand_repeat(&entry.path, n)?,
    };
    Ok(paths
        .into_iter()
        .map(|path| FileSpec {
            path,
            content: content.clone(),
            mtime,
            attrs: attrs.clone(),
        })
        .collect())
}

/// A sample name: a relative `/`-separated path inside `fixtures/samples`.
fn sample_name(sample: &str) -> Result<String, String> {
    let parts: Vec<&str> = sample.split(['/', '\\']).collect();
    let valid = parts
        .iter()
        .all(|p| !p.is_empty() && *p != "." && *p != ".." && !p.contains(':'));
    if !valid {
        return Err(format!(
            "sample {sample:?} must be a relative path inside fixtures/samples"
        ));
    }
    Ok(parts.join("/"))
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
#[path = "fixture_tests.rs"]
mod tests;
