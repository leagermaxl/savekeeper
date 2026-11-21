//! Windows tests of source reading (SPEC-10 T-10-05, §6): files held by other
//! handles, cloud-only files, links, long paths. Temp dirs under
//! `<workspace>/target/tmp`, no admin rights.

#![allow(clippy::unwrap_used)]

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::windows::fs::OpenOptionsExt;
use std::process::Command;
use std::time::Instant;

use sk_core::path::to_extended;

use super::tests::temp_dir;
use super::*;

/// `FILE_ATTRIBUTE_OFFLINE`.
const FILE_ATTRIBUTE_OFFLINE: u32 = 0x1000;

/// A short policy so that the tests do not wait long.
fn quick() -> RetryPolicy {
    RetryPolicy {
        attempts: 3,
        delays_ms: vec![10, 20, 30],
    }
}

/// Opens `path` for reading with no sharing, as a program holding a database does.
fn hold_exclusively(path: &Path) -> File {
    OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .unwrap()
}

#[test]
fn file_held_without_sharing_is_locked_after_retries() {
    let dir = temp_dir();
    let path = dir.path().join("History");
    fs::write(&path, b"sqlite").unwrap();
    let _holder = hold_exclusively(&path);

    let started = Instant::now();
    let result = open_source(&path, &quick(), &CancellationToken::new());
    let Err(OpenError::Locked(e)) = result else {
        panic!("expected Locked, got {result:?}");
    };
    assert_eq!(e.raw_os_error(), Some(ERROR_SHARING_VIOLATION));
    assert!(
        started.elapsed() >= Duration::from_millis(60),
        "three waits"
    );
}

#[test]
fn file_released_during_retries_is_read() {
    let dir = temp_dir();
    let path = dir.path().join("config.vdf");
    fs::write(&path, b"vdf").unwrap();
    let holder = hold_exclusively(&path);
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        drop(holder);
    });

    let mut source =
        open_source(&path, &RetryPolicy::default(), &CancellationToken::new()).unwrap();
    let mut out = Vec::new();
    source.read_to_end(&mut out).unwrap();
    assert_eq!(out, b"vdf");
    release.join().unwrap();
}

#[test]
fn file_held_for_writing_by_a_game_is_read() {
    let dir = temp_dir();
    let path = dir.path().join("ER0000.sl2");
    fs::write(&path, b"save").unwrap();
    // A game keeps its save open for writing and shares read and write.
    let _game = OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(0x1 | 0x2)
        .open(&path)
        .unwrap();

    let mut source = open_source(&path, &quick(), &CancellationToken::new()).unwrap();
    let mut out = Vec::new();
    source.read_to_end(&mut out).unwrap();
    assert_eq!(out, b"save");
}

#[test]
fn reading_does_not_block_writers_or_deleters() {
    let dir = temp_dir();
    let path = dir.path().join("log.txt");
    fs::write(&path, b"line\n").unwrap();
    let source = open_source(&path, &quick(), &CancellationToken::new()).unwrap();

    // Another program can still open the file for writing and rename it.
    let mut writer = OpenOptions::new().append(true).open(&path).unwrap();
    writer.write_all(b"next\n").unwrap();
    drop(writer);
    let renamed = dir.path().join("log.old");
    fs::rename(&path, &renamed).unwrap();
    drop(source);
    assert_eq!(fs::read(&renamed).unwrap(), b"line\nnext\n");
}

#[test]
fn byte_range_lock_is_a_lock_error_on_read() {
    let dir = temp_dir();
    let path = dir.path().join("data.ost");
    fs::write(&path, b"outlook data").unwrap();
    let holder = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    holder.lock().unwrap();

    let mut source = open_source(&path, &quick(), &CancellationToken::new()).unwrap();
    let e = source.read_to_end(&mut Vec::new()).unwrap_err();
    assert!(is_lock_error(&e), "{e:?}");
    assert_eq!(e.raw_os_error(), Some(ERROR_LOCK_VIOLATION));
    holder.unlock().unwrap();
}

/// An OFFLINE file is cloud-only and is not opened: it is held without
/// sharing, so an open would be a sharing violation.
#[test]
fn cloud_only_file_is_not_opened() {
    let dir = temp_dir();
    let path = dir.path().join("photo.jpg");
    fs::write(&path, b"jpeg").unwrap();
    sk_core::fs::set_attributes(&path, FILE_ATTRIBUTE_OFFLINE).unwrap();
    let _holder = hold_exclusively(&path);

    let result = open_source(&path, &quick(), &CancellationToken::new());
    assert!(matches!(result, Err(OpenError::CloudOnly)), "{result:?}");
}

#[test]
fn attributes_and_times_are_kept_and_not_changed() {
    let dir = temp_dir();
    let path = dir.path().join("hidden.ini");
    fs::write(&path, b"[a]").unwrap();
    // HIDDEN | ARCHIVE
    sk_core::fs::set_attributes(&path, 0x2 | 0x20).unwrap();
    let before = fs::metadata(&path).unwrap();

    let mut source = open_source(&path, &quick(), &CancellationToken::new()).unwrap();
    assert_eq!(
        source.meta().attributes(),
        [FileAttribute::Hidden, FileAttribute::Archive]
    );
    assert!(source.meta().ctime.is_some());
    source.read_to_end(&mut Vec::new()).unwrap();
    drop(source);

    let after = fs::metadata(&path).unwrap();
    use std::os::windows::fs::MetadataExt;
    assert_eq!(after.file_attributes(), before.file_attributes());
    assert_eq!(after.last_write_time(), before.last_write_time());
    assert_eq!(after.creation_time(), before.creation_time());
}

#[test]
fn path_of_400_characters_is_read() {
    let dir = temp_dir();
    let mut path = dir.path().to_path_buf();
    while path.as_os_str().len() < 380 {
        path.push("a-rather-long-folder-name");
    }
    fs::create_dir_all(to_extended(&path)).unwrap();
    let file = path.join("long-save-file-name.sav");
    assert!(file.as_os_str().len() >= 400, "{}", file.as_os_str().len());
    fs::write(to_extended(&file), b"long").unwrap();

    let mut source = open_source(&file, &quick(), &CancellationToken::new()).unwrap();
    let mut out = Vec::new();
    source.read_to_end(&mut out).unwrap();
    assert_eq!(out, b"long");
}

#[test]
fn junction_is_not_followed() {
    let dir = temp_dir();
    let target = dir.path().join("target");
    fs::create_dir(&target).unwrap();
    let link = dir.path().join("link");
    let out = Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(&link)
        .arg(&target)
        .output()
        .unwrap();
    assert!(out.status.success(), "mklink /J failed: {out:?}");

    let result = open_source(&link, &quick(), &CancellationToken::new());
    assert!(matches!(result, Err(OpenError::NotAFile)), "{result:?}");
}
