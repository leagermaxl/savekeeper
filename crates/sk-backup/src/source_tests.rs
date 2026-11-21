//! Tests of source reading (SPEC-10 T-10-05): retries by the policy, hashing
//! while copying, `changed_during_read`, errors of `open_source`. Files are
//! created under `<workspace>/target/tmp`, never in the system temp folder.

#![allow(clippy::unwrap_used)]

use std::cell::Cell;
use std::fs::{self, OpenOptions};
use std::io::{Cursor, Write};

use tempfile::TempDir;

use super::*;

/// A fresh temporary folder under `<workspace>/target/tmp`.
pub(super) fn temp_dir() -> TempDir {
    // Absolute without `..`, so that `mklink` accepts it.
    let base = std::path::absolute(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp"))
        .unwrap();
    fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new()
        .prefix("sk-backup-source-")
        .tempdir_in(&base)
        .unwrap()
}

fn locked() -> OpenError {
    OpenError::Locked(io::Error::other("sharing violation"))
}

fn policy(attempts: u32, delays_ms: &[u64]) -> RetryPolicy {
    RetryPolicy {
        attempts,
        delays_ms: delays_ms.to_vec(),
    }
}

/// Runs `with_retries` with an op that fails `Locked` `failures` times;
/// returns the result, the number of calls and the waits.
fn run_retries(
    policy: &RetryPolicy,
    failures: u32,
    cancel_after_waits: Option<usize>,
) -> (Result<u32, OpenError>, u32, Vec<u64>) {
    let calls = Cell::new(0u32);
    let mut waits = Vec::new();
    let result = with_retries(
        policy,
        &mut |d| {
            waits.push(u64::try_from(d.as_millis()).unwrap());
            cancel_after_waits.is_none_or(|n| waits.len() < n)
        },
        &mut || {
            calls.set(calls.get() + 1);
            if calls.get() <= failures {
                Err(locked())
            } else {
                Ok(calls.get())
            }
        },
    );
    (result, calls.get(), waits)
}

#[test]
fn default_policy_retries_three_times_with_200_500_1000() {
    let (result, calls, waits) = run_retries(&RetryPolicy::default(), u32::MAX, None);
    assert!(matches!(result, Err(OpenError::Locked(_))));
    assert_eq!(calls, 4, "first attempt and three retries");
    assert_eq!(waits, [200, 500, 1000]);
}

#[test]
fn success_on_a_retry_stops_retrying() {
    let (result, calls, waits) = run_retries(&RetryPolicy::default(), 2, None);
    assert_eq!(result.unwrap(), 3);
    assert_eq!(calls, 3);
    assert_eq!(waits, [200, 500]);
}

#[test]
fn last_delay_repeats_and_no_delays_means_no_wait() {
    let (_, calls, waits) = run_retries(&policy(4, &[10, 20]), u32::MAX, None);
    assert_eq!(calls, 5);
    assert_eq!(waits, [10, 20, 20, 20]);

    let (_, calls, waits) = run_retries(&policy(2, &[]), u32::MAX, None);
    assert_eq!(calls, 3);
    assert_eq!(waits, [0, 0]);
}

#[test]
fn zero_attempts_means_no_retry() {
    let (result, calls, waits) = run_retries(&policy(0, &[200]), u32::MAX, None);
    assert!(matches!(result, Err(OpenError::Locked(_))));
    assert_eq!(calls, 1);
    assert!(waits.is_empty());
}

#[test]
fn cancel_during_a_wait_stops_with_cancelled() {
    let (result, calls, waits) = run_retries(&RetryPolicy::default(), u32::MAX, Some(2));
    assert!(matches!(result, Err(OpenError::Cancelled)));
    assert_eq!(calls, 2);
    assert_eq!(waits, [200, 500]);
}

#[test]
fn other_errors_are_not_retried() {
    let calls = Cell::new(0u32);
    let result: Result<(), _> = with_retries(
        &RetryPolicy::default(),
        &mut |_| panic!("no wait expected"),
        &mut || {
            calls.set(calls.get() + 1);
            Err(OpenError::AccessDenied(io::Error::from(
                ErrorKind::PermissionDenied,
            )))
        },
    );
    assert!(matches!(result, Err(OpenError::AccessDenied(_))));
    assert_eq!(calls.get(), 1);
}

#[test]
fn wait_honours_cancellation() {
    let cancel = CancellationToken::new();
    assert!(wait(Duration::from_millis(1), &cancel));
    cancel.cancel();
    let started = std::time::Instant::now();
    assert!(!wait(Duration::from_secs(10), &cancel));
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn error_codes_and_io_classification() {
    assert_eq!(OpenError::CloudOnly.code(), ErrorCode::CloudOnly);
    assert_eq!(locked().code(), ErrorCode::Locked);
    assert_eq!(OpenError::NotAFile.code(), ErrorCode::Io);
    assert_eq!(OpenError::Cancelled.code(), ErrorCode::Cancelled);
    let code = |e: io::Error| OpenError::from_io(e).code();
    assert_eq!(code(ErrorKind::NotFound.into()), ErrorCode::NotFound);
    assert_eq!(
        code(ErrorKind::PermissionDenied.into()),
        ErrorCode::AccessDenied
    );
    assert_eq!(
        code(io::Error::from_raw_os_error(ERROR_FILENAME_EXCED_RANGE)),
        ErrorCode::PathTooLong
    );
    assert_eq!(code(io::Error::other("x")), ErrorCode::Io);
    let lock_code = if cfg!(windows) {
        ErrorCode::Locked
    } else {
        ErrorCode::Io
    };
    for raw in [ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION] {
        let e = io::Error::from_raw_os_error(raw);
        assert_eq!(is_lock_error(&e), cfg!(windows));
        assert_eq!(code(e), lock_code);
    }
}

#[test]
fn hashing_reader_hashes_and_counts_what_is_read() {
    let data: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    let mut reader = HashingReader::new(Cursor::new(&data));
    let mut out = Vec::new();
    let copied = io::copy(&mut reader, &mut out).unwrap();
    assert_eq!(copied, data.len() as u64);
    assert_eq!(out, data);
    assert_eq!(reader.bytes_read(), data.len() as u64);
    let hashed = reader.finish();
    assert_eq!(hashed.bytes, data.len() as u64);
    assert_eq!(hashed.blake3, blake3::hash(&data).to_hex().to_string());
    assert_eq!(hashed.blake3.len(), 64);

    let empty = HashingReader::new(Cursor::new(Vec::<u8>::new())).finish();
    assert_eq!(empty.bytes, 0);
    assert_eq!(empty.blake3, blake3::hash(b"").to_hex().to_string());
}

#[test]
fn open_source_reads_content_and_metadata() {
    let dir = temp_dir();
    let path = dir.path().join("save.sl2");
    fs::write(&path, b"save data").unwrap();
    let before = fs::metadata(&path).unwrap();

    let mut source =
        open_source(&path, &RetryPolicy::default(), &CancellationToken::new()).unwrap();
    assert_eq!(source.meta().size, 9);
    assert_eq!(
        source.meta().mtime,
        before.modified().ok().map(OffsetDateTime::from)
    );
    let mut reader = HashingReader::new(&mut source);
    let mut out = Vec::new();
    reader.read_to_end(&mut out).unwrap();
    let hashed = reader.finish();
    assert_eq!(out, b"save data");
    assert_eq!(
        hashed.blake3,
        blake3::hash(b"save data").to_hex().to_string()
    );
    assert!(!source.changed_during_read(hashed.bytes));
    drop(source);

    // Principle P1: the source is untouched.
    let after = fs::metadata(&path).unwrap();
    assert_eq!(after.len(), before.len());
    assert_eq!(after.modified().unwrap(), before.modified().unwrap());
    assert_eq!(after.permissions(), before.permissions());
    assert_eq!(fs::read(&path).unwrap(), b"save data");
}

#[test]
fn readonly_attribute_is_reported() {
    let dir = temp_dir();
    let path = dir.path().join("ro.cfg");
    fs::write(&path, b"x").unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_readonly(true);
    fs::set_permissions(&path, perms.clone()).unwrap();

    let source = open_source(&path, &RetryPolicy::default(), &CancellationToken::new()).unwrap();
    assert!(source
        .meta()
        .attributes()
        .contains(&FileAttribute::Readonly));
    drop(source);

    // Make the file writable again so that the temporary folder can go.
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    fs::set_permissions(&path, perms).unwrap();
}

#[test]
fn change_while_reading_is_detected() {
    let dir = temp_dir();
    let path = dir.path().join("save.sl2");
    fs::write(&path, b"0123456789").unwrap();

    let mut source =
        open_source(&path, &RetryPolicy::default(), &CancellationToken::new()).unwrap();
    let mut reader = HashingReader::new(&mut source);
    let mut head = [0u8; 4];
    reader.read_exact(&mut head).unwrap();
    // A game appends to its save while it is read (allowed by the sharing mode).
    let mut writer = OpenOptions::new().append(true).open(&path).unwrap();
    writer.write_all(b"more").unwrap();
    writer.flush().unwrap();
    drop(writer);
    let mut rest = Vec::new();
    reader.read_to_end(&mut rest).unwrap();
    let bytes = reader.finish().bytes;
    assert!(source.changed_during_read(bytes));
}

#[test]
fn short_read_counts_as_changed() {
    let dir = temp_dir();
    let path = dir.path().join("a.bin");
    fs::write(&path, b"abc").unwrap();
    let source = open_source(&path, &RetryPolicy::default(), &CancellationToken::new()).unwrap();
    assert!(source.changed_during_read(2));
    assert!(!source.changed_during_read(3));
}

#[test]
fn open_source_errors() {
    let dir = temp_dir();
    let policy = RetryPolicy::default();
    let cancel = CancellationToken::new();

    let missing = open_source(&dir.path().join("missing.txt"), &policy, &cancel);
    assert!(matches!(missing, Err(OpenError::NotFound(_))));

    let folder = open_source(dir.path(), &policy, &cancel);
    assert!(matches!(folder, Err(OpenError::NotAFile)));

    let file = dir.path().join("f.txt");
    fs::write(&file, b"x").unwrap();
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(matches!(
        open_source(&file, &policy, &cancelled),
        Err(OpenError::Cancelled)
    ));
}

#[cfg(unix)]
#[test]
fn symlink_is_not_followed() {
    let dir = temp_dir();
    let target = dir.path().join("target.txt");
    fs::write(&target, b"x").unwrap();
    let link = dir.path().join("link.txt");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    assert!(matches!(
        open_source(&link, &RetryPolicy::default(), &CancellationToken::new()),
        Err(OpenError::NotAFile)
    ));
}
