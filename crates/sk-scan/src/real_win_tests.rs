//! Windows tests of `RealFs::walk` (SPEC-03 §6): junctions, long paths,
//! cloud attributes, locked files. Temp dirs only, no admin rights.

use std::fs::OpenOptions;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::process::Command;

use sk_core::path::to_extended;

use super::*;
use crate::win::{set_attributes, FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_REPARSE_POINT};
use crate::ReparseKind;

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
fn junction_is_reported_but_not_entered() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("profile");
    write(&root, "Documents/report.docx", 4);
    make_junction(&root.join("My Documents"), &root.join("Documents"));

    let mut kinds = Vec::new();
    let (seen, stats) = walk_with(&real(), &root, &opts(), |e| {
        kinds.push((e.rel.clone(), e.meta.kind, e.meta.attrs));
        WalkControl::Continue
    });
    assert_eq!(seen, ["Documents", "Documents/report.docx", "My Documents"]);
    assert_eq!(stats.entries, 3);
    let (_, kind, attrs) = kinds
        .iter()
        .find(|(rel, _, _)| rel == Path::new("My Documents"))
        .unwrap();
    assert_eq!(*kind, EntryKind::Reparse(ReparseKind::Junction));
    assert_ne!(attrs & FILE_ATTRIBUTE_REPARSE_POINT, 0);

    // A junction root is not entered (SPEC-03 §5).
    let stats = real()
        .walk(
            &root.join("My Documents"),
            &opts(),
            &mut |_| WalkControl::Continue,
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(stats, WalkStats::default());
    assert_eq!(
        real().metadata(&root.join("My Documents")).unwrap().kind,
        EntryKind::Reparse(ReparseKind::Junction)
    );
}

#[test]
fn long_paths_are_walked() {
    let dir = tempfile::tempdir().unwrap();
    let mut deep = dir.path().to_path_buf();
    while deep.as_os_str().len() < 400 {
        deep.push("a_rather_long_directory_name");
    }
    fs::create_dir_all(to_extended(&deep)).unwrap();
    let file = deep.join("save.sav");
    fs::write(to_extended(&file), b"save").unwrap();
    assert!(file.as_os_str().len() > 400);

    let mut found = None;
    let stats = real()
        .walk(
            dir.path(),
            &opts(),
            &mut |e| {
                if e.path.file_name() == Some(OsStr::new("save.sav")) {
                    found = Some(e.clone());
                }
                WalkControl::Continue
            },
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(stats.errors, 0);
    let found = found.unwrap();
    assert_eq!(found.path, file);
    assert_eq!(found.meta.size, 4);
    assert_eq!(real().metadata(&file).unwrap().size, 4);
    assert_eq!(real().read_dir(&deep).unwrap().len(), 1);
}

/// An OFFLINE file is listed as cloud-only with its size, and not opened:
/// the walk works while the file is held without sharing, and nothing changes.
#[test]
fn offline_file_is_cloud_only_and_not_opened() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("photo.jpg");
    fs::write(&file, b"jpeg-data").unwrap();
    set_attributes(&file, FILE_ATTRIBUTE_OFFLINE).unwrap();
    let holder = OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&file)
        .unwrap();

    let mut photo = None;
    let stats = real()
        .walk(
            dir.path(),
            &opts(),
            &mut |e| {
                photo = Some(e.meta.clone());
                WalkControl::Continue
            },
            &CancellationToken::new(),
        )
        .unwrap();
    drop(holder);
    assert_eq!(stats.errors, 0);
    let photo = photo.unwrap();
    assert_eq!(photo.kind, EntryKind::File);
    assert_eq!(photo.cloud, CloudState::CloudOnly);
    assert_eq!(photo.size, 9);
    assert_eq!(real().metadata(&file).unwrap().cloud, CloudState::CloudOnly);
    assert_eq!(real().probe_readable(&file), Readability::CloudOnly);

    assert_ne!(
        fs::metadata(&file).unwrap().file_attributes() & FILE_ATTRIBUTE_OFFLINE,
        0
    );
    assert_eq!(fs::read(&file).unwrap(), b"jpeg-data");
}

/// A file locked by another handle is still listed with its size.
#[test]
fn locked_file_is_listed() {
    let dir = sample();
    let file = dir.path().join("top.sav");
    let _holder = OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&file)
        .unwrap();
    let mut size = None;
    real()
        .walk(
            dir.path(),
            &opts(),
            &mut |e| {
                if e.rel == Path::new("top.sav") {
                    size = Some(e.meta.size);
                }
                WalkControl::Continue
            },
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(size, Some(5));
    assert_eq!(real().probe_readable(&file), Readability::Locked);
}

/// A path with wildcards names no entry, as in `MemFs` (SPEC-03 §4.1):
/// `top.*` must not describe `top.sav`.
#[test]
fn wildcard_paths_do_not_exist() {
    let dir = sample();
    let real = real();
    let mem = sample_mem();
    for pattern in ["top.*", "top.sa?", "*", "a/*.txt", "?/x.txt"] {
        let path = dir.path().join(pattern);
        assert!(!real.exists(&path), "{pattern}");
        assert!(
            matches!(real.metadata(&path), Err(FsError::NotFound)),
            "{pattern}"
        );
        assert_eq!(
            real.probe_readable(&path),
            Readability::Missing,
            "{pattern}"
        );
        let mem_path = Path::new("/r").join(pattern);
        assert!(!mem.exists(&mem_path), "{pattern}");
        assert!(
            matches!(mem.metadata(&mem_path), Err(FsError::NotFound)),
            "{pattern}"
        );
    }
    assert!(matches!(
        real.walk(
            &dir.path().join("*"),
            &opts(),
            &mut |_| WalkControl::Continue,
            &CancellationToken::new()
        ),
        Err(FsError::NotFound)
    ));
    assert!(real.exists(&dir.path().join("top.sav")));
}
