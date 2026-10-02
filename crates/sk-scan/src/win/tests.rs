//! Tests of `sk-scan::win` (T-03-04, T-03-05). Everything happens in temp dirs.

use std::fs;

use sk_core::fs::{CloudState, EntryKind, FsError, Readability, ReparseKind};

use super::*;

#[test]
fn cloud_state_by_attributes() {
    assert_eq!(cloud_state(0), CloudState::Local);
    // ARCHIVE | PINNED: the content is local.
    assert_eq!(cloud_state(0x20 | 0x8_0000), CloudState::Local);
    for attr in [
        FILE_ATTRIBUTE_OFFLINE,
        FILE_ATTRIBUTE_RECALL_ON_OPEN,
        FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS,
    ] {
        assert_eq!(cloud_state(attr | 0x20), CloudState::CloudOnly, "{attr:#x}");
    }
}

#[test]
fn reparse_kind_by_tag() {
    assert_eq!(reparse_kind(0xA000_000C), ReparseKind::Symlink);
    assert_eq!(reparse_kind(0xA000_0003), ReparseKind::Junction);
    assert_eq!(reparse_kind(0x8000_001B), ReparseKind::AppExecLink);
    for tag in [0x9000_001A, 0x9000_101A, 0x9000_601A, 0x9000_F01A] {
        assert_eq!(reparse_kind(tag), ReparseKind::CloudPlaceholder, "{tag:#x}");
    }
    // IO_REPARSE_TAG_WOF, IO_REPARSE_TAG_DEDUP.
    assert_eq!(reparse_kind(0x8000_0017), ReparseKind::Other(0x8000_0017));
    assert_eq!(reparse_kind(0x8000_0013), ReparseKind::Other(0x8000_0013));
}

#[test]
fn raw_meta_kind_and_walking() {
    let raw = |is_dir, is_symlink, tag| RawMeta {
        attrs: 0,
        is_dir,
        is_symlink,
        tag,
        size: 0,
        mtime: None,
        ctime: None,
    };
    let cases = [
        (raw(false, false, None), EntryKind::File, false),
        (raw(true, false, None), EntryKind::Dir, true),
        (
            raw(true, false, Some(IO_REPARSE_TAG_MOUNT_POINT)),
            EntryKind::Reparse(ReparseKind::Junction),
            false,
        ),
        (
            raw(true, false, Some(IO_REPARSE_TAG_CLOUD)),
            EntryKind::Reparse(ReparseKind::CloudPlaceholder),
            true,
        ),
        (
            raw(false, false, Some(IO_REPARSE_TAG_CLOUD)),
            EntryKind::Reparse(ReparseKind::CloudPlaceholder),
            false,
        ),
        (
            raw(true, false, Some(0)),
            EntryKind::Reparse(ReparseKind::Other(0)),
            false,
        ),
        (
            raw(false, true, None),
            EntryKind::Reparse(ReparseKind::Symlink),
            false,
        ),
    ];
    for (meta, kind, walked) in cases {
        assert_eq!(meta.kind(), kind, "{meta:?}");
        assert_eq!(meta.is_walked(), walked, "{meta:?}");
        assert_eq!(meta.entry_meta().kind, kind);
    }
    let offline = RawMeta {
        attrs: FILE_ATTRIBUTE_OFFLINE,
        size: 7,
        ..raw(false, false, None)
    };
    assert_eq!(offline.entry_meta().cloud, CloudState::CloudOnly);
    assert_eq!(offline.entry_meta().size, 7);
}

