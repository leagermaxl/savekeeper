//! Both sides of every marker threshold (SPEC-03 §4.4, §6).

use sk_core::env::{CloudProvider, CloudRoot};
use sk_core::model::Marker;

use super::tests::{env, markers, p, run, run_env, s, tree};
use crate::{FsScanner, MemFs};

const LOCAL: &str = "Users/user/AppData/Local";
const LOCALLOW: &str = "Users/user/AppData/LocalLow";
const SQLITE: &[u8] = b"SQLite format 3\x00 and the rest of it";

fn has(fs: &dyn FsScanner, rel: &str, m: Marker) -> bool {
    markers(fs, rel).contains(&m)
}

/// `n` files `d/<prefix><i>.<ext>` of `size` bytes each.
fn many(files: &mut Vec<(String, u64)>, prefix: &str, ext: &str, n: u64, size: u64) {
    for i in 0..n {
        files.push((format!("d/{prefix}{i}.{ext}"), size));
    }
}

fn fs_of(files: &[(String, u64)]) -> MemFs {
    let refs: Vec<(&str, u64)> = files.iter().map(|(r, n)| (r.as_str(), *n)).collect();
    tree(&refs)
}

#[test]
fn empty_folder_has_no_share_markers() {
    let mut fs = MemFs::new();
    fs.add_dir(&s("d/logs"));
    assert_eq!(markers(&fs, "d"), Vec::<Marker>::new());
    // Zero bytes: byte shares are false, file shares still count.
    let fs = tree(&[
        ("z/a.json", 0),
        ("z/b.json", 0),
        ("z/c.json", 0),
        ("z/d.mp4", 0),
        ("z/tmp", 0),
    ]);
    assert_eq!(markers(&fs, "z"), vec![Marker::ConfigLike]);
}

#[test]
fn has_executables() {
    let m = Marker::HasExecutables;
    let bytes = |exec: u64| {
        tree(&[
            ("d/a.dll", exec - 21),
            ("d/b.sys", 10),
            ("d/c.msi", 5),
            ("d/sub/e.ocx", 6),
            ("d/data.bin", 100 - exec),
        ])
    };
    assert!(has(&bytes(31), "d", m));
    assert!(!has(&bytes(30), "d", m));
    // Three `.exe` in `dir` or its direct subfolders, whatever their size.
    let exes = |deep: &str| {
        tree(&[
            ("d/a.exe", 1),
            ("d/sub/b.exe", 1),
            (deep, 1),
            ("d/x.bin", 1000),
        ])
    };
    assert!(has(&exes("d/sub2/c.EXE"), "d", m));
    assert!(!has(&exes("d/sub/deeper/c.exe"), "d", m));
    // No bytes at all: the byte share is false.
    assert!(!has(&tree(&[("z/a.exe", 0)]), "z", m));
}

#[test]
fn git_repo() {
    let mut fs = MemFs::new();
    fs.add_dir(&s("dir/.git"))
        .add_file(&s("file/.GIT"), 30, "-1d", None);
    fs.add_dir(&s("deep/sub/.git")).add_dir(&s("hub/.github"));
    assert!(has(&fs, "dir", Marker::GitRepo));
    assert!(has(&fs, "file", Marker::GitRepo));
    assert!(!has(&fs, "deep", Marker::GitRepo));
    assert!(!has(&fs, "hub", Marker::GitRepo));
}

#[test]
fn unity_game() {
    let m = Marker::UnityGame;
    let product = format!("{LOCALLOW}/Company/Product");
    let fs = tree(&[
        (&format!("{product}/Player.log"), 1),
        (&format!("{product}/Extra/output_log.txt"), 1),
        (&format!("{LOCALLOW}/Company/Player-prev.log"), 1),
        (&format!("{LOCALLOW}/Other/Game/Unity/x"), 1),
        (&format!("{LOCALLOW}/Other/Plain/save.dat"), 1),
        (&format!("{LOCALLOW}/Other/Nested/sub/Player.log"), 1),
        ("game/bin/Game_Data/level0", 1),
        ("game/bin/UnityPlayer.dll", 1),
        ("split/a/Game_Data/level0", 1),
        ("split/b/UnityPlayer.dll", 1),
        ("file/x_Data", 1),
        ("file/UnityPlayer.dll", 1),
    ]);
    assert!(has(&fs, &product, m));
    assert!(has(&fs, &format!("{LOCALLOW}/Other/Game"), m));
    // Not `Company\Product`: one or three components below `{LOCALLOW}`.
    assert!(!has(&fs, &format!("{LOCALLOW}/Company"), m));
    assert!(!has(&fs, &format!("{product}/Extra"), m));
    assert!(!has(&fs, &format!("{LOCALLOW}/Other/Plain"), m));
    assert!(!has(&fs, &format!("{LOCALLOW}/Other/Nested"), m));
    // `*_Data` folder next to `UnityPlayer.dll`, at any level.
    assert!(has(&fs, "game", m));
    assert!(!has(&fs, "split", m));
    assert!(!has(&fs, "file", m));
}

