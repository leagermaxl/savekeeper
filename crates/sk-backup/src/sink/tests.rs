//! Round-trip tests of the sinks (SPEC-10 T-10-04): write entries, read them
//! back with the `zip` crate or from the folder, compare content and metadata.
//! Files are created under `<workspace>/target/tmp`, never in the system temp
//! folder.

use std::fs::{self, File};
use std::io::{self, Cursor, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use tempfile::TempDir;
use time::macros::datetime;
use time::OffsetDateTime;
use zip::{CompressionMethod, ExtraField, ZipArchive};

use super::zip_sink::method_for;
use super::*;

const MTIME: OffsetDateTime = datetime!(2026-09-27 22:10:04.123_456_7 UTC);
const CTIME: OffsetDateTime = datetime!(2025-03-01 10:00:00 UTC);

/// A fresh temporary folder under `<workspace>/target/tmp`.
fn temp_dir() -> TempDir {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
    fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new()
        .prefix("sk-backup-sink-")
        .tempdir_in(&base)
        .unwrap()
}

/// Content that deflate compresses well.
fn text(len: usize) -> Vec<u8> {
    b"SaveKeeper save data line\n"
        .iter()
        .copied()
        .cycle()
        .take(len)
        .collect()
}

fn entry<'a>(path: &'a str, attributes: &'a [FileAttribute], size: u64) -> SinkEntry<'a> {
    SinkEntry {
        archive_path: path,
        size_hint: size,
        mtime: Some(MTIME),
        ctime: Some(CTIME),
        attributes,
    }
}

/// One file to write in the round-trip tests.
struct Sample {
    path: &'static str,
    content: Vec<u8>,
    attributes: &'static [FileAttribute],
}

fn samples() -> Vec<Sample> {
    vec![
        Sample {
            path: "files/3fa2c9e01b7d44aa/SL2/ER0000.sl2",
            content: text(300_000),
            attributes: &[FileAttribute::Archive],
        },
        Sample {
            path: "files/3fa2c9e01b7d44aa/shot.PNG",
            content: text(10_000),
            attributes: &[FileAttribute::Readonly, FileAttribute::Hidden],
        },
        Sample {
            path: "files/91bd0f7e2c3a1188/Документы/сейв 1.sav",
            content: b"save".to_vec(),
            attributes: &[FileAttribute::System],
        },
        Sample {
            path: "files/91bd0f7e2c3a1188/empty.cfg",
            content: Vec::new(),
            attributes: &[],
        },
    ]
}

const EMPTY_DIR: &str = "files/3fa2c9e01b7d44aa/empty";

/// Writes the samples and the empty folder into `sink`; returns the container size.
fn write_samples(mut sink: Box<dyn ArchiveSink>) -> u64 {
    let cancel = CancellationToken::new();
    for s in samples() {
        let e = entry(s.path, s.attributes, s.content.len() as u64);
        let n = sink
            .add_file(&e, &mut Cursor::new(&s.content), &cancel)
            .unwrap();
        assert_eq!(n, s.content.len() as u64);
    }
    sink.add_dir(&entry(EMPTY_DIR, &[FileAttribute::Hidden], 0))
        .unwrap();
    sink.finish().unwrap()
}

/// A reader that fails after `ok` bytes.
struct Failing {
    ok: usize,
}

impl Read for Failing {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.ok == 0 {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "gone"));
        }
        let n = buf.len().min(self.ok);
        buf[..n].fill(7);
        self.ok -= n;
        Ok(n)
    }
}

/// An endless reader that cancels `token` on its first read.
struct Cancelling(CancellationToken);

impl Read for Cancelling {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.cancel();
        buf.fill(1);
        Ok(buf.len())
    }
}

// ---------------------------------------------------------------- helpers

#[test]
fn paths_are_validated() {
    for bad in [
        "", "/a", "a/", "a//b", "./a", "a/../b", "..", "a\\b", "C:/a", "a/b\u{1}",
    ] {
        assert!(
            matches!(validate_path(bad), Err(SinkError::InvalidPath(_))),
            "{bad:?}"
        );
    }
    validate_path("files/id/..~sk/a.~sk").unwrap();
    validate_path("files/id/Документы/a b.txt").unwrap();
}

