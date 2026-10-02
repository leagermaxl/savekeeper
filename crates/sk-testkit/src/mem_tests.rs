use std::path::PathBuf;

use sk_core::env::KnownFolder;
use sk_scan::{CloudState, EntryKind, FsScanner, Readability, ReparseKind};
use time::Duration;

use super::*;
use crate::FakeProfile;

fn root() -> PathBuf {
    PathBuf::from(if cfg!(windows) { r"C:\fake" } else { "/fake" })
}

fn load(src: &str) -> (MemFs, Environment) {
    mem_from_yaml(src, &root()).unwrap()
}

fn err(src: &str) -> String {
    mem_from_yaml(src, &root()).map(drop).unwrap_err()
}

#[test]
fn files_folders_and_flags_are_loaded() {
    let (mem, env) = load(
        r#"
known_folders:
  DOCUMENTS: "OneDrive/Documents"
tree:
  - path: "{APPDATA}/App/data.bin"
    size: 1 KiB
    mtime: "2020-01-02T03:04:05Z"
  - path: "{APPDATA}/App/config.json"
    content: "{\"a\": 1}"
  - path: "{APPDATA}/App/empty"
    dir: true
  - path: "{APPDATA}/App/LOCK"
    locked: true
  - path: "{DOCUMENTS}/report.docx"
    size: 100
    cloud_only: true
  - path: "{HOME}/My Documents"
    dir: true
    reparse: junction
  - path: "{LOCALAPPDATA}/Microsoft/WindowsApps/winget.exe"
    reparse: app_exec_link
  - path: "{HOME}/odd"
    reparse: 2684354560
  - path: "{LOCALAPPDATA}/c/f_01"
    size: 3
    repeat: 2
"#,
    );
    let documents = env.known_folder(KnownFolder::Documents).unwrap();
    assert_eq!(documents, root().join("OneDrive").join("Documents"));
    let app = env.known_folder(KnownFolder::AppData).unwrap().join("App");
    let home = env.known_folder(KnownFolder::Home).unwrap().to_path_buf();
    let local = env.known_folder(KnownFolder::LocalAppData).unwrap();

    let data = mem.metadata(&app.join("data.bin")).unwrap();
    assert_eq!((data.kind, data.size), (EntryKind::File, 1024));
    assert_eq!(
        data.mtime,
        OffsetDateTime::parse("2020-01-02T03:04:05Z", &Rfc3339).ok()
    );
    assert_eq!(mem.read_head(&app.join("data.bin"), 4).unwrap(), [0; 4]);

    let config = app.join("config.json");
    assert_eq!(mem.metadata(&config).unwrap().size, 8);
    assert_eq!(mem.read_small(&config, 100).unwrap(), br#"{"a": 1}"#);
    let age = OffsetDateTime::now_utc() - mem.metadata(&config).unwrap().mtime.unwrap();
    assert!(age < Duration::MINUTE, "no mtime: the time of loading");

    assert!(mem.read_dir(&app.join("empty")).unwrap().is_empty());
    assert_eq!(mem.probe_readable(&app.join("LOCK")), Readability::Locked);
    let report = mem.metadata(&documents.join("report.docx")).unwrap();
    assert_eq!((report.cloud, report.size), (CloudState::CloudOnly, 100));
    assert_eq!(
        mem.metadata(&home.join("My Documents")).unwrap().kind,
        EntryKind::Reparse(ReparseKind::Junction)
    );
    let winget = local.join("Microsoft/WindowsApps/winget.exe");
    assert_eq!(
        mem.metadata(&winget).unwrap().kind,
        EntryKind::Reparse(ReparseKind::AppExecLink)
    );
    assert_eq!(
        mem.metadata(&home.join("odd")).unwrap().kind,
        EntryKind::Reparse(ReparseKind::Other(0xA000_0000))
    );
    assert!(mem.exists(&local.join("c").join("f_02")));
    // Known folders exist even when empty.
    assert!(mem.exists(env.known_folder(KnownFolder::SavedGames).unwrap()));
}

#[test]
fn fs_fixtures_forbid_git_and_sample() {
    let git = err("tree:\n  - path: \"{HOME}/r/.git\"\n    git: { commits: 1 }\n");
    assert!(git.contains("`git` is not allowed"), "{git}");
    let sample =
        err("tree:\n  - path: \"{HOME}/a.vdf\"\n    sample: \"steam/libraryfolders.vdf\"\n");
    assert!(sample.contains("`sample` is not allowed"), "{sample}");
}

#[test]
fn content_and_dir_exclude_other_content_keys() {
    let both = err("tree:\n  - path: \"{HOME}/a\"\n    content: \"x\"\n    size: 1\n");
    assert!(both.contains("mutually exclusive"), "{both}");
    let dir = err("tree:\n  - path: \"{HOME}/a\"\n    dir: true\n    content: \"x\"\n");
    assert!(dir.contains("`dir` cannot be combined"), "{dir}");
    let dir_mtime = err("tree:\n  - path: \"{HOME}/a\"\n    dir: true\n    mtime: \"-1d\"\n");
    assert!(
        dir_mtime.contains("`dir` cannot be combined"),
        "{dir_mtime}"
    );
    assert!(ProfileSpec::parse("tree:\n  - path: a\n    reparse: hardlink\n").is_err());
}

#[test]
fn profiles_reject_fs_only_keys() {
    for key in ["reparse: junction", "locked: true", "cloud_only: true"] {
        let src = format!("tree:\n  - path: \"{{HOME}}/a\"\n    {key}\n");
        let e = FakeProfile::from_yaml(&src).unwrap_err();
        assert!(e.contains("allowed only in fixtures/fs"), "{key}: {e}");
    }
}

#[test]
fn profiles_accept_content_and_dir() {
    let profile = FakeProfile::from_yaml(
        "tree:\n  - path: \"{APPDATA}/App/a.txt\"\n    content: \"hello\"\n  \
         - path: \"{APPDATA}/App/empty\"\n    dir: true\n",
    )
    .unwrap();
    assert_eq!(
        std::fs::read(profile.path("{APPDATA}/App/a.txt")).unwrap(),
        b"hello"
    );
    assert!(profile.path("{APPDATA}/App/empty").is_dir());
}

#[test]
fn electron_app_fixture_loads() {
    let (mem, env) = mem_fixture("electron-app", &root());
    let app = env
        .known_folder(KnownFolder::AppData)
        .unwrap()
        .join("Obsidian");
    assert!(mem.exists(&app.join("Local Storage")));
}

#[test]
#[should_panic(expected = "cannot read fixture")]
fn missing_fixture_panics() {
    mem_fixture("no-such-fixture", &root());
}
