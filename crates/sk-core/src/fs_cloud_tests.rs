//! Tests of [`cloud_state`] (SPEC-02 §3.5, T-02-12) and [`set_attributes`]
//! (SPEC-02 §3.7) on the real file system.
//!
//! Files are created in a temporary folder under `<workspace>/target/tmp`,
//! not in the system temp folder; on Windows only attributes of these test
//! files are changed.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::*;
use crate::path::to_extended;

/// A fresh temporary folder under `<workspace>/target/tmp`.
fn temp_dir() -> TempDir {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
    fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new()
        .prefix("sk-core-cloud-")
        .tempdir_in(&base)
        .unwrap()
}

/// A folder below `root` whose path is longer than 260 characters.
fn long_dir(root: &Path) -> PathBuf {
    let mut dir = root.to_path_buf();
    while dir.as_os_str().len() <= 300 {
        dir.push("a-rather-long-folder-name-for-max-path");
    }
    fs::create_dir_all(to_extended(&dir)).unwrap();
    dir
}

/// Creates the file `name` in `dir` with a little content.
fn file(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(to_extended(&path), b"save").unwrap();
    path
}

#[test]
fn missing_path_is_not_found() {
    let dir = temp_dir();
    let err = cloud_state(&dir.path().join("missing.sav")).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::NotFound);
    let err = cloud_state(&dir.path().join("missing/inner.sav")).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::NotFound);
}

#[test]
fn plain_file_and_folder_are_local() {
    let dir = temp_dir();
    let path = file(dir.path(), "plain.sav");
    assert_eq!(cloud_state(&path).unwrap(), CloudState::Local);
    assert_eq!(cloud_state(dir.path()).unwrap(), CloudState::Local);

    let deep = long_dir(dir.path());
    let path = file(&deep, "deep.sav");
    assert!(path.as_os_str().len() > 260);
    assert_eq!(cloud_state(&path).unwrap(), CloudState::Local);
    assert_eq!(cloud_state(&deep).unwrap(), CloudState::Local);
    let err = cloud_state(&deep.join("missing.sav")).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::NotFound);
}

#[cfg(windows)]
mod win {
    use std::os::windows::fs::MetadataExt;

    use super::*;

    const FILE_ATTRIBUTE_OFFLINE: u32 = 0x1000;

    /// Marks `path` as offline and checks that `cloud_state` sees it without
    /// changing anything.
    fn assert_offline_is_cloud_only(path: &Path) {
        set_attributes(path, FILE_ATTRIBUTE_OFFLINE).unwrap();
        assert_eq!(cloud_state(path).unwrap(), CloudState::CloudOnly);
        let attrs = fs::metadata(to_extended(path)).unwrap().file_attributes();
        assert_ne!(attrs & FILE_ATTRIBUTE_OFFLINE, 0);
    }

    #[test]
    fn offline_file_is_cloud_only() {
        let dir = temp_dir();
        let path = file(dir.path(), "cloud.sav");
        assert_offline_is_cloud_only(&path);
        assert_eq!(fs::read(&path).unwrap(), b"save");
    }

    #[test]
    fn offline_file_with_long_path_is_cloud_only() {
        let dir = temp_dir();
        let deep = long_dir(dir.path());
        let path = file(&deep, "cloud.sav");
        assert!(path.as_os_str().len() > 260);
        assert_offline_is_cloud_only(&path);
        // The extended form of the same path gives the same answer.
        assert_eq!(
            cloud_state(&to_extended(&path)).unwrap(),
            CloudState::CloudOnly
        );
    }

    /// `set_attributes` sets exactly the given flags; 0 clears them (`NORMAL`).
    #[test]
    fn set_attributes_on_long_path() {
        const READONLY_HIDDEN_SYSTEM: u32 = 0x1 | 0x2 | 0x4;
        let dir = temp_dir();
        let path = file(&long_dir(dir.path()), "attrs.sav");
        set_attributes(&path, READONLY_HIDDEN_SYSTEM).unwrap();
        let attrs = crate::win::fs::attributes(&path).unwrap();
        assert_eq!(attrs & 0x27, READONLY_HIDDEN_SYSTEM);
        set_attributes(&path, 0).unwrap();
        let attrs = crate::win::fs::attributes(&path).unwrap();
        assert_eq!(attrs & 0x27, 0);
        let err = set_attributes(&dir.path().join("missing.sav"), 0).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound);
    }

    #[test]
    fn invalid_name_error_is_returned_as_is() {
        let dir = temp_dir();
        let err = cloud_state(&dir.path().join("a*b.sav")).unwrap_err();
        assert_ne!(err.kind(), ErrorKind::NotFound);
        assert!(err.raw_os_error().is_some());
    }
}

#[cfg(not(windows))]
#[test]
fn existing_path_is_local_outside_windows() {
    let dir = temp_dir();
    let path = file(dir.path(), "plain.sav");
    assert_eq!(cloud_state(&path).unwrap(), CloudState::Local);
}

/// Outside Windows only the read-only flag has an effect.
#[cfg(not(windows))]
#[test]
fn set_attributes_maps_readonly_outside_windows() {
    let dir = temp_dir();
    let path = file(dir.path(), "plain.sav");
    set_attributes(&path, 0x1 | 0x2).unwrap();
    assert!(fs::metadata(&path).unwrap().permissions().readonly());
    set_attributes(&path, 0).unwrap();
    assert!(!fs::metadata(&path).unwrap().permissions().readonly());
}
