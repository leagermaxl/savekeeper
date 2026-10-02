//! Windows tests of `RealFs` reads (T-03-06, SPEC-03 §4.1, §6): OFFLINE and
//! locked files, links, long paths, wildcards. Temp dirs only, no admin rights.

use std::fs::{File, OpenOptions};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::process::Command;

use sk_core::path::to_extended;

use super::*;
use crate::win::{set_attributes, FILE_ATTRIBUTE_OFFLINE};

/// `ERROR_PRIVILEGE_NOT_HELD`: symbolic links need developer mode or admin.
const ERROR_PRIVILEGE_NOT_HELD: i32 = 1314;

/// Opens `path` for reading with no sharing, as a program holding a database does.
fn lock_exclusively(path: &Path) -> File {
    OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .unwrap()
}

/// Creates the junction `link` -> `target`; `mklink /J` needs no admin rights.
fn make_junction(link: &Path, target: &Path) {
    let out = Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(link)
        .arg(target)
        .output()
        .unwrap();
    assert!(out.status.success(), "mklink /J failed: {out:?}");
}

/// An OFFLINE file is `CloudOnly` and is not opened: it is held without
/// sharing, so an open would be a sharing violation.
#[test]
fn offline_file_is_cloud_only_and_not_opened() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("photo.jpg");
    fs::write(&file, b"jpeg-data").unwrap();
    set_attributes(&file, FILE_ATTRIBUTE_OFFLINE).unwrap();
    let holder = lock_exclusively(&file);

    assert!(matches!(
        real().read_head(&file, 4),
        Err(FsError::CloudOnly)
    ));
    assert!(matches!(
        real().read_small(&file, 100),
        Err(FsError::CloudOnly)
    ));
    // Not `TooLarge` either: cloud-only is checked first.
    assert!(matches!(
        real().read_small(&file, 1),
        Err(FsError::CloudOnly)
    ));
    drop(holder);
    assert_ne!(
        fs::metadata(&file).unwrap().file_attributes() & FILE_ATTRIBUTE_OFFLINE,
        0
    );
    assert_eq!(fs::read(&file).unwrap(), b"jpeg-data");
}

#[test]
fn locked_file_is_a_sharing_violation() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("history.db");
    fs::write(&file, b"SQLite format 3\0").unwrap();
    let holder = lock_exclusively(&file);
    assert!(matches!(
        real().read_head(&file, 16),
        Err(FsError::SharingViolation)
    ));
    assert!(matches!(
        real().read_small(&file, 100),
        Err(FsError::SharingViolation)
    ));
    drop(holder);
    assert_eq!(real().read_head(&file, 16).unwrap(), b"SQLite format 3\0");
}

/// A file opened by another program with full sharing is read.
#[test]
fn file_shared_for_writing_is_read() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("app.log");
    fs::write(&file, b"line").unwrap();
    let _writer = OpenOptions::new()
        .write(true)
        .share_mode(0x7) // READ | WRITE | DELETE
        .open(&file)
        .unwrap();
    assert_eq!(real().read_small(&file, 4).unwrap(), b"line");
}

/// A junction is not followed, and its target is not read.
#[test]
fn junction_is_not_read() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("a.cfg"), b"a").unwrap();
    let link = dir.path().join("link");
    make_junction(&link, &target);

    assert!(is_io(real().read_head(&link, 4)));
    assert!(is_io(real().read_small(&link, 4)));
    // A junction in the middle of the path is resolved by the OS (§4.1).
    assert_eq!(real().read_small(&link.join("a.cfg"), 4).unwrap(), b"a");
}

/// A symbolic link to a file is not followed: its target is held without
/// sharing, so following the link would be a sharing violation. Creating a
/// symbolic link needs developer mode; without it the test checks nothing.
#[test]
fn file_symlink_is_not_followed() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target.cfg");
    fs::write(&target, b"secret").unwrap();
    let link = dir.path().join("link.cfg");
    match std::os::windows::fs::symlink_file(&target, &link) {
        Ok(()) => {}
        Err(e) if e.raw_os_error() == Some(ERROR_PRIVILEGE_NOT_HELD) => {
            eprintln!("skipped: no privilege to create symbolic links");
            return;
        }
        Err(e) => panic!("symlink_file failed: {e}"),
    }
    let holder = lock_exclusively(&target);
    assert!(is_io(real().read_head(&link, 4)));
    assert!(is_io(real().read_small(&link, 100)));
    drop(holder);
    assert_eq!(real().read_small(&target, 100).unwrap(), b"secret");
}

#[test]
fn long_paths_are_read() {
    let dir = tempfile::tempdir().unwrap();
    let mut deep = dir.path().to_path_buf();
    while deep.as_os_str().len() < 400 {
        deep.push("a_rather_long_directory_name");
    }
    fs::create_dir_all(to_extended(&deep)).unwrap();
    let file = deep.join("save.sav");
    fs::write(to_extended(&file), b"save").unwrap();
    assert!(file.as_os_str().len() > 400);

    assert_eq!(real().read_head(&file, 2).unwrap(), b"sa");
    assert_eq!(real().read_small(&file, 4).unwrap(), b"save");
    assert!(matches!(
        real().read_small(&file, 3),
        Err(FsError::TooLarge)
    ));
}

/// A path with wildcards names no entry (as `metadata`): `top.*` must not
/// read `top.sav`.
#[test]
fn wildcard_paths_are_not_found() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("top.sav"), b"top").unwrap();
    for pattern in ["top.*", "top.sa?", "*", "to<", "?/top.sav"] {
        let path = dir.path().join(pattern);
        assert!(
            matches!(real().read_head(&path, 4), Err(FsError::NotFound)),
            "{pattern}"
        );
        assert!(
            matches!(real().read_small(&path, 4), Err(FsError::NotFound)),
            "{pattern}"
        );
    }
}

/// A drive root is a folder and is not read.
#[test]
fn drive_root_is_not_read() {
    let dir = tempfile::tempdir().unwrap();
    let root: PathBuf = dir.path().ancestors().last().unwrap().to_path_buf();
    assert!(is_io(real().read_head(&root, 4)));
}
