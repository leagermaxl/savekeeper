use std::fs;
use std::path::Path;
use std::time::SystemTime;

use super::*;
use crate::{tree_hash, FakeProfile};

fn secs_from_now(path: &Path) -> u64 {
    let modified = fs::metadata(path).unwrap().modified().unwrap();
    match SystemTime::now().duration_since(modified) {
        Ok(d) => d.as_secs(),
        Err(e) => e.duration().as_secs(),
    }
}

fn read(path: impl AsRef<Path>) -> String {
    fs::read_to_string(path).unwrap()
}

#[test]
fn launchers_fill_the_environment() {
    let profile = FakeProfile::from_yaml(
        r#"
launchers:
  steam: { root: "Program Files (x86)/Steam", users: ["12345678"] }
  epic: { users: ["abc"] }
tree:
  - path: "{STEAM}/config/loginusers.vdf"
    size: 3
"#,
    )
    .unwrap();
    let root = profile.root.path();
    let steam_root = root.join("Program Files (x86)").join("Steam");
    let launchers = &profile.env.launchers;
    assert_eq!(launchers.len(), 2);
    let epic = &launchers[0];
    assert_eq!((epic.id.as_str(), &epic.root), ("epic", &None));
    assert_eq!(epic.user_ids[0].id, "abc");
    assert_eq!(epic.user_ids[0].alt_id, None);
    let steam = &launchers[1];
    assert_eq!(steam.id, "steam");
    assert_eq!(steam.root.as_deref(), Some(steam_root.as_path()));
    assert_eq!(steam.user_ids[0].id, "12345678");
    assert_eq!(
        steam.user_ids[0].alt_id.as_deref(),
        Some("76561197972611406")
    );
    assert!(steam.games.is_empty());
    assert!(steam_root.join("config/loginusers.vdf").is_file());
    assert_eq!(
        profile.path("{STEAM}/config"),
        steam_root.join("config").as_path()
    );
}

#[test]
fn samples_are_copied() {
    let profile = FakeProfile::from_yaml(
        r#"
tree:
  - path: "{HOME}/a/loginusers.vdf"
    sample: "steam/loginusers.vdf"
  - path: "{HOME}/b/copy_1.vdf"
    sample: 'steam\loginusers.vdf'
    repeat: 2
"#,
    )
    .unwrap();
    let sample = fs::read(fixtures_dir().join("samples/steam/loginusers.vdf")).unwrap();
    for template in [
        "{HOME}/a/loginusers.vdf",
        "{HOME}/b/copy_1.vdf",
        "{HOME}/b/copy_2.vdf",
    ] {
        assert_eq!(fs::read(profile.path(template)).unwrap(), sample);
    }
}

#[test]
fn git_repos_have_commits_remote_and_dirty_state() {
    let profile = FakeProfile::from_yaml(
        r#"
tree:
  - path: "{HOME}/p/plain/.git"
    git: { commits: 2 }
  - path: "{HOME}/p/pushed/.git"
    git: { commits: 3, dirty: true, remote: "https://example.com/pushed.git", unpushed: 1 }
  - path: "{HOME}/p/local/.git"
    git: { commits: 2, remote: "https://example.com/local.git", unpushed: 2 }
  - path: "{HOME}/p/empty/.git"
    git: { commits: 0, dirty: true }
"#,
    )
    .unwrap();
    let repo = |name: &str| profile.path(&format!("{{HOME}}/p/{name}"));

    let plain = repo("plain");
    assert_eq!(read(plain.join(".git/HEAD")), "ref: refs/heads/main\n");
    assert_eq!(
        read(plain.join("README.md")),
        "# plain\n\nchange 1\nchange 2\n"
    );
    assert!(plain.join(".git/index").is_file());
    let head = read(plain.join(".git/refs/heads/main"));
    let head = head.trim();
    assert_eq!(head.len(), 40);
    let object = plain.join(".git/objects").join(&head[..2]).join(&head[2..]);
    assert!(object.is_file(), "{}", object.display());
    assert!(!read(plain.join(".git/config")).contains("[remote"));

    let pushed = repo("pushed");
    assert_eq!(
        read(pushed.join("README.md")),
        "# pushed\n\nchange 1\nchange 2\nchange 3\nuncommitted change\n"
    );
    let config = read(pushed.join(".git/config"));
    assert!(config.contains("[remote \"origin\"]\n\turl = https://example.com/pushed.git\n"));
    assert!(config.contains("[branch \"main\"]\n\tremote = origin\n"));
    let local = read(pushed.join(".git/refs/heads/main"));
    let remote = read(pushed.join(".git/refs/remotes/origin/main"));
    assert_ne!(local, remote);

    let local = repo("local");
    let config = read(local.join(".git/config"));
    assert!(config.contains("[remote \"origin\"]"));
    assert!(!config.contains("[branch"));
    assert!(!local.join(".git/refs/remotes/origin/main").exists());

    let empty = repo("empty");
    assert!(!empty.join(".git/refs/heads/main").exists());
    assert!(!empty.join(".git/index").exists());
    assert_eq!(read(empty.join("README.md")), "uncommitted change\n");
}

