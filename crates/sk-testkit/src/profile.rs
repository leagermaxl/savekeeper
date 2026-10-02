//! `FakeProfile`: a user profile materialized in a temporary folder (SPEC-12 §4.2, §4.3).

use std::fs;
use std::path::PathBuf;

use sk_core::env::Environment;
use tempfile::TempDir;
use time::OffsetDateTime;

use crate::materialize::{create_parent, materialize_profile, resolve, set_modified};

/// A fake user profile: files under a temporary root and an
/// [`Environment::fake`] whose known folders point inside it.
///
/// Test helper: every failure panics with a message naming the cause.
#[derive(Debug)]
pub struct FakeProfile {
    /// Temporary root folder; removed when the profile is dropped.
    pub root: TempDir,
    /// Environment with all known folders under `root`.
    pub env: Environment,
}

impl FakeProfile {
    /// Materializes `fixtures/profiles/<name>.yaml` in a temporary folder and
    /// builds `Environment::fake(root)` with the profile's known folder
    /// overrides and launchers.
    ///
    /// All known folders and launcher roots are created on disk, then the
    /// items of `tree`. Generated content is cached in `target/fixtures-cache`
    /// and copied (see [`materialize_profile`]).
    pub fn load(name: &str) -> Self {
        let root = TempDir::new().unwrap_or_else(|e| panic!("cannot create a temp dir: {e}"));
        let env = materialize_profile(name, root.path())
            .unwrap_or_else(|e| panic!("fixture profile {name:?}: {e}"));
        Self { root, env }
    }

    /// Materializes a profile description without the cache (SPEC-12 §4.3).
    #[cfg(test)]
    pub(crate) fn from_yaml(src: &str) -> Result<Self, String> {
        let root = TempDir::new().map_err(|e| format!("cannot create a temp dir: {e}"))?;
        let env = crate::materialize::materialize(src, root.path(), None)?;
        Ok(Self { root, env })
    }

    /// Absolute path inside the root for a template: `"{APPDATA}\\Foo"` →
    /// `<root>/Users/user/AppData/Roaming/Foo`. `/` and `\` both separate.
    ///
    /// Panics if the template is invalid, resolves to nothing or outside the root.
    pub fn path(&self, template: &str) -> PathBuf {
        resolve(&self.env, self.root.path(), template).unwrap_or_else(|e| panic!("{e}"))
    }

    /// Writes a file at a template path, creating parent folders.
    pub fn write(&self, template: &str, content: impl AsRef<[u8]>) {
        let path = self.path(template);
        create_parent(&path).unwrap_or_else(|e| panic!("{e}"));
        fs::write(&path, content)
            .unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
    }

    /// Sets the modification time of an existing file or folder.
    pub fn set_mtime(&self, template: &str, t: OffsetDateTime) {
        let path = self.path(template);
        set_modified(&path, t.into()).unwrap_or_else(|e| panic!("{e}"));
    }
}