#[test]
fn unreal_save_games() {
    let m = Marker::UnrealSaveGames;
    let fs = tree(&[
        ("u3/Game/Saved/SaveGames/slot1.sav", 1),
        ("u4/a/Game/Saved/SaveGames/slot1.sav", 1),
        ("other/Game/Stuff/SaveGames/slot1.sav", 1),
        ("cfg3/a/b/Saved/Config/WindowsNoEditor/Game.ini", 1),
        ("cfg3/a/b/Saved/x/y/slot.sav", 1),
        ("cfg4/a/b/c/Saved/Config/Windows/Game.ini", 1),
        ("cfg4/a/b/c/Saved/slot.sav", 1),
        ("nosav/Saved/Config/Windows/Game.ini", 1),
        ("nosav/Saved/slot.dat", 1),
        ("outside/Saved/Config/Windows/Game.ini", 1),
        ("outside/slot.sav", 1),
    ]);
    assert!(has(&fs, "u3", m));
    assert!(!has(&fs, "u4", m));
    assert!(!has(&fs, "other", m));
    assert!(has(&fs, "cfg3", m));
    assert!(!has(&fs, "cfg4", m));
    assert!(!has(&fs, "nosav", m));
    assert!(!has(&fs, "outside", m));
}

#[test]
fn unreal_save_games_with_dir_as_the_saved_folder() {
    let m = Marker::UnrealSaveGames;
    // `dir` itself is the folder of depth 0: a `Saved` root counts, an
    // `Other` root with the same children does not.
    let mut files = Vec::new();
    for root in ["g1/Saved", "g1/Other"] {
        files.push((format!("{root}/SaveGames/slot.sav"), 1));
    }
    for root in ["g2/SAVED", "g2/Other"] {
        files.push((format!("{root}/Config/WindowsNoEditor/Game.ini"), 1));
        files.push((format!("{root}/a/b/c/d/slot.sav"), 1));
    }
    files.push(("g3/Saved/x/SaveGames/slot.sav".to_owned(), 1));
    files.push(("g4/Saved/Config/Windows/Game.ini".to_owned(), 1));
    files.push(("g4/Saved/slot.dat".to_owned(), 1));
    let fs = fs_of(&files);
    assert!(has(&fs, "g1/Saved", m));
    assert!(!has(&fs, "g1/Other", m));
    assert!(has(&fs, "g2/SAVED", m));
    assert!(!has(&fs, "g2/Other", m));
    // `SaveGames` whose parent is not `Saved`; `Config\Windows*` without a `.sav`.
    assert!(!has(&fs, "g3/Saved", m));
    assert!(!has(&fs, "g4/Saved", m));
}

#[test]
fn electron_app() {
    let m = Marker::ElectronApp;
    let mut fs = MemFs::new();
    for d in ["Local Storage", "IndexedDB", "gpucache"] {
        fs.add_dir(&s(&format!("three/{d}")));
    }
    for d in ["Local Storage", "IndexedDB"] {
        fs.add_dir(&s(&format!("two/{d}")));
        fs.add_dir(&s(&format!("deep/x/{d}")));
    }
    fs.add_dir(&s("deep/x/Cache"));
    fs.add_dir(&s("files/Code Cache"))
        .add_file(&s("files/blob_storage"), 1, "-1d", None)
        .add_file(&s("files/Service Worker"), 1, "-1d", None);
    assert!(has(&fs, "three", m));
    assert!(!has(&fs, "two", m));
    assert!(!has(&fs, "deep", m));
    assert!(has(&fs, "files", m));
}

