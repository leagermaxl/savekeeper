//! `FakeProfile`: a user profile materialized in a temporary folder (SPEC-12 §4.2, §4.3).

use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use sk_core::env::{Environment, KnownFolder};
use sk_core::template::{PathTemplate, ResolveContext};
use tempfile::TempDir;
use time::OffsetDateTime;

use crate::fixture::{write_content, ProfileSpec};

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
    /// builds `Environment::fake(root)` with the profile's known folder overrides.
    ///
    /// All known folders are created on disk, then the files of `tree`.
    pub fn load(name: &str) -> Self {
        let file = fixtures_dir().join("profiles").join(format!("{name}.yaml"));
        let src = fs::read_to_string(&file)
            .unwrap_or_else(|e| panic!("cannot read fixture {}: {e}", file.display()));
        Self::from_yaml(&src).unwrap_or_else(|e| panic!("invalid fixture {}: {e}", file.display()))
    }

    /// Materializes a profile description (SPEC-12 §4.3).
    pub(crate) fn from_yaml(src: &str) -> Result<Self, String> {
        let spec = ProfileSpec::parse(src)?;
        let files = spec.files(OffsetDateTime::now_utc())?;
        let root = TempDir::new().map_err(|e| format!("cannot create a temp dir: {e}"))?;
        let mut env = Environment::fake(root.path());
        for (token, relative) in &spec.known_folders {
            let folder = KnownFolder::from_token(token)
                .ok_or_else(|| format!("unknown known folder {token:?}"))?;
            env.known_folders
                .insert(folder, inside(root.path(), relative)?);
        }
        for dir in env.known_folders.values() {
            fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        let profile = Self { root, env };
        for file in files {
            let path = profile.try_path(&file.path)?;
            create_parent(&path)?;
            File::create(&path)
                .and_then(|out| {
                    let mut out = BufWriter::new(out);
                    write_content(&mut out, &file.path, file.size)?;
                    out.flush()
                })
                .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
            if let Some(t) = file.mtime {
                set_modified(&path, t.into())?;
            }
        }
        Ok(profile)
    }

    /// Absolute path inside the root for a template: `"{APPDATA}\\Foo"` →
    /// `<root>/Users/user/AppData/Roaming/Foo`. `/` and `\` both separate.
    ///
    /// Panics if the template is invalid, resolves to nothing or outside the root.
    pub fn path(&self, template: &str) -> PathBuf {
        self.try_path(template).unwrap_or_else(|e| panic!("{e}"))
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

    fn try_path(&self, template: &str) -> Result<PathBuf, String> {
        let parsed = PathTemplate::parse(template)
            .map_err(|e| format!("invalid path template {template:?}: {e}"))?;
        let path = parsed
            .resolve(&self.env, &ResolveContext::default())
            .into_iter()
            .next()
            .ok_or_else(|| format!("path template {template:?} resolves to nothing"))?;
        if !path.starts_with(self.root.path()) {
            return Err(format!(
                "path template {template:?} resolves outside the profile root: {}",
                path.display()
            ));
        }
        Ok(path)
    }
}

/// The repository's `fixtures/` folder.
fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("fixtures")
}

/// `root` joined with a relative `/`-separated path that must stay inside it.
fn inside(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let outside = || format!("folder {relative:?} must be relative to the profile root");
    if relative.starts_with(['/', '\\']) {
        return Err(outside());
    }
    let mut path = root.to_path_buf();
    for part in relative.split(['/', '\\']).filter(|p| !p.is_empty()) {
        if part == "." || part == ".." || part.contains(':') {
            return Err(outside());
        }
        path.push(part);
    }
    if path == root {
        return Err(format!("folder {relative:?} is empty"));
    }
    Ok(path)
}

fn create_parent(path: &Path) -> Result<(), String> {
    match path.parent() {
        Some(parent) => fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display())),
        None => Ok(()),
    }
}

/// Sets the modification time of a file or folder.
fn set_modified(path: &Path, t: SystemTime) -> Result<(), String> {
    open_for_times(path)
        .and_then(|file| file.set_modified(t))
        .map_err(|e| format!("cannot set mtime of {}: {e}", path.display()))
}

#[cfg(windows)]
fn open_for_times(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_WRITE_ATTRIBUTES: u32 = 0x0100;
    // Required to open a folder.
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    OpenOptions::new()
        .access_mode(FILE_WRITE_ATTRIBUTES)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
}

#[cfg(not(windows))]
fn open_for_times(path: &Path) -> std::io::Result<File> {
    OpenOptions::new().read(true).open(path)
}
