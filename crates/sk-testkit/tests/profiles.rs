//! The fixture profiles of `fixtures/profiles` (SPEC-12 §4.3, §6).

// Helpers outside `#[test]` functions are not covered by `allow-unwrap-in-tests`.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use sk_core::env::KnownFolder;
use sk_testkit::{tree_hash, FakeProfile};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn secs_from_now(path: &Path) -> u64 {
    let modified = fs::metadata(path).unwrap().modified().unwrap();
    match SystemTime::now().duration_since(modified) {
        Ok(d) => d.as_secs(),
        Err(e) => e.duration().as_secs(),
    }
}

/// SPEC-12 §6: two loads of `gamer` give the same tree.
#[test]
fn gamer_load_is_deterministic() {
    let a = FakeProfile::load("gamer");
    let b = FakeProfile::load("gamer");
    assert_ne!(a.root.path(), b.root.path());
    let hashes = tree_hash(a.root.path());
    assert!(hashes.len() > 500, "{}", hashes.len());
    assert_eq!(hashes, tree_hash(b.root.path()));
    assert_eq!(a.env.launchers[0].user_ids, b.env.launchers[0].user_ids);
}

#[test]
fn gamer_has_steam_games_and_relative_mtimes() {
    let p = FakeProfile::load("gamer");
    let steam = p.env.launchers.iter().find(|l| l.id == "steam").unwrap();
    assert_eq!(
        steam.root.as_deref(),
        Some(p.root.path().join("Program Files (x86)/Steam").as_path())
    );
    assert_eq!(
        steam.user_ids[0].alt_id.as_deref(),
        Some("76561197972611406")
    );
    let manifest = p.path("{STEAM}/steamapps/appmanifest_1245620.acf");
    assert_eq!(
        fs::read(&manifest).unwrap(),
        fs::read(fixtures().join("samples/steam/appmanifest_1245620.acf")).unwrap()
    );
    let save = p.path("{APPDATA}/EldenRing/76561197972611406/ER0000.sl2");
    assert_eq!(fs::metadata(&save).unwrap().len(), 28_311_552);
    // Relative to this load, not to the time the cache was filled.
    assert!(secs_from_now(&save).abs_diff(86_400) < 60);
    // Without `mtime`, files are as new as the load.
    assert!(secs_from_now(&p.path("{APPDATA}/discord/settings.json")) < 60);
    for template in [
        "{LOCALLOW}/Team Cherry/Hollow Knight/user1.dat",
        "{SAVED_GAMES}/CD Projekt Red/Cyberpunk 2077/ManualSave-0/sav.dat",
        "{DOCUMENTS}/My Games/Skyrim Special Edition/SkyrimPrefs.ini",
        "{APPDATA}/discord/Cache/Cache_Data/f_000500",
        "{APPDATA}/obs-studio/basic/scenes/Untitled.json",
    ] {
        assert!(p.path(template).is_file(), "{template}");
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        let ini = fs::metadata(p.path("{DESKTOP}/desktop.ini")).unwrap();
        assert_eq!(ini.file_attributes() & 0x6, 0x6, "hidden and system");
    }
}

/// Read-only and hidden files and git objects do not keep the root alive.
#[test]
fn dropping_a_profile_removes_its_root() {
    for name in ["gamer", "developer"] {
        let p = FakeProfile::load(name);
        let root = p.root.path().to_path_buf();
        assert!(root.is_dir());
        drop(p);
        assert!(!root.exists(), "{name}: {} is left", root.display());
    }
}

#[test]
fn empty_has_only_known_folders() {
    let p = FakeProfile::load("empty");
    assert!(tree_hash(p.root.path()).is_empty());
    for folder in KnownFolder::ALL {
        assert!(p.env.known_folder(folder).unwrap().is_dir(), "{folder:?}");
    }
}

#[test]
fn developer_load_is_deterministic() {
    let a = FakeProfile::load("developer");
    let b = FakeProfile::load("developer");
    let hashes = tree_hash(a.root.path());
    assert!(hashes.contains_key("Users/user/Projects/clean-lib/.git/HEAD"));
    assert_eq!(hashes, tree_hash(b.root.path()));
}

/// `git` from PATH, if installed: the repositories must look right to real git.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .output()
        .ok()?;
    assert!(out.status.success(), "git {args:?}: {out:?}");
    Some(String::from_utf8(out.stdout).unwrap())
}

#[test]
fn developer_repos_have_the_described_state() {
    let p = FakeProfile::load("developer");
    let repo = |name: &str| p.path(&format!("{{HOME}}/Projects/{name}"));
    let Some(_) = git(&repo("clean-lib"), &["--version"]) else {
        eprintln!("git is not installed; skipping the git state check");
        return;
    };
    let ahead = |name: &str| git(&repo(name), &["rev-list", "--count", "origin/main..main"]);
    let status = |name: &str| git(&repo(name), &["status", "--porcelain=v1"]);

    let expected = BTreeMap::from([
        ("clean-lib", ("3", "0", "")),
        (
            "dirty-app",
            ("5", "0", " M README.md\n?? .gitignore\n?? package.json\n"),
        ),
        ("unpushed-tool", ("4", "2", "")),
    ]);
    for (name, (commits, unpushed, porcelain)) in expected {
        let count = git(&repo(name), &["rev-list", "--count", "main"]).unwrap();
        assert_eq!(count.trim(), commits, "{name}");
        assert_eq!(ahead(name).unwrap().trim(), unpushed, "{name}");
        assert_eq!(status(name).unwrap(), porcelain, "{name}");
    }
}

/// FR-12-03: samples stay small.
#[test]
fn samples_are_small() {
    fn visit(dir: &Path, out: &mut Vec<(PathBuf, u64)>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(&path, out);
            } else {
                let len = fs::metadata(&path).unwrap().len();
                out.push((path, len));
            }
        }
    }
    let mut files = Vec::new();
    visit(&fixtures().join("samples"), &mut files);
    assert!(files.len() >= 5);
    for (path, len) in files {
        assert!(len <= 100 * 1024, "{} is {len} bytes", path.display());
    }
}

#[test]
fn ludusavi_sample_has_20_games() {
    let src = fs::read_to_string(fixtures().join("samples/ludusavi/manifest-mini.yaml")).unwrap();
    let games: BTreeMap<String, serde::de::IgnoredAny> = serde_saphyr::from_str(&src).unwrap();
    assert_eq!(games.len(), 20);
    assert!(games.contains_key("ELDEN RING"));
}