#[test]
fn compressed_formats_and_level_zero_are_stored() {
    assert_eq!(method_for("files/a/x.txt", 6), CompressionMethod::Deflated);
    assert_eq!(method_for("files/a/x.TXT", 1), CompressionMethod::Deflated);
    for name in [
        "a.zip", "a.7z", "a.RAR", "a.jpg", "a.png", "a.mp4", "a.mkv", "a.mp3", "a.gz", "a.zst",
        "b/c.pak",
    ] {
        assert_eq!(method_for(name, 6), CompressionMethod::Stored, "{name}");
    }
    assert_eq!(method_for("a.png.d/noext", 6), CompressionMethod::Deflated);
    assert_eq!(method_for("files/a/x.txt", 0), CompressionMethod::Stored);
}

#[test]
fn attributes_and_file_times_convert() {
    let all = [
        FileAttribute::Readonly,
        FileAttribute::Hidden,
        FileAttribute::System,
        FileAttribute::Archive,
    ];
    assert_eq!(attribute_bits(&all), 0x27);
    assert_eq!(attributes_of(0x27 | ATTR_DIRECTORY), all.to_vec());
    assert_eq!(attribute_bits(&[]), 0);
    assert_eq!(
        file_time(OffsetDateTime::UNIX_EPOCH),
        Some(116_444_736_000_000_000)
    );
    assert_eq!(file_time(datetime!(1601-01-01 0:00 UTC)), Some(0));
    assert_eq!(file_time(datetime!(1600-12-31 0:00 UTC)), None);
}

// ---------------------------------------------------------------- zip

/// General purpose flags of the local and central headers of `file`.
fn header_flags(path: &Path, local: u64, central: u64) -> (u16, u16) {
    let mut f = File::open(path).unwrap();
    let mut read_u16 = |at: u64| {
        let mut b = [0u8; 2];
        f.seek(SeekFrom::Start(at)).unwrap();
        f.read_exact(&mut b).unwrap();
        u16::from_le_bytes(b)
    };
    (read_u16(local + 6), read_u16(central + 8))
}

#[test]
fn zip_round_trip() {
    let dir = temp_dir();
    let path = dir.path().join("backup.zip.partial");
    let size = write_samples(Box::new(ZipSink::create(&path, 6).unwrap()));
    assert_eq!(size, fs::metadata(&path).unwrap().len());

    let mut archive = ZipArchive::new(File::open(&path).unwrap()).unwrap();
    assert_eq!(archive.len(), samples().len() + 1);
    for (i, s) in samples().iter().enumerate() {
        let mut file = archive.by_index(i).unwrap();
        assert_eq!(file.name().unwrap(), s.path);
        let mut content = Vec::new();
        file.read_to_end(&mut content).unwrap();
        assert_eq!(content, s.content, "{}", s.path);
        assert_eq!(file.compression(), method_for(s.path, 6));
        assert_eq!(attributes_of(file.external_attributes()), s.attributes);
        assert_eq!(file.last_modified(), Some(zip_sink::dos_time(MTIME)));
        let flags = header_flags(&path, file.header_start(), file.central_header_start());
        let efs = if s.path.is_ascii() { 0 } else { 0x800 };
        assert_eq!((flags.0 & 0x800, flags.1 & 0x800), (efs, efs), "{}", s.path);
        let mut ntfs = None;
        let mut unix = None;
        for field in file.extra_data_fields() {
            match field {
                ExtraField::Ntfs(f) => ntfs = Some((f.mtime(), f.ctime())),
                ExtraField::ExtendedTimestamp(f) => unix = Some(f.mod_time()),
                _ => {}
            }
        }
        let expected = (file_time(MTIME).unwrap(), file_time(CTIME).unwrap());
        assert_eq!(ntfs, Some(expected), "{}", s.path);
        assert_eq!(unix, Some(Some(MTIME.unix_timestamp() as u32)));
    }
    let large = samples()[0].content.len() as u64;
    let deflated = archive.by_index(0).unwrap().compressed_size();
    assert!(deflated < large / 10, "{deflated} of {large}");
    let dir_entry = archive.by_name(&format!("{EMPTY_DIR}/")).unwrap();
    assert!(dir_entry.is_dir());
    assert_eq!(dir_entry.external_attributes() & 0xFF, 0x12);
}

#[test]
fn zip_level_zero_stores_everything() {
    let mut sink = ZipSink::new(Cursor::new(Vec::new()), 0);
    let cancel = CancellationToken::new();
    let data = text(50_000);
    sink.add_file(&entry("a.txt", &[], 0), &mut data.as_slice(), &cancel)
        .unwrap();
    let mut archive = ZipArchive::new(sink.finish_into_inner().unwrap()).unwrap();
    let file = archive.by_index(0).unwrap();
    assert_eq!(file.compression(), CompressionMethod::Stored);
    assert_eq!(file.compressed_size(), 50_000);
}

