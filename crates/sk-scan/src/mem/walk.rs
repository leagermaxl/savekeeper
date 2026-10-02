//! Depth-first traversal of [`MemFs::walk`] (SPEC-03 §4.1 «Поведение walk»).

use std::ffi::OsStr;
use std::path::Path;

use super::{Key, MemFs, MemNode, FILE_ATTRIBUTE_DIRECTORY};
use crate::exclude::name_matches;
use crate::{
    CancellationToken, DirEntryInfo, Exclusion, FsError, FsScanner, WalkControl, WalkOptions,
    WalkStats,
};

/// Depth-first, sorted traversal state of [`MemFs::walk`].
pub(super) struct Walker<'a, 'v> {
    pub(super) fs: &'a MemFs,
    pub(super) opts: &'a WalkOptions,
    pub(super) visit: &'v mut dyn FnMut(&DirEntryInfo) -> WalkControl,
    pub(super) cancel: &'a CancellationToken,
    pub(super) stats: WalkStats,
    pub(super) stopped: bool,
}

impl Walker<'_, '_> {
    /// Visits the children of the folder `key` at `abs`; `rel` and `rel_glob`
    /// (`/`-separated, for glob matching) are relative to the walk root.
    pub(super) fn dir(
        &mut self,
        abs: &Path,
        key: &[String],
        rel: &Path,
        rel_glob: &str,
        depth: u32,
    ) -> Result<(), FsError> {
        let listing = self.fs.list(key)?;
        for &(child_key, node) in &listing {
            if self.cancel.is_cancelled() {
                return Err(FsError::Cancelled);
            }
            let path = abs.join(&node.name);
            let child_glob = if rel_glob.is_empty() {
                node.name.clone()
            } else {
                format!("{rel_glob}/{}", node.name)
            };
            let excluded = self.is_excluded(&path, child_key, node, &listing)
                || self
                    .opts
                    .exclude
                    .as_ref()
                    .is_some_and(|g| g.is_match(&child_glob));
            if excluded {
                self.stats.skipped_excluded += 1;
                continue;
            }
            // `include` selects files; folders are always entered.
            let included = node.is_dir()
                || self
                    .opts
                    .include
                    .as_ref()
                    .is_none_or(|g| g.is_match(&child_glob));
            if !included {
                continue;
            }
            if self.stats.entries >= self.opts.max_entries {
                self.stats.truncated = true;
                self.stopped = true;
                return Ok(());
            }
            self.stats.entries += 1;
            let info = DirEntryInfo {
                path,
                rel: rel.join(&node.name),
                depth,
                meta: node.meta(),
            };
            match (self.visit)(&info) {
                WalkControl::Stop => {
                    self.stopped = true;
                    return Ok(());
                }
                WalkControl::SkipDir => continue,
                WalkControl::Continue => {}
            }
            if node.is_walked() && depth < self.opts.max_depth {
                self.dir(&info.path, child_key, &info.rel, &child_glob, depth + 1)?;
                if self.stopped {
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    /// Asks the global filter and resolves its conditions (SPEC-03 §4.2 step 3):
    /// siblings from the same `listing`, a child file by one metadata lookup.
    fn is_excluded(
        &self,
        path: &Path,
        key: &Key,
        node: &MemNode,
        listing: &[(&Key, &MemNode)],
    ) -> bool {
        match self
            .opts
            .excludes
            .check(path, OsStr::new(&node.name), node.is_dir())
        {
            Exclusion::Keep => false,
            Exclusion::Exclude => true,
            Exclusion::ExcludeIfSibling(glob) => listing
                .iter()
                .any(|(k, n)| *k != key && name_matches(glob, &n.name)),
            Exclusion::ExcludeIfChild(file) => {
                node.is_dir()
                    && self
                        .fs
                        .metadata(&path.join(file))
                        .is_ok_and(|m| m.attrs & FILE_ATTRIBUTE_DIRECTORY == 0)
            }
        }
    }
}
