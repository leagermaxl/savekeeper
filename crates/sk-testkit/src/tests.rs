use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use sk_core::config::Config;
use sk_core::env::KnownFolder;
use sk_core::events::{Event, ScanPhase};
use sk_core::fs::{
    DirEntryInfo, EntryMeta, FsError, FsScanner, Readability, WalkControl, WalkOptions, WalkStats,
};
use sk_core::CancellationToken;
use time::macros::datetime;
use time::OffsetDateTime;

use super::*;

const PROFILE: &str = r#"
known_folders:
  DOCUMENTS: "OneDrive/Documents"
tree:
  - path: "{APPDATA}/EldenRing/76561198000000000/ER0000.sl2"
    size: 200 KiB
    mtime: "-1d"
  - path: "{LOCALLOW}/Team Cherry/Hollow Knight/user1.dat"
    size: 12 KiB
  - path: "{LOCALAPPDATA}/discord/Cache/Cache_Data/f_000001"
    size: 1 KiB
    repeat: 5
  - path: "{DOCUMENTS}/notes.txt"
    size: 10
"#;

fn mtime(path: &Path) -> SystemTime {
    fs::metadata(path).unwrap().modified().unwrap()
}

fn secs_between(a: SystemTime, b: SystemTime) -> u64 {
    match a.duration_since(b) {
        Ok(d) => d.as_secs(),
        Err(e) => e.duration().as_secs(),
    }
}

#[test]
fn materializes_tree() {
    let profile = FakeProfile::from_yaml(PROFILE).unwrap();
    let root = profile.root.path();

    let save = profile.path("{APPDATA}/EldenRing/76561198000000000/ER0000.sl2");
    assert_eq!(fs::metadata(&save).unwrap().len(), 200 * 1024);
    let day_ago = SystemTime::now() - std::time::Duration::from_secs(86_400);
    assert!(secs_between(mtime(&save), day_ago) < 60);

    let hashes = tree_hash(root);
    let names: Vec<_> = hashes.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        [
            "OneDrive/Documents/notes.txt",
            "Users/user/AppData/Local/discord/Cache/Cache_Data/f_000001",
            "Users/user/AppData/Local/discord/Cache/Cache_Data/f_000002",
            "Users/user/AppData/Local/discord/Cache/Cache_Data/f_000003",
            "Users/user/AppData/Local/discord/Cache/Cache_Data/f_000004",
            "Users/user/AppData/Local/discord/Cache/Cache_Data/f_000005",
            "Users/user/AppData/LocalLow/Team Cherry/Hollow Knight/user1.dat",
            "Users/user/AppData/Roaming/EldenRing/76561198000000000/ER0000.sl2",
        ]
    );
    // Content is seeded by the path, so repeated files differ.
    let cache = "Users/user/AppData/Local/discord/Cache/Cache_Data/f_00000";
    assert_ne!(hashes[&format!("{cache}1")], hashes[&format!("{cache}2")]);
}

#[test]
fn known_folders_exist_and_follow_overrides() {
    let profile = FakeProfile::from_yaml(PROFILE).unwrap();
    let root = profile.root.path();
    assert_eq!(
        profile.env.known_folder(KnownFolder::Documents),
        Some(root.join("OneDrive").join("Documents").as_path())
    );
    assert_eq!(
        profile.env.known_folder(KnownFolder::AppData),
        Some(root.join("Users/user/AppData/Roaming").as_path())
    );
    for folder in KnownFolder::ALL {
        assert!(
            profile.env.known_folder(folder).unwrap().is_dir(),
            "{folder:?}"
        );
    }
}

#[test]
fn load_is_deterministic() {
    let a = FakeProfile::from_yaml(PROFILE).unwrap();
    let b = FakeProfile::from_yaml(PROFILE).unwrap();
    assert_ne!(a.root.path(), b.root.path());
    assert_eq!(tree_hash(a.root.path()), tree_hash(b.root.path()));
}

#[test]
fn empty_description_gives_known_folders_only() {
    let profile = FakeProfile::from_yaml("{}").unwrap();
    assert!(tree_hash(profile.root.path()).is_empty());
    assert!(profile.path("{HOME}").is_dir());
}

#[test]
fn invalid_descriptions_are_errors() {
    for src in [
        "known_folders:\n  NOPE: x\n",
        "known_folders:\n  DOCUMENTS: \"../outside\"\n",
        "known_folders:\n  DOCUMENTS: \"/abs\"\n",
        "tree:\n  - path: \"{NOPE}/a\"\n",
        "tree:\n  - path: \"{STEAM}/a\"\n",
        "tree:\n  - path: \"{HOME}/a\"\n    size: 1 KB\n",
        "tree:\n  - path: \"{HOME}/a\"\n    sample: \"no/such.vdf\"\n",
        "launchers:\n  steam: { root: \"../Steam\" }\n",
        "launchers:\n  steam: { root: \"Steam\", games: [] }\n",
        "tree:\n  - path: \"{HOME}/r/.git\"\n    git: { commits: 1, unpushed: 1 }\n",
    ] {
        assert!(FakeProfile::from_yaml(src).is_err(), "{src}");
    }
}

