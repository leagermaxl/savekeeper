//! The shared fixture materializer (SPEC-12 §4.3): a profile description →
//! folders, files and git repositories under a root, plus the matching
//! [`Environment`]. Used by [`FakeProfile`](crate::FakeProfile) and
//! `cargo xtask fixtures`.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use sk_core::env::{Environment, KnownFolder, LauncherInfo, StoreUser};
use sk_core::template::{PathTemplate, ResolveContext};
use time::OffsetDateTime;

use crate::cache;
use crate::fixture::{write_content, Attr, Content, Item, ProfileSpec};
use crate::fixture_fs::Format;
use crate::git;

/// Steam id64 of the account with id3 `0`.
const STEAM_ID64_BASE: u64 = 76_561_197_960_265_728;

/// Materializes `fixtures/profiles/<name>.yaml` into the folder `dest` and
/// returns the environment of the profile (SPEC-12 §4.3).
///
/// `dest` is created if missing and must be empty. Generated content is cached
/// in `target/fixtures-cache` by a hash of the description and its samples and
/// copied from there; modification times from the description are relative to
/// the time of this call. This is what [`FakeProfile::load`](crate::FakeProfile::load)
/// does in a temporary folder and what `cargo xtask fixtures` does in `target/fixtures`.
pub fn materialize_profile(name: &str, dest: &Path) -> Result<Environment, String> {
    let file = fixtures_dir().join("profiles").join(format!("{name}.yaml"));
    let src = fs::read_to_string(&file)
        .map_err(|e| format!("cannot read fixture {}: {e}", file.display()))?;
    fs::create_dir_all(dest).map_err(|e| format!("cannot create {}: {e}", dest.display()))?;
    let not_empty = fs::read_dir(dest)
        .map_err(|e| format!("cannot read {}: {e}", dest.display()))?
        .next()
        .is_some();
    if not_empty {
        return Err(format!("{} is not empty", dest.display()));
    }
    materialize(&src, dest, Some(name)).map_err(|e| format!("fixture {}: {e}", file.display()))
}

/// Materializes a description into the empty folder `dest`; with `cache_name`
/// the generated content comes from the cache.
pub(crate) fn materialize(
    src: &str,
    dest: &Path,
    cache_name: Option<&str>,
) -> Result<Environment, String> {
    let spec = ProfileSpec::parse(src)?;
    let items = spec.items(OffsetDateTime::now_utc(), Format::Profile)?;
    let samples = fixtures_dir().join("samples");
    let cached = match cache_name {
        Some(name) => {
            let key = cache::key(src, &spec, &samples)?;
            cache::get_or_create(name, &key, |dir| {
                generate(&environment(&spec, dir)?, dir, &items, &samples)
            })?
        }
        None => None,
    };
    let env = environment(&spec, dest)?;
    match cached {
        Some(dir) => copy_tree(&dir, dest)?,
        None => generate(&env, dest, &items, &samples)?,
    }
    apply_mtimes(&env, dest, &items)?;
    Ok(env)
}

/// The repository's `fixtures/` folder.
pub(crate) fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("fixtures")
}

/// `Environment::fake(root)` with the known folder overrides and launchers of `spec`.
pub(crate) fn environment(spec: &ProfileSpec, root: &Path) -> Result<Environment, String> {
    let mut env = Environment::fake(root);
    for (token, relative) in &spec.known_folders {
        let folder = KnownFolder::from_token(token)
            .ok_or_else(|| format!("unknown known folder {token:?}"))?;
        env.known_folders.insert(folder, inside(root, relative)?);
    }
    for (id, launcher) in &spec.launchers {
        env.launchers.push(LauncherInfo {
            id: id.clone(),
            root: launcher
                .root
                .as_deref()
                .map(|r| inside(root, r))
                .transpose()?,
            user_ids: launcher.users.iter().map(|u| store_user(id, u)).collect(),
            games: Vec::new(),
        });
    }
    Ok(env)
}

/// A store account; for Steam `id` is the id3 and `alt_id` the id64.
fn store_user(launcher: &str, id: &str) -> StoreUser {
    let alt_id = (launcher == "steam")
        .then(|| id.parse::<u64>().ok())
        .flatten()
        .and_then(|id3| id3.checked_add(STEAM_ID64_BASE))
        .map(|id64| id64.to_string());
    StoreUser {
        id: id.to_owned(),
        alt_id,
        name: None,
    }
}

/// Absolute path of a template; it must resolve inside the profile root.
pub(crate) fn resolve(env: &Environment, root: &Path, template: &str) -> Result<PathBuf, String> {
    let parsed = PathTemplate::parse(template)
        .map_err(|e| format!("invalid path template {template:?}: {e}"))?;
    let path = parsed
        .resolve(env, &ResolveContext::default())
        .into_iter()
        .next()
        .ok_or_else(|| format!("path template {template:?} resolves to nothing"))?;
    if !path.starts_with(root) {
        return Err(format!(
            "path template {template:?} resolves outside the profile root: {}",
            path.display()
        ));
    }
    Ok(path)
}

