//! Tests of the helpers shared by the exporters.

use std::io::{self, Read};

use super::partial::{Partial, ISSUE_FILE_COPY_FAILED};
use super::test_util::{files_under, temp_dir};
use super::*;
use crate::error::ExportError;
use crate::exporter::RestoreHint;

/// A source that yields some bytes, then fails, like a cloud-only placeholder whose
/// download stops or a disk read error.
struct FailingReader {
    sent: bool,
}

impl Read for FailingReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.sent {
            return Err(io::Error::other("read failed midway"));
        }
        self.sent = true;
        let data = b"partial";
        buf[..data.len()].copy_from_slice(data);
        Ok(data.len())
    }
}

#[test]
fn copy_file_copies_and_hashes() {
    let dir = temp_dir();
    let src = dir.path().join("src.ttf");
    std::fs::write(&src, b"font data").unwrap();
    let target = dir.path().join("out");
    let file = copy_file(&src, &target, Path::new("user").join("a.ttf")).unwrap();
    assert_eq!(file.rel_path, Path::new("user").join("a.ttf"));
    assert_eq!(file.bytes, 9);
    assert_eq!(file.blake3, blake3::hash(b"font data").to_hex().to_string());
    assert_eq!(
        std::fs::read(target.join("user").join("a.ttf")).unwrap(),
        b"font data"
    );
    assert_eq!(std::fs::read(&src).unwrap(), b"font data");
}

/// A read failure after the output file was created leaves no file behind (FR-06-02).
#[test]
fn failed_read_midway_removes_the_partial_output() {
    let dir = temp_dir();
    let path = dir.path().join("partial.ttf");
    let result = write_new(&path, |dest| {
        copy_hashed(&mut FailingReader { sent: false }, dest)
    });
    let err = result.unwrap_err();
    assert_eq!(err.to_string(), "read failed midway");
    assert!(!path.exists());
    assert!(files_under(dir.path()).is_empty());
}

/// A source that cannot be read (a folder: `open` fails on Windows, `read` on Unix)
/// leaves no file in the target folder and is reported as `file_copy_failed`.
#[test]
fn unreadable_source_leaves_no_file_and_is_a_copy_warning() {
    let dir = temp_dir();
    let src = dir.path().join("folder.ttf");
    std::fs::create_dir_all(&src).unwrap();
    let target = dir.path().join("out");

    let err = copy_file(&src, &target, Path::new("user").join("folder.ttf")).unwrap_err();
    let mut failures = Partial::default();
    failures
        .add_path(ISSUE_FILE_COPY_FAILED, &src, err.into())
        .unwrap();

    assert!(!target.join("user").join("folder.ttf").exists());
    if target.exists() {
        assert!(files_under(&target).is_empty());
    }
    let result = failures.finish(Vec::new(), RestoreHint::None);
    assert!(matches!(result, Err(ExportError::Io(_))), "{result:?}");

    let mut failures = Partial::default();
    let err = copy_file(&src, &target, "x.ttf").unwrap_err();
    failures
        .add_path(ISSUE_FILE_COPY_FAILED, &src, err.into())
        .unwrap();
    let ok = failures
        .finish(
            vec![write_file(&target, "ok.txt", b"ok").unwrap()],
            RestoreHint::None,
        )
        .unwrap();
    assert_eq!(ok.warnings.len(), 1);
    assert_eq!(ok.warnings[0].message_key, ISSUE_FILE_COPY_FAILED);
    assert_eq!(ok.warnings[0].message_args["path"], src.to_string_lossy());
    assert_eq!(files_under(&target), [Path::new("ok.txt")]);
}