#[test]
fn path_write_and_set_mtime() {
    let profile = FakeProfile::from_yaml("{}").unwrap();
    let path = profile.path(r"{APPDATA}\Foo\settings.json");
    assert_eq!(
        path,
        profile
            .root
            .path()
            .join("Users/user/AppData/Roaming/Foo/settings.json")
    );
    assert_eq!(profile.path("{APPDATA}/Foo/settings.json"), path);

    profile.write(r"{APPDATA}\Foo\settings.json", "{}");
    assert_eq!(fs::read_to_string(&path).unwrap(), "{}");

    let t = datetime!(2020-01-02 03:04:05 UTC);
    profile.set_mtime(r"{APPDATA}\Foo\settings.json", t);
    assert_eq!(OffsetDateTime::from(mtime(&path)), t);
    profile.set_mtime(r"{APPDATA}\Foo", t);
    assert_eq!(OffsetDateTime::from(mtime(path.parent().unwrap())), t);
}

#[test]
#[should_panic(expected = "outside the profile root")]
fn path_outside_root_panics() {
    let profile = FakeProfile::from_yaml("{}").unwrap();
    profile.path(r"C:\Windows");
}

#[test]
#[should_panic(expected = "cannot read fixture")]
fn load_of_missing_profile_panics() {
    FakeProfile::load("no-such-profile");
}

/// A file system with nothing in it.
struct EmptyFs;

impl FsScanner for EmptyFs {
    fn metadata(&self, _: &Path) -> Result<EntryMeta, FsError> {
        Err(FsError::NotFound)
    }
    fn exists(&self, _: &Path) -> bool {
        false
    }
    fn read_dir(&self, _: &Path) -> Result<Vec<DirEntryInfo>, FsError> {
        Err(FsError::NotFound)
    }
    fn walk(
        &self,
        _: &Path,
        _: &WalkOptions,
        _: &mut dyn FnMut(&DirEntryInfo) -> WalkControl,
        _: &CancellationToken,
    ) -> Result<WalkStats, FsError> {
        Err(FsError::NotFound)
    }
    fn read_head(&self, _: &Path, _: usize) -> Result<Vec<u8>, FsError> {
        Err(FsError::NotFound)
    }
    fn read_small(&self, _: &Path, _: usize) -> Result<Vec<u8>, FsError> {
        Err(FsError::NotFound)
    }
    fn probe_readable(&self, _: &Path) -> Readability {
        Readability::Missing
    }
}

#[test]
fn collect_ctx_and_drain_events() {
    let profile = FakeProfile::from_yaml(PROFILE).unwrap();
    let (ctx, mut rx) = collect_ctx(&profile, Config::default(), Arc::new(EmptyFs));
    assert_eq!(*ctx.env, profile.env);
    assert_eq!(*ctx.config, Config::default());
    assert!(!ctx.cancel.is_cancelled());
    assert!(drain_events(&mut rx).is_empty());

    let started = Event::PhaseStarted {
        phase: ScanPhase::Collect,
    };
    let found = Event::FindingsAdded { count: 2 };
    ctx.events.send(started.clone()).unwrap();
    ctx.events.send(found.clone()).unwrap();
    assert_eq!(drain_events(&mut rx), [started, found.clone()]);
    assert!(drain_events(&mut rx).is_empty());

    // Events sent before the context is dropped are still drained.
    ctx.events.send(found.clone()).unwrap();
    drop(ctx);
    assert_eq!(drain_events(&mut rx), [found]);
}

#[test]
fn tree_hash_detects_changes() {
    let dir = tempfile::TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("a/b")).unwrap();
    fs::create_dir_all(dir.path().join("empty")).unwrap();
    fs::write(dir.path().join("a/b/x.txt"), "hello").unwrap();
    fs::write(dir.path().join("y.bin"), [0u8, 1, 2]).unwrap();

    let before = tree_hash(dir.path());
    assert_eq!(
        before.keys().map(String::as_str).collect::<Vec<_>>(),
        ["a/b/x.txt", "y.bin"]
    );
    assert_eq!(
        before["a/b/x.txt"],
        blake3::hash(b"hello").to_hex().to_string()
    );

    fs::write(dir.path().join("a/b/x.txt"), "hello!").unwrap();
    assert_ne!(tree_hash(dir.path()), before);
}
