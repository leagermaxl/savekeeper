//! `cargo xtask fixtures`: materializes the profiles of `fixtures/profiles`
//! (SPEC-12 §4.3) with the same materializer as `FakeProfile::load`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use clap::Args;

/// Marker file in the output folder: only folders made by this command are overwritten.
const MARKER: &str = ".savekeeper-fixtures";

/// Arguments of `cargo xtask fixtures`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct FixturesArgs {
    /// Profile to materialize (name of `fixtures/profiles/<name>.yaml`); repeatable. Default: all.
    #[arg(long, value_name = "NAME")]
    pub profile: Vec<String>,
    /// Output folder; each profile goes to `<out>/<name>`. Default: `target/fixtures`.
    #[arg(long, value_name = "DIR")]
    pub out: Option<PathBuf>,
}

/// Workspace root: the parent of the `xtask` package.
fn workspace_root() -> PathBuf {
    let xtask = Path::new(env!("CARGO_MANIFEST_DIR"));
    xtask.parent().unwrap_or(xtask).to_path_buf()
}

/// Names of all profiles in `dir`, sorted.
pub fn profile_names(dir: &Path) -> anyhow::Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in fs::read_dir(dir).with_context(|| format!("cannot read {}", dir.display()))? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "yaml") {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                names.push(stem.to_owned());
            }
        }
    }
    names.sort();
    Ok(names)
}

pub fn run(args: &FixturesArgs) -> anyhow::Result<()> {
    let names = if args.profile.is_empty() {
        profile_names(&workspace_root().join("fixtures").join("profiles"))?
    } else {
        args.profile.clone()
    };
    let out = args
        .out
        .clone()
        .unwrap_or_else(|| workspace_root().join("target").join("fixtures"));
    for (name, files) in materialize(&names, &out)? {
        println!("{name}: {files} files -> {}", out.join(&name).display());
    }
    Ok(())
}

/// Materializes `names` into `<out>/<name>`, replacing earlier output.
/// Returns the number of files of each profile.
pub fn materialize(names: &[String], out: &Path) -> anyhow::Result<Vec<(String, usize)>> {
    prepare_out(out)?;
    let mut done = Vec::new();
    for name in names {
        let valid = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !valid {
            bail!("invalid profile name {name:?}");
        }
        let dest = out.join(name);
        if dest.exists() {
            fs::remove_dir_all(&dest)
                .with_context(|| format!("cannot remove {}", dest.display()))?;
        }
        sk_testkit::materialize_profile(name, &dest).map_err(anyhow::Error::msg)?;
        done.push((name.clone(), sk_testkit::tree_hash(&dest).len()));
    }
    Ok(done)
}

/// Creates `out` with the marker, or checks that an existing non-empty `out`
/// was made by this command.
///
/// A folder with only empty subfolders counts as empty: a CI cache of `target/`
/// (`Swatinem/rust-cache`) may restore earlier output as such a skeleton,
/// without the marker.
fn prepare_out(out: &Path) -> anyhow::Result<()> {
    let marker = out.join(MARKER);
    if out.exists() && !marker.exists() && contains_files(out)? {
        bail!(
            "{} is not empty and was not created by `cargo xtask fixtures`; choose another --out",
            out.display()
        );
    }
    fs::create_dir_all(out).with_context(|| format!("cannot create {}", out.display()))?;
    fs::write(
        &marker,
        "Created by `cargo xtask fixtures` (SPEC-12 §4.3).\n",
    )
    .with_context(|| format!("cannot write {}", marker.display()))
}

/// Whether `dir` or any folder below it holds anything but folders
/// (links are not followed and count as files).
fn contains_files(dir: &Path) -> anyhow::Result<bool> {
    for entry in fs::read_dir(dir).with_context(|| format!("cannot read {}", dir.display()))? {
        let entry = entry.with_context(|| format!("cannot read {}", dir.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("cannot stat {}", path.display()))?;
        if !file_type.is_dir() || contains_files(&path)? {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_the_spec_profiles() {
        let names = profile_names(&workspace_root().join("fixtures").join("profiles")).unwrap();
        for name in ["developer", "empty", "gamer"] {
            assert!(names.iter().any(|n| n == name), "{names:?}");
        }
    }

    #[test]
    fn materializes_and_replaces_output() {
        let dir = tempfile::TempDir::new().unwrap();
        let out = dir.path().join("fixtures");
        let names = vec!["empty".to_owned()];
        assert_eq!(
            materialize(&names, &out).unwrap(),
            [("empty".to_owned(), 0)]
        );
        assert!(out.join("empty/Users/user/AppData/Roaming").is_dir());
        // A second run replaces the earlier output.
        fs::write(out.join("empty/stale.txt"), "x").unwrap();
        materialize(&names, &out).unwrap();
        assert!(!out.join("empty/stale.txt").exists());
        assert!(out.join(MARKER).is_file());
    }

    #[test]
    fn refuses_foreign_folders_and_bad_names() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::write(dir.path().join("mine.txt"), "x").unwrap();
        let err = materialize(&["empty".to_owned()], dir.path()).unwrap_err();
        assert!(err.to_string().contains("not created by"), "{err}");
        assert!(dir.path().join("mine.txt").is_file());

        let out = dir.path().join("out");
        for name in ["../x", "", "a/b"] {
            assert!(materialize(&[name.to_owned()], &out).is_err(), "{name}");
        }
        let err = materialize(&["no-such-profile".to_owned()], &out).unwrap_err();
        assert!(err.to_string().contains("cannot read fixture"), "{err}");
    }

    #[test]
    fn accepts_a_skeleton_of_empty_folders() {
        // What a CI cache of `target/` restores: the folders of an earlier
        // output without any files, the marker included.
        let dir = tempfile::TempDir::new().unwrap();
        let out = dir.path().join("fixtures");
        fs::create_dir_all(out.join("empty/Users/user/AppData/Roaming")).unwrap();
        fs::create_dir_all(out.join("gamer/Users/user/Saved Games")).unwrap();
        assert!(!contains_files(&out).unwrap());
        let names = vec!["empty".to_owned()];
        assert_eq!(
            materialize(&names, &out).unwrap(),
            [("empty".to_owned(), 0)]
        );
        assert!(out.join(MARKER).is_file());
        assert!(out.join("empty/Users/user/AppData/Roaming").is_dir());
        assert!(out.join("gamer/Users/user/Saved Games").is_dir());
    }

    #[test]
    fn refuses_a_file_deep_in_empty_folders() {
        let dir = tempfile::TempDir::new().unwrap();
        let out = dir.path().join("fixtures");
        let file = out.join("a/b/c/mine.txt");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::create_dir_all(out.join("d/e")).unwrap();
        fs::write(&file, "x").unwrap();
        assert!(contains_files(&out).unwrap());
        let err = materialize(&["empty".to_owned()], &out).unwrap_err();
        assert!(err.to_string().contains("not created by"), "{err}");
        assert!(file.is_file());
        assert!(!out.join(MARKER).exists());
        assert!(!out.join("empty").exists());
    }
}
