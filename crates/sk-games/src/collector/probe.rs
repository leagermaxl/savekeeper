//! Checks of manifest paths on the file system (§4.6 step 4, §4.7 step 2):
//! the resolved root exists, and, for an entry with include globs, a file
//! under it matches them.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use globset::{GlobBuilder, GlobMatcher, GlobSet, GlobSetBuilder};
use sk_core::fs::{
    EntryKind, EntryMeta, Exclusion, FsScanner, PathFilter, WalkControl, WalkOptions,
};
use sk_core::template::{PathTemplate, ResolveContext};

use super::Scan;

/// `FILE_ATTRIBUTE_DIRECTORY`.
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;

/// Entries one include probe looks at, at most; a folder this large is not
/// searched further and counts as not matching.
const MAX_PROBE_ENTRIES: u64 = 200_000;

/// Existing paths of `template` and whether each is a folder: see
/// [`probe_path`].
pub(super) fn existing(
    scan: &Scan<'_>,
    template: &PathTemplate,
    include: &[String],
    ctx: &ResolveContext,
) -> Vec<(PathBuf, bool)> {
    template
        .resolve(scan.env, ctx)
        .iter()
        .filter_map(|path| probe_path(scan, path, include))
        .collect()
}

/// `path` and whether it is a folder, if it can be a target: without
/// include globs any existing entry; with them a folder in which some file
/// matches the globs (as `measure` applies them: relative to the folder,
/// case-insensitive, `*` does not cross `\`).
pub(super) fn probe_path(
    scan: &Scan<'_>,
    path: &Path,
    include: &[String],
) -> Option<(PathBuf, bool)> {
    let meta = scan.fs.metadata(path).ok()?;
    let dir = folder_like(&meta);
    if include.is_empty() {
        return Some((path.to_path_buf(), dir));
    }
    (dir && include_matches(scan, path, include)).then(|| (path.to_path_buf(), true))
}

/// Whether `path` is a folder or a link to one on `fs`.
pub(super) fn is_folder(fs: &dyn FsScanner, path: &Path) -> bool {
    fs.metadata(path).is_ok_and(|m| folder_like(&m))
}

/// A directory, or a reparse point (link, junction, cloud placeholder) with
/// `FILE_ATTRIBUTE_DIRECTORY`.
pub(super) fn folder_like(meta: &EntryMeta) -> bool {
    match meta.kind {
        EntryKind::Dir => true,
        EntryKind::File => false,
        EntryKind::Reparse(_) => meta.attrs & FILE_ATTRIBUTE_DIRECTORY != 0,
    }
}

/// Whether a file under `root` matches `include`; stops at the first one.
/// Folders that cannot lead to a match ([`Prefixes`]) are not entered.
fn include_matches(scan: &Scan<'_>, root: &Path, include: &[String]) -> bool {
    let Some(globs) = glob_set(include) else {
        return false;
    };
    let prefixes = Prefixes::new(include);
    let opts = WalkOptions {
        max_depth: scan.max_depth,
        max_entries: MAX_PROBE_ENTRIES,
        follow_links: false,
        excludes: Arc::new(KeepAll),
        include: Some(globs),
        exclude: None,
        threads: 1,
    };
    let mut found = false;
    let mut visit = |entry: &sk_core::fs::DirEntryInfo| {
        // `include` selects files; folders are always reported.
        if folder_like(&entry.meta) {
            if prefixes.may_contain(&entry.rel) {
                WalkControl::Continue
            } else {
                WalkControl::SkipDir
            }
        } else {
            found = true;
            WalkControl::Stop
        }
    };
    if let Err(e) = scan.fs.walk(root, &opts, &mut visit, scan.cancel) {
        tracing::debug!(root = %root.display(), error = %e, "include probe failed");
    }
    found
}

/// The include globs as `measure` compiles them (SPEC-03 §4.3); `None` when
/// one does not compile.
fn glob_set(include: &[String]) -> Option<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in include {
        let glob = GlobBuilder::new(pattern)
            .case_insensitive(true)
            .literal_separator(true)
            .build();
        match glob {
            Ok(glob) => {
                builder.add(glob);
            }
            Err(e) => {
                tracing::debug!(pattern, error = %e, "manifest include glob skipped");
                return None;
            }
        }
    }
    builder.build().ok()
}

/// One `/`-separated segment of an include glob.
#[derive(Debug)]
enum Segment {
    /// Matches one path component (case-insensitive, as [`glob_set`]).
    One(GlobMatcher),
    /// `**`, or a segment that cannot be judged alone: anything below can
    /// match, so no folder is pruned from here on.
    Rest,
}

/// Segment-prefix pruning of the include probe (T-05-14): a folder is
/// entered only if its path relative to the probe root matches the leading
/// segments of some include glob and is shorter than that glob, so a file
/// below it can still match. E.g. with `[User]/AppData/Roaming/Nitroplus`
/// under `C:\Users`, no user folder is entered (`[User]` is one character).
#[derive(Debug)]
struct Prefixes(Vec<Vec<Segment>>);

impl Prefixes {
    /// Splits each glob of `include` into segments; a segment that does not
    /// compile on its own (e.g. a part of `[/]`) or contains `**` together
    /// with other text disables pruning below it.
    fn new(include: &[String]) -> Self {
        let globs = include
            .iter()
            .map(|pattern| {
                let mut segments = Vec::new();
                for text in pattern.split('/') {
                    let segment = if text.contains("**") {
                        Segment::Rest
                    } else {
                        GlobBuilder::new(text)
                            .case_insensitive(true)
                            .literal_separator(true)
                            .build()
                            .map_or(Segment::Rest, |glob| Segment::One(glob.compile_matcher()))
                    };
                    let rest = matches!(segment, Segment::Rest);
                    segments.push(segment);
                    if rest {
                        break;
                    }
                }
                segments
            })
            .collect();
        Self(globs)
    }

    /// Whether a file below the folder at `rel` (relative to the probe root,
    /// not empty) can match one of the globs.
    fn may_contain(&self, rel: &Path) -> bool {
        let components: Vec<&OsStr> = rel.iter().collect();
        self.0.iter().any(|segments| {
            for (i, name) in components.iter().enumerate() {
                match segments.get(i) {
                    None => return false,
                    Some(Segment::Rest) => return true,
                    Some(Segment::One(glob)) if glob.is_match(Path::new(name)) => {}
                    Some(Segment::One(_)) => return false,
                }
            }
            components.len() < segments.len()
        })
    }
}

/// The probe looks at every entry: it only asks whether a file exists.
#[derive(Debug)]
struct KeepAll;

impl PathFilter for KeepAll {
    fn check(&self, _: &Path, _: &OsStr, _: bool) -> Exclusion {
        Exclusion::Keep
    }
}

#[cfg(test)]
#[path = "probe_tests.rs"]
mod tests;