#[test]
fn chromium_profile() {
    let m = Marker::ChromiumProfile;
    let fs = tree(&[
        ("a/Local State", 1),
        ("a/Default/Preferences", 1),
        ("b/Local State", 1),
        ("c/Preferences", 1),
        ("c/Bookmarks", 1),
        ("d/Preferences", 1),
        ("d/History", 1),
        ("e/Preferences", 1),
        ("f/Default/Preferences", 1),
        ("g/Local State", 1),
        ("g/Other/Preferences", 1),
    ]);
    assert!(has(&fs, "a", m));
    assert!(!has(&fs, "b", m));
    assert!(has(&fs, "c", m));
    assert!(has(&fs, "d", m));
    assert!(!has(&fs, "e", m));
    assert!(!has(&fs, "f", m));
    assert!(!has(&fs, "g", m));
}

#[test]
fn sqlite_files() {
    let m = Marker::SqliteFiles;
    let mut fs = MemFs::new();
    fs.add_file(&s("yes/x.DB3"), 64, "-1d", Some(SQLITE))
        .add_file(&s("wrong/x.sqlite"), 64, "-1d", Some(&[7; 64]))
        .add_file(&s("ext/x.bin"), 64, "-1d", Some(SQLITE))
        .add_file(&s("small/x.db"), 15, "-1d", Some(SQLITE))
        .add_file(&s("cloud/x.sqlite3"), 64, "-1d", Some(SQLITE))
        .set_cloud_only(&s("cloud/x.sqlite3"));
    assert!(has(&fs, "yes", m));
    assert!(!has(&fs, "wrong", m));
    assert!(!has(&fs, "ext", m));
    let before = fs.calls().read_head;
    assert!(!has(&fs, "ext", m));
    // Too small and cloud-only files are not candidates: nothing is read.
    assert!(!has(&fs, "small", m));
    assert!(!has(&fs, "cloud", m));
    assert_eq!(fs.calls().read_head, before);
}

#[test]
fn sqlite_candidates_are_the_five_shallowest() {
    let m = Marker::SqliteFiles;
    let mut fs = MemFs::new();
    // Order: depth, then lowercase path. The first one is locked: a failed try.
    fs.add_file(&s("q/a.db"), 64, "-1d", Some(SQLITE))
        .set_locked(&s("q/a.db"));
    for name in ["b/x.db", "B/y.db", "c/x.db", "d/x.db"] {
        fs.add_file(&s(&format!("q/{name}")), 64, "-1d", None);
    }
    // The sixth candidate is a database, but only five are read.
    fs.add_file(&s("q/a/b/real.db"), 64, "-1d", Some(SQLITE));
    let before = fs.calls().read_head;
    assert!(!has(&fs, "q", m));
    assert_eq!(fs.calls().read_head - before, 5);

    // A shallow database is found with one read.
    fs.add_file(&s("q/z.sqlite"), 64, "-1d", Some(SQLITE));
    let before = fs.calls().read_head;
    assert!(has(&fs, "q", m));
    assert_eq!(fs.calls().read_head - before, 2);
}

#[test]
fn cache_like() {
    let m = Marker::CacheLike;
    let mut fs = MemFs::new();
    for name in ["Cache", "GPUCache", "Crashpad", "LOGS", "tmp"] {
        fs.add_dir(&s(name));
    }
    fs.add_dir(&s("Cachex")).add_dir(&s("crashed"));
    for name in ["Cache", "GPUCache", "Crashpad", "LOGS", "tmp"] {
        assert!(has(&fs, name, m), "{name}");
    }
    assert!(!has(&fs, "Cachex", m));
    assert!(!has(&fs, "crashed", m));
    // Bytes in direct children with cache names (a folder counts its subtree).
    let split = |cache: u64| {
        tree(&[
            ("app/ShaderCache/a/b.bin", cache),
            ("app/temp", 0),
            ("app/x.bin", 100 - cache),
        ])
    };
    assert!(has(&split(50), "app", m));
    assert!(!has(&split(49), "app", m));
    let file = tree(&[
        ("f/logs", 50),
        ("f/x.bin", 50),
        ("g/sub/logs/a", 50),
        ("g/x.bin", 50),
    ]);
    assert!(has(&file, "f", m));
    assert!(!has(&file, "g", m));
    // `tmp+log+dmp+etl` files: 61 of 100 against 60 of 100.
    let exts = |n: u64| {
        let mut files = Vec::new();
        many(&mut files, "t", "tmp", n - 3, 1);
        many(&mut files, "l", "LOG", 1, 1);
        many(&mut files, "m", "dmp", 1, 1);
        many(&mut files, "e", "etl", 1, 1);
        many(&mut files, "x", "bin", 100 - n, 1000);
        fs_of(&files)
    };
    assert!(has(&exts(61), "d", m));
    assert!(!has(&exts(60), "d", m));
}