/// Cloud placeholders are `Reparse(CloudPlaceholder)` for files and folders;
/// the OS attributes pass through unchanged, so `FILE_ATTRIBUTE_DIRECTORY`
/// tells them apart and only the folder is walked (SPEC-03 §4.2).
#[test]
fn cloud_placeholders_keep_the_directory_attribute() {
    let placeholder = |attrs: u32| RawMeta {
        attrs,
        // As `listing_meta`/`find_meta` derive it on Windows.
        is_dir: attrs & FILE_ATTRIBUTE_DIRECTORY != 0,
        is_symlink: false,
        tag: Some(IO_REPARSE_TAG_CLOUD | 0x3000),
        size: 42,
        mtime: None,
        ctime: None,
    };
    let folder = placeholder(FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT);
    let file = placeholder(FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS);
    let kind = EntryKind::Reparse(ReparseKind::CloudPlaceholder);

    let meta = folder.entry_meta();
    assert_eq!(meta.kind, kind);
    assert_ne!(meta.attrs & FILE_ATTRIBUTE_DIRECTORY, 0);
    assert_eq!(meta.cloud, CloudState::Local);
    assert!(folder.is_walked());

    let meta = file.entry_meta();
    assert_eq!(meta.kind, kind);
    assert_eq!(meta.attrs & FILE_ATTRIBUTE_DIRECTORY, 0);
    assert_eq!(meta.cloud, CloudState::CloudOnly);
    assert_eq!(meta.size, 42);
    assert!(!file.is_walked());
}

#[test]
fn readability_by_error_code() {
    assert_eq!(readability_from_error(32), Readability::Locked);
    assert_eq!(readability_from_error(33), Readability::Locked);
    for code in [2, 3, 15, 21, 53, 67, 123, 161] {
        assert_eq!(readability_from_error(code), Readability::Missing, "{code}");
    }
    assert_eq!(readability_from_error(5), Readability::Denied);
    assert_eq!(readability_from_error(1920), Readability::Denied);
}

#[test]
fn probe_plain_file_dir_and_missing() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("save.dat");
    fs::write(&file, b"data").unwrap();

    assert_eq!(probe_readable(&file), Readability::Ok);
    assert_eq!(probe_readable(dir.path()), Readability::Ok);
    assert_eq!(
        probe_readable(&dir.path().join("missing.dat")),
        Readability::Missing
    );
    assert_eq!(
        probe_readable(&dir.path().join("no").join("such.dat")),
        Readability::Missing
    );
    // The probe reads nothing and changes nothing.
    assert_eq!(fs::read(&file).unwrap(), b"data");
}

#[test]
fn plain_entries_have_no_reparse_tag() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.txt");
    fs::write(&file, b"a").unwrap();

    assert_eq!(reparse_tag(&file).unwrap(), None);
    assert_eq!(reparse_tag(dir.path()).unwrap(), None);
    assert!(matches!(
        reparse_tag(&dir.path().join("missing")),
        Err(FsError::NotFound)
    ));
}

#[cfg(windows)]
mod windows_only {
    use std::fs::{File, OpenOptions};
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::path::Path;
    use std::process::Command;

    use sk_core::path::to_extended;
    use windows::Win32::Foundation::FILETIME;

    use super::super::ffi::{filetime, has_wildcard};
    use super::*;

    /// Opens `path` for reading with no sharing, as a program holding a database does.
    fn lock_exclusively(path: &Path) -> File {
        OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(path)
            .unwrap()
    }

    fn set_attributes(path: &Path, attrs: u32) {
        super::super::set_attributes(path, attrs).unwrap();
    }

    #[test]
    fn filetime_converts_to_utc() {
        let ft = |ticks: u64| FILETIME {
            dwLowDateTime: ticks as u32,
            dwHighDateTime: (ticks >> 32) as u32,
        };
        assert_eq!(filetime(ft(0)), None);
        assert_eq!(
            filetime(ft(116_444_736_000_000_000)),
            Some(time::OffsetDateTime::UNIX_EPOCH)
        );
        // 2020-01-02T03:04:05Z.
        assert_eq!(
            filetime(ft(116_444_736_000_000_000 + 1_577_934_245 * 10_000_000))
                .map(|t| t.unix_timestamp()),
            Some(1_577_934_245)
        );
    }

