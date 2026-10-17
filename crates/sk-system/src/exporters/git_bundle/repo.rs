//! The repository of a `git-bundle` export (SPEC-06 §4.3): where it is, whether it has
//! a work tree, and the unique name of its files in the target folder.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use sk_core::env::Environment;
use sk_core::template::{PathTemplate, ResolveContext};

use crate::exporter::RestoreHint;
use crate::reg_export::slug;

/// Slug of a repository folder whose name has no letters or digits.
pub(crate) const DEFAULT_SLUG: &str = "repo";

/// The repository folder of `params.repo` on this machine: an existing folder.
pub(crate) fn repo_path(env: &Environment, params: &serde_json::Value) -> Option<PathBuf> {
    let text = params.get("repo")?.as_str()?;
    let template = match PathTemplate::parse(text) {
        Ok(template) => template,
        Err(err) => {
            tracing::debug!(error = %err, "invalid git-bundle repo template");
            return None;
        }
    };
    let repo = template
        .resolve(env, &ResolveContext::default())
        .into_iter()
        .next()?;
    if repo.is_dir() {
        Some(repo)
    } else {
        tracing::debug!("git-bundle repository folder not found");
        None
    }
}

/// Whether `repo` has a working copy (`.git` folder or file); a bare repository has none.
pub(crate) fn has_worktree(repo: &Path) -> bool {
    std::fs::symlink_metadata(repo.join(".git")).is_ok()
}

/// The slug of the repository folder name that neither `<slug>.bundle` nor
/// `<slug>-worktree` uses in `target` yet: `-2`, `-3`, … on a collision.
pub(crate) fn unique_slug(repo: &Path, target: &Path) -> String {
    let name = repo
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let base = match slug(&name) {
        slug if slug.is_empty() => DEFAULT_SLUG.to_owned(),
        slug => slug,
    };
    let taken = |slug: &str| {
        let bundle = target.join(format!("{slug}.bundle"));
        let worktree = target.join(format!("{slug}-worktree"));
        std::fs::symlink_metadata(bundle).is_ok() || std::fs::symlink_metadata(worktree).is_ok()
    };
    let mut candidate = base.clone();
    let mut n = 2u32;
    while taken(&candidate) {
        candidate = format!("{base}-{n}");
        n += 1;
    }
    candidate
}

/// The names among `keys` that start with `GIT_` (case-insensitive, as on Windows).
pub(crate) fn inherited_git_vars(keys: impl Iterator<Item = OsString>) -> Vec<OsString> {
    keys.filter(|key| {
        key.to_str()
            .and_then(|key| key.get(..4))
            .is_some_and(|head| head.eq_ignore_ascii_case("GIT_"))
    })
    .collect()
}

/// Removes an output file of this export in the target folder that git left behind
/// after a failure (FR-06-02); never a source file.
pub(crate) fn remove_own_output(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => tracing::warn!(error = %err, "partial bundle file not removed"),
    }
}

/// `git clone <slug>.bundle <slug>` (SPEC-13); the paths are relative to the export
/// folder. The files of `<slug>-worktree/` are copied over the clone by hand.
pub(crate) fn restore_hint(slug: &str) -> RestoreHint {
    RestoreHint::Command {
        program: "git".to_owned(),
        args: vec![
            "clone".to_owned(),
            format!("{slug}.bundle"),
            slug.to_owned(),
        ],
    }
}