#[test]
fn config_like() {
    let m = Marker::ConfigLike;
    let share = |n: u64| {
        let exts = [
            "json", "ini", "xml", "cfg", "conf", "toml", "yaml", "yml", "reg", "config", "prefs",
            "plist",
        ];
        let mut files: Vec<(String, u64)> = (0..n)
            .map(|k| (format!("d/c{k}.{}", exts[k as usize % exts.len()]), 1))
            .collect();
        many(&mut files, "x", "bin", 100 - n, 1);
        fs_of(&files)
    };
    assert!(has(&share(60), "d", m));
    assert!(!has(&share(59), "d", m));
    // At most 50 MiB.
    let size = |bytes: u64| tree(&[("d/a.json", bytes - 1), ("d/b.json", 1)]);
    assert!(has(&size(52_428_800), "d", m));
    assert!(!has(&size(52_428_801), "d", m));
}

#[test]
fn media_heavy() {
    let m = Marker::MediaHeavy;
    let fs = |media: u64| {
        tree(&[
            ("d/a.JPG", media - 30),
            ("d/b.mkv", 10),
            ("d/c.opus", 10),
            ("d/e.dng", 10),
            ("d/x.bin", 100 - media),
        ])
    };
    assert!(has(&fs(71), "d", m));
    assert!(!has(&fs(70), "d", m));
}

#[test]
fn document_heavy() {
    let m = Marker::DocumentHeavy;
    let share = |n: u64| {
        let mut files = Vec::new();
        many(&mut files, "doc", "PDF", n - 2, 1);
        many(&mut files, "md", "md", 1, 1);
        many(&mut files, "dj", "djvu", 1, 1);
        many(&mut files, "x", "bin", 100 - n, 1000);
        fs_of(&files)
    };
    assert!(has(&share(51), "d", m));
    assert!(!has(&share(50), "d", m));
}

#[test]
fn project_like() {
    let m = Marker::ProjectLike;
    let mut fs = tree(&[
        ("d3/a/b/Cargo.toml", 1),
        ("d4/a/b/c/Cargo.toml", 1),
        ("g/build.gradle.kts", 1),
        ("s/x/App.SLN", 1),
        ("mk/MAKEFILE", 1),
        ("blend/art/scene.blend", 1),
        ("u/a/b/ProjectSettings/ProjectVersion.txt", 1),
        ("u/a/b/Assets/Scenes/main.unity", 1),
        ("u4/a/b/c/ProjectSettings/x.txt", 1),
        ("u4/a/b/c/Assets/x.txt", 1),
        ("half/ProjectSettings/x.txt", 1),
        ("scene/Assets/main.unity", 1),
    ]);
    fs.add_dir(&s("folder/package.json"));
    for rel in ["d3", "g", "s", "mk", "blend", "u"] {
        assert!(has(&fs, rel, m), "{rel}");
    }
    for rel in ["d4", "u4", "half", "scene", "folder"] {
        assert!(!has(&fs, rel, m), "{rel}");
    }
}

#[test]
fn cloud_synced() {
    let m = Marker::CloudSynced;
    let fs = tree(&[
        ("Users/user/OneDrive/Docs/a.txt", 1),
        ("Users/user/OneDriveX/a.txt", 1),
    ]);
    let mut env = env();
    env.cloud_roots.push(CloudRoot {
        provider: CloudProvider::Dropbox,
        path: p("Users/user/OneDrive"),
    });
    let on = |rel: &str| run_env(&fs, rel, &env).markers.contains(&m);
    assert!(on("Users/user/OneDrive"));
    assert!(on("Users/user/OneDrive/Docs"));
    assert!(!on("Users/user/OneDriveX"));
    assert!(!run(&fs, "Users/user/OneDrive").markers.contains(&m));
}

#[test]
fn uwp_package() {
    let m = Marker::UwpPackage;
    let pkg = format!("{LOCAL}/Packages/Microsoft.Foo_8wekyb3d8bbwe");
    let bad = format!("{LOCAL}/Packages/Microsoft.Foo_8wekyb3d8bbw");
    let fs = tree(&[
        (&format!("{pkg}/LocalState/a.dat"), 1),
        (&format!("{bad}/LocalState/a.dat"), 1),
    ]);
    assert!(has(&fs, &pkg, m));
    assert!(!has(&fs, &format!("{pkg}/LocalState"), m));
    assert!(!has(&fs, &bad, m));
    assert!(!has(&fs, &format!("{LOCAL}/Packages"), m));
}
