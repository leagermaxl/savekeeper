//! Path expansion of rule targets and claims (SPEC-04 §4.2, §4.5 step 1.2).
//!
//! Two steps turn a rule template into concrete paths:
//! 1. `PathTemplate::specialize` (SPEC-02 §3.2) replaces the multi-valued
//!    tokens `{DRIVE:*}` and `{STEAM_USERID}` by each of their values, so
//!    every path gets its own template and therefore its own `FindingId`
//!    (FR-04-02);
//! 2. [`glob_paths`] resolves a template and matches its `*` segments against
//!    folder listings (`glob_root`), one level per segment, without walking.

use std::path::PathBuf;

use sk_core::env::Environment;
use sk_core::fs::{EntryKind, EntryMeta, FsScanner, ReparseKind};
use sk_core::template::{PathTemplate, ResolveContext};

/// `FILE_ATTRIBUTE_DIRECTORY`: tells a folder reparse point (cloud
/// placeholder, link, junction) from a file one.
pub(super) const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;

/// A concrete path with the template it was found under.
#[derive(Debug, Clone)]
pub(super) struct Globbed {
    /// The template with every `*` segment replaced by the matched name.
    pub(super) template: PathTemplate,
    /// Absolute path.
    pub(super) path: PathBuf,
    /// Metadata of the path when its last segment was matched by `*`
    /// (from the listing); `None` when it was not listed.
    pub(super) meta: Option<EntryMeta>,
}

/// Whether `template` has `*` segments other than `{DRIVE:*}`.
pub(super) fn has_wildcard(template: &PathTemplate) -> bool {
    crate::compile::wildcard_segments(template) > 0
}

/// Paths of `template`. Segments with `*` (other than `{DRIVE:*}`) are
/// matched without case against the entries of the folder above them; an
/// intermediate match must be a folder, the last one a folder or a file.
/// Links are not followed: junctions and symbolic links never match.
///
/// Paths after the last `*` segment and paths of a template without `*` are
/// not checked for existence.
pub(super) fn glob_paths(
    template: &PathTemplate,
    env: &Environment,
    ctx: &ResolveContext,
    fs: &dyn FsScanner,
) -> Vec<Globbed> {
    let text = template.as_str();
    let segments: Vec<&str> = text.split('\\').collect();
    let (first, rest) = match segments.split_first() {
        Some(split) => split,
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    descend(first, rest, env, ctx, fs, &mut out);
    out
}

/// Expands `rest` below the template `prefix` (already free of `*`).
fn descend(
    prefix: &str,
    rest: &[&str],
    env: &Environment,
    ctx: &ResolveContext,
    fs: &dyn FsScanner,
    out: &mut Vec<Globbed>,
) {
    let Some(star) = rest.iter().position(|s| s.contains('*')) else {
        let full = join(prefix, rest);
        if let Ok(template) = PathTemplate::parse(&full) {
            for path in template.resolve(env, ctx) {
                out.push(Globbed {
                    template: template.clone(),
                    path,
                    meta: None,
                });
            }
        }
        return;
    };
    let head = join(prefix, &rest[..star]);
    let pattern = rest[star];
    let tail = &rest[star + 1..];
    let Ok(head_template) = PathTemplate::parse(&head) else {
        return;
    };
    for base in head_template.resolve(env, ctx) {
        // A missing or unreadable folder simply has no matches.
        let Ok(entries) = fs.read_dir(&base) else {
            continue;
        };
        for entry in entries {
            let Some(name) = entry.path.file_name().and_then(|n| n.to_str()) else {
                continue; // A non-UTF-8 name cannot be part of a template.
            };
            let usable = if tail.is_empty() {
                is_dir(&entry.meta) || is_file(&entry.meta)
            } else {
                is_dir(&entry.meta)
            };
            if !usable || !wildcard_match(pattern, name) {
                continue;
            }
            let matched = format!("{head}\\{name}");
            if tail.is_empty() {
                if let Ok(template) = PathTemplate::parse(&matched) {
                    out.push(Globbed {
                        template,
                        path: entry.path.clone(),
                        meta: Some(entry.meta.clone()),
                    });
                }
            } else {
                descend(&matched, tail, env, ctx, fs, out);
            }
        }
    }
}

fn join(prefix: &str, segments: &[&str]) -> String {
    let mut s = prefix.to_owned();
    for segment in segments {
        s.push('\\');
        s.push_str(segment);
    }
    s
}

/// A folder, including a cloud placeholder folder.
pub(super) fn is_dir(meta: &EntryMeta) -> bool {
    match meta.kind {
        EntryKind::Dir => true,
        EntryKind::Reparse(ReparseKind::CloudPlaceholder) => {
            meta.attrs & FILE_ATTRIBUTE_DIRECTORY != 0
        }
        _ => false,
    }
}

/// A file, including a cloud placeholder file.
pub(super) fn is_file(meta: &EntryMeta) -> bool {
    match meta.kind {
        EntryKind::File => true,
        EntryKind::Reparse(ReparseKind::CloudPlaceholder) => {
            meta.attrs & FILE_ATTRIBUTE_DIRECTORY == 0
        }
        _ => false,
    }
}

/// Whether `name` matches `pattern`, where `*` stands for any run of
/// characters and everything else is literal; case-insensitive.
pub(super) fn wildcard_match(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.to_lowercase().chars().collect();
    let name: Vec<char> = name.to_lowercase().chars().collect();
    let (mut p, mut n) = (0, 0);
    // Position after the last `*` and the name position it was tried at.
    let mut backtrack: Option<(usize, usize)> = None;
    while n < name.len() {
        if p < pattern.len() && pattern[p] == '*' {
            p += 1;
            backtrack = Some((p, n));
        } else if p < pattern.len() && pattern[p] == name[n] {
            p += 1;
            n += 1;
        } else if let Some((after_star, tried)) = backtrack {
            p = after_star;
            n = tried + 1;
            backtrack = Some((after_star, tried + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}