/// Creates the known folders, launcher roots, files and repositories.
fn generate(env: &Environment, root: &Path, items: &[Item], samples: &Path) -> Result<(), String> {
    let launcher_roots = env.launchers.iter().filter_map(|l| l.root.as_ref());
    for dir in env.known_folders.values().chain(launcher_roots) {
        fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    for item in items {
        match item {
            Item::File(file) => {
                let path = resolve(env, root, &file.path)?;
                create_parent(&path)?;
                write_file(&path, &file.path, &file.content, &file.attrs, samples)
                    .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
            }
            Item::Dir(dir) => {
                let path = resolve(env, root, &dir.path)?;
                fs::create_dir_all(&path)
                    .map_err(|e| format!("cannot create {}: {e}", path.display()))?;
            }
            Item::Repo(repo) => git::create(&resolve(env, root, &repo.path)?, &repo.git)?,
        }
    }
    Ok(())
}

fn write_file(
    path: &Path,
    seed: &str,
    content: &Content,
    attrs: &[Attr],
    samples: &Path,
) -> io::Result<()> {
    let mut out = BufWriter::new(create_file(path, win_attributes(attrs))?);
    match content {
        Content::Random(size) => write_content(&mut out, seed, *size)?,
        Content::Sample(name) => {
            let sample = samples.join(name);
            let mut src = File::open(&sample).map_err(|e| {
                io::Error::new(
                    e.kind(),
                    format!("cannot read sample {}: {e}", sample.display()),
                )
            })?;
            io::copy(&mut src, &mut out)?;
        }
        Content::Text(text) => out.write_all(text.as_bytes())?,
    }
    out.flush()
}

/// `FILE_ATTRIBUTE_*` flags of `attrs`.
fn win_attributes(attrs: &[Attr]) -> u32 {
    attrs
        .iter()
        .map(|a| match a {
            Attr::Readonly => 0x1,
            Attr::Hidden => 0x2,
            Attr::System => 0x4,
        })
        .fold(0, |acc, flag| acc | flag)
}

/// Creates a new file. On Windows the attributes are set at creation, so a
/// read-only file is still writable through the returned handle; elsewhere
/// they are ignored (SPEC-12 §4.3).
fn create_file(path: &Path, attributes: u32) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(windows)]
    if attributes != 0 {
        use std::os::windows::fs::OpenOptionsExt;
        options.attributes(attributes);
    }
    #[cfg(not(windows))]
    let _ = attributes;
    options.open(path)
}

/// Attributes kept by [`copy_tree`]: read-only, hidden, system.
#[cfg(windows)]
fn copied_attributes(meta: &fs::Metadata) -> u32 {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes() & 0x7
}

#[cfg(not(windows))]
fn copied_attributes(_: &fs::Metadata) -> u32 {
    0
}

/// Copies folders and files with their contents and (on Windows) attributes;
/// modification times are not copied.
pub(crate) fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    let entries = fs::read_dir(from).map_err(|e| format!("cannot read {}: {e}", from.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("cannot read {}: {e}", from.display()))?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        let meta = fs::symlink_metadata(&src)
            .map_err(|e| format!("cannot stat {}: {e}", src.display()))?;
        if meta.is_dir() {
            fs::create_dir(&dst).map_err(|e| format!("cannot create {}: {e}", dst.display()))?;
            copy_tree(&src, &dst)?;
        } else if meta.is_file() {
            File::open(&src)
                .and_then(|mut input| {
                    let mut out = create_file(&dst, copied_attributes(&meta))?;
                    io::copy(&mut input, &mut out).map(drop)
                })
                .map_err(|e| format!("cannot copy {}: {e}", src.display()))?;
        } else {
            return Err(format!("unsupported file type: {}", src.display()));
        }
    }
    Ok(())
}

/// Sets the modification times given by `mtime`.
fn apply_mtimes(env: &Environment, root: &Path, items: &[Item]) -> Result<(), String> {
    for item in items {
        if let Item::File(file) = item {
            if let Some(t) = file.mtime {
                set_modified(&resolve(env, root, &file.path)?, t.into())?;
            }
        }
    }
    Ok(())
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

pub(crate) fn create_parent(path: &Path) -> Result<(), String> {
    match path.parent() {
        Some(parent) => fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display())),
        None => Ok(()),
    }
}

/// Sets the modification time of a file or folder.
pub(crate) fn set_modified(path: &Path, t: SystemTime) -> Result<(), String> {
    open_for_times(path)
        .and_then(|file| file.set_modified(t))
        .map_err(|e| format!("cannot set mtime of {}: {e}", path.display()))
}

#[cfg(windows)]
fn open_for_times(path: &Path) -> io::Result<File> {
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
fn open_for_times(path: &Path) -> io::Result<File> {
    OpenOptions::new().read(true).open(path)
}

#[cfg(test)]
#[path = "materialize_tests.rs"]
mod tests;