#[test]
fn zip_entry_without_times_has_default_dos_time() {
    let mut sink = ZipSink::new(Cursor::new(Vec::new()), 6);
    let e = SinkEntry {
        mtime: None,
        ctime: None,
        ..entry("a.txt", &[], 1)
    };
    sink.add_file(&e, &mut &b"x"[..], &CancellationToken::new())
        .unwrap();
    let mut archive = ZipArchive::new(sink.finish_into_inner().unwrap()).unwrap();
    let file = archive.by_index(0).unwrap();
    assert_eq!(file.last_modified(), Some(zip::DateTime::DEFAULT));
    assert_eq!(file.extra_data_fields().count(), 0);
}

#[test]
fn zip_large_entry_gets_zip64_headers() {
    let mut sink = ZipSink::new(Cursor::new(Vec::new()), 6);
    let cancel = CancellationToken::new();
    sink.add_file(&entry("big.bin", &[], 5 << 30), &mut &b"x"[..], &cancel)
        .unwrap();
    sink.add_file(&entry("small.bin", &[], 10), &mut &b"y"[..], &cancel)
        .unwrap();
    let mut archive = ZipArchive::new(sink.finish_into_inner().unwrap()).unwrap();
    assert_eq!(archive.by_name("big.bin").unwrap().version_needed(), 45);
    assert!(archive.by_name("small.bin").unwrap().version_needed() < 45);
}

#[test]
fn zip_failed_duplicate_and_cancelled_entries_are_dropped() {
    let mut sink = ZipSink::new(Cursor::new(Vec::new()), 6);
    let cancel = CancellationToken::new();
    sink.add_file(&entry("a.txt", &[], 1), &mut &b"a"[..], &cancel)
        .unwrap();
    let err = sink
        .add_file(&entry("a.txt", &[], 1), &mut &b"b"[..], &cancel)
        .unwrap_err();
    assert!(
        matches!(err, SinkError::Duplicate(ref p) if p == "a.txt"),
        "{err:?}"
    );
    let err = sink
        .add_file(
            &entry("b.txt", &[], 1),
            &mut Failing { ok: 3 << 20 },
            &cancel,
        )
        .unwrap_err();
    assert!(matches!(err, SinkError::Source(_)), "{err:?}");
    let token = CancellationToken::new();
    let err = sink
        .add_file(
            &entry("c.txt", &[], 1),
            &mut Cancelling(token.clone()),
            &token,
        )
        .unwrap_err();
    assert!(matches!(err, SinkError::Cancelled), "{err:?}");
    let err = sink
        .add_file(&entry("d.txt", &[], 1), &mut &b"d"[..], &token)
        .unwrap_err();
    assert!(matches!(err, SinkError::Cancelled), "{err:?}");
    let err = sink
        .add_file(&entry("../e.txt", &[], 1), &mut &b"e"[..], &cancel)
        .unwrap_err();
    assert!(matches!(err, SinkError::InvalidPath(_)), "{err:?}");
    // The archive stays valid and holds only the good entries.
    sink.add_file(&entry("b.txt", &[], 1), &mut &b"b2"[..], &cancel)
        .unwrap();
    let mut archive = ZipArchive::new(sink.finish_into_inner().unwrap()).unwrap();
    let names: Vec<String> = archive
        .file_names()
        .map(|n| n.unwrap().into_owned())
        .collect();
    assert_eq!(names, ["a.txt", "b.txt"]);
    let mut content = String::new();
    archive
        .by_name("b.txt")
        .unwrap()
        .read_to_string(&mut content)
        .unwrap();
    assert_eq!(content, "b2");
}

#[test]
fn zip_never_overwrites_an_existing_file() {
    let dir = temp_dir();
    let path = dir.path().join("taken.zip");
    fs::write(&path, b"old").unwrap();
    assert!(matches!(
        ZipSink::create(&path, 6),
        Err(SinkError::Target(_))
    ));
    assert_eq!(fs::read(&path).unwrap(), b"old");
}

// ---------------------------------------------------------------- dir

fn on_disk(root: &Path, archive_path: &str) -> PathBuf {
    let mut path = sk_core::path::to_extended(root);
    path.extend(archive_path.split('/'));
    path
}

/// Attributes of an entry on disk, as saved in the manifest.
fn disk_attributes(path: &Path) -> Vec<FileAttribute> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        attributes_of(fs::metadata(path).unwrap().file_attributes())
    }
    #[cfg(not(windows))]
    {
        if fs::metadata(path).unwrap().permissions().readonly() {
            vec![FileAttribute::Readonly]
        } else {
            Vec::new()
        }
    }
}