#[test]
fn git_repos_are_deterministic() {
    let src = "tree:\n  - path: \"{HOME}/r/.git\"\n    git: { commits: 3, dirty: true, remote: u, unpushed: 1 }\n";
    let a = FakeProfile::from_yaml(src).unwrap();
    let b = FakeProfile::from_yaml(src).unwrap();
    let hashes = tree_hash(a.root.path());
    assert!(hashes.keys().any(|k| k.ends_with(".git/index")));
    assert_eq!(hashes, tree_hash(b.root.path()));
}

#[test]
fn attributes_are_set_on_windows_only() {
    let profile = FakeProfile::from_yaml(
        r#"
tree:
  - path: "{DESKTOP}/desktop.ini"
    size: 10
    attrs: [hidden, system, readonly]
    mtime: "-1d"
"#,
    )
    .unwrap();
    let path = profile.path("{DESKTOP}/desktop.ini");
    let meta = fs::metadata(&path).unwrap();
    assert_eq!(meta.len(), 10);
    assert!(secs_from_now(&path).abs_diff(86_400) < 60);
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        assert_eq!(meta.file_attributes() & 0x7, 0x7);
    }
    #[cfg(not(windows))]
    assert!(!meta.permissions().readonly());
}

#[test]
fn copy_tree_keeps_contents_and_attributes_but_not_times() {
    let src = FakeProfile::from_yaml(
        r#"
tree:
  - path: "{HOME}/a/b/old.txt"
    size: 1 KiB
    mtime: "-10d"
    attrs: [readonly]
  - path: "{HOME}/c/.git"
    git: { commits: 1 }
"#,
    )
    .unwrap();
    let dest = tempfile::TempDir::new().unwrap();
    copy_tree(src.root.path(), dest.path()).unwrap();
    assert_eq!(tree_hash(src.root.path()), tree_hash(dest.path()));
    let copy = dest.path().join("Users/user/a/b/old.txt");
    assert!(secs_from_now(&copy) < 60);
    // Empty known folders are copied too.
    assert!(dest.path().join("Users/user/Videos").is_dir());
    #[cfg(windows)]
    assert!(fs::metadata(&copy).unwrap().permissions().readonly());
}

#[test]
fn materialize_profile_needs_an_empty_destination() {
    let dest = tempfile::TempDir::new().unwrap();
    fs::write(dest.path().join("x"), "x").unwrap();
    let err = materialize_profile("empty", dest.path()).unwrap_err();
    assert!(err.contains("is not empty"), "{err}");
    let err = materialize_profile("no-such-profile", dest.path()).unwrap_err();
    assert!(err.contains("cannot read fixture"), "{err}");
}

#[test]
fn materialize_profile_creates_the_destination() {
    let dir = tempfile::TempDir::new().unwrap();
    let dest = dir.path().join("out").join("empty");
    let env = materialize_profile("empty", &dest).unwrap();
    assert_eq!(env, Environment::fake(&dest));
    assert!(dest.join("Users/user/AppData/Roaming").is_dir());
    assert!(tree_hash(&dest).is_empty());
}
