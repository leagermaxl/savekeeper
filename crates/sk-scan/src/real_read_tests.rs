//! Tests of `RealFs::read_head` / `read_small` (T-03-06, SPEC-03 §4.1).
//! Everything happens in temp dirs, without admin rights.

use std::fs;
use std::path::Path;

use sk_core::env::Environment;

use super::*;
use crate::win::{check_readable, read_limited, RawMeta};
use crate::{EntryKind, ReparseKind};

#[cfg(windows)]
#[path = "real_read_win_tests.rs"]
mod windows_only;

fn real() -> RealFs {
    RealFs::new(&Environment::fake(Path::new("/nonexistent")))
}

fn is_io(r: Result<Vec<u8>, FsError>) -> bool {
    matches!(r, Err(FsError::Io(_)))
}

#[test]
fn reads_head_and_small_files() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("libraryfolders.vdf");
    fs::write(&file, b"\"libraryfolders\"").unwrap();
    let empty = dir.path().join("empty.cfg");
    fs::write(&empty, b"").unwrap();
    let fs_ = real();

    assert_eq!(fs_.read_head(&file, 3).unwrap(), b"\"li");
    assert_eq!(fs_.read_head(&file, 0).unwrap(), b"");
    assert_eq!(fs_.read_head(&file, 1000).unwrap(), b"\"libraryfolders\"");
    assert_eq!(fs_.read_small(&file, 16).unwrap(), b"\"libraryfolders\"");
    assert_eq!(
        fs_.read_small(&file, usize::MAX).unwrap(),
        b"\"libraryfolders\""
    );
    assert!(matches!(fs_.read_small(&file, 15), Err(FsError::TooLarge)));
    assert_eq!(fs_.read_small(&empty, 0).unwrap(), b"");
    assert_eq!(fs_.read_head(&empty, 10).unwrap(), b"");
}

#[test]
fn folders_and_missing_paths_are_not_read() {
    let dir = tempfile::tempdir().unwrap();
    let fs_ = real();
    assert!(is_io(fs_.read_head(dir.path(), 4)));
    assert!(is_io(fs_.read_small(dir.path(), 4)));
    for missing in [
        dir.path().join("missing.dat"),
        dir.path().join("no").join("such.dat"),
    ] {
        assert!(matches!(fs_.read_head(&missing, 4), Err(FsError::NotFound)));
        assert!(matches!(
            fs_.read_small(&missing, 4),
            Err(FsError::NotFound)
        ));
    }
}

/// Reading changes neither the content nor the modification time (P1).
#[test]
fn reads_do_not_change_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("config.json");
    fs::write(&file, b"{\"a\":1}").unwrap();
    let before = fs::metadata(&file).unwrap().modified().unwrap();
    real().read_head(&file, 2).unwrap();
    real().read_small(&file, 100).unwrap();
    assert_eq!(fs::metadata(&file).unwrap().modified().unwrap(), before);
    assert_eq!(fs::read(&file).unwrap(), b"{\"a\":1}");
}

/// A file that grew after its size was taken is `TooLarge` by what was read,
/// and no more than `max + 1` bytes are read (an endless reader ends).
#[test]
fn read_limit_counts_bytes_read() {
    assert!(matches!(
        read_limited(&b"abcdef"[..], 3, 4, true),
        Err(FsError::TooLarge)
    ));
    assert_eq!(read_limited(&b"abcdef"[..], 3, 4, false).unwrap(), b"abcd");
    assert_eq!(read_limited(&b"abcd"[..], 3, 4, true).unwrap(), b"abcd");
    assert!(matches!(
        read_limited(std::io::repeat(1), 0, 5, true),
        Err(FsError::TooLarge)
    ));
    assert_eq!(
        read_limited(std::io::repeat(1), u64::MAX, 5, false).unwrap(),
        [1; 5]
    );
}

#[test]
fn checks_before_opening() {
    let raw = |attrs, is_dir, tag| RawMeta {
        attrs,
        is_dir,
        is_symlink: false,
        tag,
        size: 0,
        mtime: None,
        ctime: None,
    };
    let cloud_tag = Some(crate::win::IO_REPARSE_TAG_CLOUD);
    assert!(check_readable(&raw(0x20, false, None)).is_ok());
    // A hydrated cloud placeholder file is read.
    assert!(check_readable(&raw(0x420, false, cloud_tag)).is_ok());
    // Cloud-only wins over everything else.
    for attrs in [0x1000, 0x4_0000, 0x40_0000] {
        assert!(matches!(
            check_readable(&raw(attrs | 0x410, true, cloud_tag)),
            Err(FsError::CloudOnly)
        ));
    }
    let io = |r: Result<(), FsError>| matches!(r, Err(FsError::Io(_)));
    assert!(io(check_readable(&raw(0x10, true, None))));
    assert!(io(check_readable(&raw(0x410, true, cloud_tag))));
    for tag in [0xA000_000C, 0xA000_0003, 0x8000_001B, 0x1234] {
        assert!(
            io(check_readable(&raw(0x400, false, Some(tag)))),
            "{tag:#x}"
        );
    }
    let symlink = RawMeta {
        is_symlink: true,
        ..raw(0, false, None)
    };
    assert_eq!(symlink.kind(), EntryKind::Reparse(ReparseKind::Symlink));
    assert!(io(check_readable(&symlink)));
}

#[cfg(unix)]
#[test]
fn symlinks_are_not_followed() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target.cfg");
    fs::write(&target, b"secret").unwrap();
    let link = dir.path().join("link.cfg");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    assert!(is_io(real().read_head(&link, 4)));
    assert!(is_io(real().read_small(&link, 100)));
    assert_eq!(real().read_small(&target, 100).unwrap(), b"secret");
}
