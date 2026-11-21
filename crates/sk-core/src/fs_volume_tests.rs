//! Tests of [`volume_info`] (SPEC-10 §4.6) on the real file system.
//!
//! Folders are created under `<workspace>/target/tmp`, not in the system temp
//! folder.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::*;
use crate::path::starts_with_ci;

fn temp_dir() -> TempDir {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
    fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new()
        .prefix("sk-core-volume-")
        .tempdir_in(&base)
        .unwrap()
}

#[test]
fn folder_has_a_volume_with_free_space() {
    let dir = temp_dir();
    let info = volume_info(dir.path()).unwrap();
    assert!(info.free_bytes > 0);
    if cfg!(windows) {
        let canonical = dunce_like(dir.path());
        assert!(
            starts_with_ci(&canonical, &info.root),
            "{info:?} vs {canonical:?}"
        );
        assert!(info.fs_name.is_some(), "{info:?}");
        assert!(!info.remote);
    } else {
        assert_eq!(info.free_bytes, u64::MAX);
        assert_eq!(info.fs_name, None);
    }
}

#[test]
fn missing_folder_is_not_found_and_file_is_not_a_directory() {
    let dir = temp_dir();
    let missing = volume_info(&dir.path().join("missing")).unwrap_err();
    assert_eq!(missing.kind(), ErrorKind::NotFound);
    let file = dir.path().join("file.txt");
    fs::write(&file, b"x").unwrap();
    let not_dir = volume_info(&file).unwrap_err();
    assert_eq!(not_dir.kind(), ErrorKind::NotADirectory);
}

#[cfg(windows)]
#[test]
fn long_path_and_system_drive() {
    let dir = temp_dir();
    let mut long = dir.path().to_path_buf();
    while long.as_os_str().len() <= 300 {
        long.push("a-rather-long-folder-name-for-max-path");
    }
    fs::create_dir_all(crate::path::to_extended(&long)).unwrap();
    let info = volume_info(&long).unwrap();
    let short = volume_info(dir.path()).unwrap();
    assert_eq!((&info.root, &info.fs_name), (&short.root, &short.fs_name));
    assert!(info.free_bytes > 0);

    let windir = PathBuf::from(std::env::var_os("SystemRoot").unwrap());
    let system = volume_info(&windir).unwrap();
    assert!(system.system, "{system:?}");
    assert_eq!(info.system, crate::path::eq_ci(&info.root, &system.root));
}

/// Absolute path as Windows reports it, without the `\\?\` prefix.
fn dunce_like(path: &Path) -> PathBuf {
    let canonical = fs::canonicalize(path).unwrap();
    let text = canonical.to_string_lossy();
    PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned())
}