    /// `find_meta` (enumeration data) agrees with `listing_meta` (a listing).
    #[test]
    fn find_meta_matches_listing_meta() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("save.dat"), b"12345").unwrap();
        fs::create_dir(dir.path().join("Saves")).unwrap();
        for entry in fs::read_dir(dir.path()).unwrap() {
            let entry = entry.unwrap();
            let listed = listing_meta(&entry.metadata().unwrap(), &entry.path());
            let found = find_meta(&entry.path()).unwrap();
            assert_eq!(listed.attrs, found.attrs);
            assert_eq!(listed.is_dir, found.is_dir);
            assert_eq!(listed.size, found.size);
            assert_eq!(listed.tag, None);
            assert_eq!(
                listed.mtime.map(|t| t.unix_timestamp()),
                found.mtime.map(|t| t.unix_timestamp())
            );
            assert!(found.mtime.is_some() && found.ctime.is_some());
        }
        let file = find_meta(&dir.path().join("save.dat")).unwrap();
        assert_eq!((file.size, file.kind()), (5, EntryKind::File));
        assert_eq!(
            find_meta(&dir.path().join("Saves")).unwrap().kind(),
            EntryKind::Dir
        );
        assert!(matches!(
            find_meta(&dir.path().join("missing")),
            Err(FsError::NotFound)
        ));
    }

    /// `FindFirstFileExW` reads the last component as a pattern: a wildcard
    /// path must not describe some other entry (`gam*.sav` → `game.sav`).
    #[test]
    fn wildcard_paths_are_not_found() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("game.sav"), b"x").unwrap();
        fs::create_dir(dir.path().join("Saves")).unwrap();
        fs::write(dir.path().join("Saves").join("slot1.sav"), b"x").unwrap();
        for pattern in [
            "gam*.sav",
            "game.sa?",
            "game<sav",
            "gam>.sav",
            "game\"sav",
            "*",
        ] {
            let path = dir.path().join(pattern);
            assert!(has_wildcard(&path), "{pattern}");
            assert!(
                matches!(find_meta(&path), Err(FsError::NotFound)),
                "{pattern}"
            );
            assert!(
                matches!(reparse_tag(&path), Err(FsError::NotFound)),
                "{pattern}"
            );
            assert!(
                matches!(find_meta(&to_extended(&path)), Err(FsError::NotFound)),
                "{pattern}"
            );
            assert_eq!(probe_readable(&path), Readability::Missing, "{pattern}");
        }
        // A wildcard in a middle component.
        let middle = dir.path().join("Sav*").join("slot1.sav");
        assert!(matches!(find_meta(&middle), Err(FsError::NotFound)));
        assert!(matches!(reparse_tag(&middle), Err(FsError::NotFound)));
        // The `?` of the `\\?\` prefix is not a wildcard.
        let real = to_extended(&dir.path().join("game.sav"));
        assert!(!has_wildcard(&real));
        assert_eq!(find_meta(&real).unwrap().size, 1);
        assert!(!has_wildcard(Path::new(r"\\?\UNC\server\share\dir")));
    }

    /// A drive root cannot be named by enumeration; it is still described.
    #[test]
    fn find_meta_of_a_drive_root() {
        let dir = tempfile::tempdir().unwrap();
        let root: std::path::PathBuf = dir.path().components().take(2).collect();
        let meta = find_meta(&root).unwrap();
        assert!(meta.is_dir, "{root:?}");
        assert_eq!(meta.kind(), EntryKind::Dir);
    }

    #[test]
    fn file_opened_without_sharing_is_locked() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("profile.db");
        fs::write(&file, b"SQLite format 3\0").unwrap();

        let holder = lock_exclusively(&file);
        assert_eq!(probe_readable(&file), Readability::Locked);
        drop(holder);
        assert_eq!(probe_readable(&file), Readability::Ok);
    }

    #[test]
    fn file_opened_with_full_sharing_is_readable() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("app.log");
        fs::write(&file, b"log").unwrap();

        // FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE.
        let _writer = OpenOptions::new()
            .write(true)
            .share_mode(0x7)
            .open(&file)
            .unwrap();
        assert_eq!(probe_readable(&file), Readability::Ok);
    }

    /// An OFFLINE file is reported as cloud-only without being opened: it is
    /// held exclusively, so an open would have reported `Locked`.
    #[test]
    fn offline_file_is_cloud_only_and_not_opened() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("photo.jpg");
        fs::write(&file, b"jpeg").unwrap();
        set_attributes(&file, FILE_ATTRIBUTE_OFFLINE);
        assert_ne!(
            fs::metadata(&file).unwrap().file_attributes() & FILE_ATTRIBUTE_OFFLINE,
            0
        );

        assert_eq!(probe_readable(&file), Readability::CloudOnly);
        let holder = lock_exclusively(&file);
        assert_eq!(probe_readable(&file), Readability::CloudOnly);
        drop(holder);

        // The probe changed nothing.
        assert_ne!(
            fs::metadata(&file).unwrap().file_attributes() & FILE_ATTRIBUTE_OFFLINE,
            0
        );
        assert_eq!(fs::read(&file).unwrap(), b"jpeg");
    }

    #[test]
    fn directory_probe_uses_backup_semantics() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("Saves");
        fs::create_dir(&sub).unwrap();
        assert_eq!(probe_readable(&sub), Readability::Ok);
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

    #[test]
    fn junction_has_mount_point_tag() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("inside.txt"), b"x").unwrap();
        let link = dir.path().join("link");

        make_junction(&link, &target);

        let tag = reparse_tag(&link).unwrap();
        assert_eq!(tag, Some(IO_REPARSE_TAG_MOUNT_POINT));
        assert_eq!(tag.map(reparse_kind), Some(ReparseKind::Junction));
        assert_eq!(reparse_tag(&target).unwrap(), None);
        assert_eq!(reparse_tag(&target.join("inside.txt")).unwrap(), None);
        let meta = find_meta(&link).unwrap();
        assert_eq!(meta.tag, Some(IO_REPARSE_TAG_MOUNT_POINT));
        assert_eq!(meta.kind(), EntryKind::Reparse(ReparseKind::Junction));
        assert!(meta.is_dir && !meta.is_walked());

        // Removing the junction does not touch its target.
        fs::remove_dir(&link).unwrap();
        assert!(target.join("inside.txt").exists());
    }

    /// The link itself is probed, not its target (FR-03-02, SPEC-03 §6).
    #[test]
    fn junction_to_removed_dir_is_readable() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("gone");
        fs::create_dir(&target).unwrap();
        let link = dir.path().join("link");
        make_junction(&link, &target);
        assert_eq!(probe_readable(&link), Readability::Ok);

        fs::remove_dir(&target).unwrap();
        assert!(!target.exists());
        assert_eq!(probe_readable(&link), Readability::Ok);
        assert_eq!(
            reparse_tag(&link).unwrap(),
            Some(IO_REPARSE_TAG_MOUNT_POINT)
        );
        // The probe did not recreate the target.
        assert!(!target.exists());
    }

    #[test]
    fn long_paths_are_supported() {
        let dir = tempfile::tempdir().unwrap();
        let mut deep = dir.path().to_path_buf();
        while deep.as_os_str().len() < 400 {
            deep.push("a_rather_long_directory_name");
        }
        fs::create_dir_all(to_extended(&deep)).unwrap();
        let file = deep.join("save.sav");
        fs::write(to_extended(&file), b"save").unwrap();
        assert!(file.as_os_str().len() > 400);

        assert_eq!(probe_readable(&file), Readability::Ok);
        assert_eq!(probe_readable(&deep), Readability::Ok);
        assert_eq!(reparse_tag(&file).unwrap(), None);
    }
}

#[cfg(unix)]
mod unix_only {
    use std::os::unix::fs::symlink;

    use super::*;

    /// Symlinks are not followed: the link itself is readable, even dangling.
    #[test]
    fn symlinks_are_probed_as_links() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.dat");
        fs::write(&target, b"x").unwrap();
        let link = dir.path().join("link.dat");
        symlink(&target, &link).unwrap();
        let dangling = dir.path().join("dangling.dat");
        symlink(dir.path().join("missing.dat"), &dangling).unwrap();

        assert_eq!(probe_readable(&link), Readability::Ok);
        assert_eq!(probe_readable(&dangling), Readability::Ok);
        assert_eq!(reparse_tag(&dangling).unwrap(), None);
    }
}
