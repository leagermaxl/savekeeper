//! Tests of `sk-scan::win` (T-03-05). Everything happens in temp dirs.

use std::fs;
use std::path::Path;

use sk_core::fs::{CloudState, FsError, Readability, ReparseKind};

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
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::process::Command;

    use sk_core::path::to_extended;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{SetFileAttributesW, FILE_FLAGS_AND_ATTRIBUTES};

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
        let name: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: `name` is NUL-terminated and outlives the call.
        unsafe { SetFileAttributesW(PCWSTR(name.as_ptr()), FILE_FLAGS_AND_ATTRIBUTES(attrs)) }
            .unwrap();
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
