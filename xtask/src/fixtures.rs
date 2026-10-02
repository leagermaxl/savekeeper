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
fn prepare_out(out: &Path) -> anyhow::Result<()> {
    let marker = out.join(MARKER);
    if out.exists() && !marker.exists() {
        let not_empty = fs::read_dir(out)
            .with_context(|| format!("cannot read {}", out.display()))?
            .next()
            .is_some();
        if not_empty {
            bail!(
                "{} is not empty and was not created by `cargo xtask fixtures`; choose another --out",
                out.display()
            );
        }
    }
    fs::create_dir_all(out).with_context(|| format!("cannot create {}", out.display()))?;
    fs::write(
        &marker,
        "Created by `cargo xtask fixtures` (SPEC-12 §4.3).\n",
    )
    .with_context(|| format!("cannot write {}", marker.display()))
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
}