/// Clears the read-only flag so that the temporary folder can be removed.
fn make_writable(path: &Path) {
    let mut perms = fs::metadata(path).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    fs::set_permissions(path, perms).unwrap();
}

#[test]
fn dir_round_trip() {
    let dir = temp_dir();
    let root = dir.path().join("backup.partial");
    let size = write_samples(Box::new(DirSink::create(&root).unwrap()));
    let total: usize = samples().iter().map(|s| s.content.len()).sum();
    assert_eq!(size, total as u64);

    for s in samples() {
        let path = on_disk(&root, s.path);
        assert_eq!(fs::read(&path).unwrap(), s.content, "{}", s.path);
        let meta = fs::metadata(&path).unwrap();
        assert_eq!(
            OffsetDateTime::from(meta.modified().unwrap()),
            MTIME,
            "{}",
            s.path
        );
        #[cfg(windows)]
        assert_eq!(OffsetDateTime::from(meta.created().unwrap()), CTIME);
        let expected: Vec<_> = if cfg!(windows) {
            s.attributes.to_vec()
        } else {
            s.attributes
                .iter()
                .copied()
                .filter(|a| *a == FileAttribute::Readonly)
                .collect()
        };
        assert_eq!(disk_attributes(&path), expected, "{}", s.path);
        make_writable(&path);
    }
    let empty = on_disk(&root, EMPTY_DIR);
    assert!(fs::read_dir(&empty).unwrap().next().is_none());
    let meta = fs::metadata(&empty).unwrap();
    assert_eq!(OffsetDateTime::from(meta.modified().unwrap()), MTIME);
    #[cfg(windows)]
    assert_eq!(disk_attributes(&empty), [FileAttribute::Hidden]);
}

#[test]
fn dir_failed_duplicate_and_cancelled_entries_are_dropped() {
    let dir = temp_dir();
    let root = dir.path().join("backup.partial");
    let mut sink = DirSink::create(&root).unwrap();
    let cancel = CancellationToken::new();
    sink.add_file(&entry("a/a.txt", &[], 1), &mut &b"a"[..], &cancel)
        .unwrap();
    let err = sink
        .add_file(&entry("a/a.txt", &[], 1), &mut &b"b"[..], &cancel)
        .unwrap_err();
    assert!(matches!(err, SinkError::Duplicate(_)), "{err:?}");
    assert_eq!(fs::read(on_disk(&root, "a/a.txt")).unwrap(), b"a");
    let err = sink
        .add_file(
            &entry("a/b.txt", &[], 1),
            &mut Failing { ok: 3 << 20 },
            &cancel,
        )
        .unwrap_err();
    assert!(matches!(err, SinkError::Source(_)), "{err:?}");
    assert!(!on_disk(&root, "a/b.txt").exists());
    let token = CancellationToken::new();
    let err = sink
        .add_file(
            &entry("a/c.txt", &[], 1),
            &mut Cancelling(token.clone()),
            &token,
        )
        .unwrap_err();
    assert!(matches!(err, SinkError::Cancelled), "{err:?}");
    assert!(!on_disk(&root, "a/c.txt").exists());
    let err = sink
        .add_file(&entry("a/../../x.txt", &[], 1), &mut &b"x"[..], &cancel)
        .unwrap_err();
    assert!(matches!(err, SinkError::InvalidPath(_)), "{err:?}");
    assert_eq!(Box::new(sink).finish().unwrap(), 1);
}

#[test]
fn dir_never_writes_into_an_existing_folder() {
    let dir = temp_dir();
    assert!(matches!(
        DirSink::create(dir.path()),
        Err(SinkError::Target(_))
    ));
}

/// Long target paths work through the extended form (§5).
#[cfg(windows)]
#[test]
fn dir_writes_paths_longer_than_max_path() {
    let dir = temp_dir();
    let root = dir.path().join("backup.partial");
    let mut sink = DirSink::create(&root).unwrap();
    let long = format!(
        "files/id/{}/save.sav",
        ["a-rather-long-folder-name"; 12].join("/")
    );
    sink.add_file(
        &entry(&long, &[FileAttribute::Archive], 4),
        &mut &b"save"[..],
        &CancellationToken::new(),
    )
    .unwrap();
    let path = on_disk(&root, &long);
    assert!(path.as_os_str().len() > 400);
    assert_eq!(fs::read(&path).unwrap(), b"save");
    let meta = fs::metadata(&path).unwrap();
    assert_eq!(OffsetDateTime::from(meta.modified().unwrap()), MTIME);
    assert_eq!(disk_attributes(&path), [FileAttribute::Archive]);
}
