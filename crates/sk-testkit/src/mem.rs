//! `mem_fixture`: a `fixtures/fs/*.yaml` tree loaded into [`MemFs`] without
//! touching the disk (SPEC-12 §4.2, §4.3 «Фикстуры `fixtures/fs`»).

use std::fs;
use std::path::Path;

use sk_core::env::Environment;
use sk_scan::MemFs;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::fixture::{Content, Item, ProfileSpec};
use crate::fixture_fs::{Format, FsFlags};
use crate::materialize::{environment, fixtures_dir, resolve};

/// Loads `fixtures/fs/<name>.yaml` into memory, without writing to disk.
///
/// Templates resolve through `Environment::fake(root)` with the description's
/// `known_folders` and `launchers`, as in
/// [`materialize_profile`](crate::materialize_profile). As there, all known
/// folders and launcher roots exist. A file given only by `size` reads as
/// zeros, `content` is its UTF-8 bytes, and a file without `mtime` gets the
/// time of loading. `attrs` are not represented in `MemFs` and are ignored,
/// as on non-Windows systems. `name` may contain `/` (`heuristics/foo`).
///
/// Panics with the error text, like [`FakeProfile::load`](crate::FakeProfile::load).
pub fn mem_fixture(name: &str, root: &Path) -> (MemFs, Environment) {
    let file = fixtures_dir().join("fs").join(format!("{name}.yaml"));
    let loaded = fs::read_to_string(&file)
        .map_err(|e| format!("cannot read fixture {}: {e}", file.display()))
        .and_then(|src| {
            mem_from_yaml(&src, root).map_err(|e| format!("fixture {}: {e}", file.display()))
        });
    loaded.unwrap_or_else(|e| panic!("{e}"))
}

/// Builds a `MemFs` and its environment from a `fixtures/fs` description.
pub(crate) fn mem_from_yaml(src: &str, root: &Path) -> Result<(MemFs, Environment), String> {
    let spec = ProfileSpec::parse(src)?;
    let now = OffsetDateTime::now_utc();
    let items = spec.items(now, Format::Fs)?;
    let env = environment(&spec, root)?;
    let mut mem = MemFs::new();
    let launcher_roots = env.launchers.iter().filter_map(|l| l.root.as_ref());
    for dir in env.known_folders.values().chain(launcher_roots) {
        mem.add_dir(utf8(dir)?);
    }
    for item in &items {
        match item {
            Item::File(file) => {
                let path = resolve(&env, root, &file.path)?;
                let path = utf8(&path)?;
                let mtime = file
                    .mtime
                    .unwrap_or(now)
                    .format(&Rfc3339)
                    .map_err(|e| format!("{:?}: cannot format mtime: {e}", file.path))?;
                match &file.content {
                    Content::Random(size) => mem.add_file(path, *size, &mtime, None),
                    Content::Text(text) => {
                        let bytes = text.as_bytes();
                        mem.add_file(path, bytes.len() as u64, &mtime, Some(bytes))
                    }
                    Content::Sample(_) => {
                        return Err(format!("{:?}: `sample` in fixtures/fs", file.path))
                    }
                };
                apply_flags(&mut mem, path, file.fs);
            }
            Item::Dir(dir) => {
                let path = resolve(&env, root, &dir.path)?;
                let path = utf8(&path)?;
                mem.add_dir(path);
                apply_flags(&mut mem, path, dir.fs);
            }
            Item::Repo(repo) => return Err(format!("{:?}: `git` in fixtures/fs", repo.path)),
        }
    }
    Ok((mem, env))
}

fn apply_flags(mem: &mut MemFs, path: &str, flags: FsFlags) {
    if let Some(kind) = flags.reparse {
        mem.add_reparse(path, kind);
    }
    if flags.locked {
        mem.set_locked(path);
    }
    if flags.cloud_only {
        mem.set_cloud_only(path);
    }
}

fn utf8(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| format!("path is not valid UTF-8: {}", path.display()))
}

#[cfg(test)]
#[path = "mem_tests.rs"]
mod tests;
