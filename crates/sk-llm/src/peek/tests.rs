//! Tests of [`content_peek`] and head decoding (SPEC-08 §4.9, §6.1) on an
//! in-memory file system that records every read.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use sk_core::env::Environment;
use sk_core::fs::{
    CloudState, DirEntryInfo, EntryKind, EntryMeta, FsError, FsScanner, Readability, ReparseKind,
    WalkControl, WalkOptions, WalkStats,
};
use sk_core::model::Category;
use sk_core::CancellationToken;

use super::*;

/// Files directly inside one folder; reads are recorded.
#[derive(Default)]
struct PeekFs {
    files: BTreeMap<PathBuf, (EntryMeta, Vec<u8>)>,
    reads: Mutex<Vec<PathBuf>>,
}

fn meta(kind: EntryKind, size: u64, cloud: CloudState) -> EntryMeta {
    EntryMeta {
        kind,
        size,
        mtime: None,
        ctime: None,
        attrs: 0,
        cloud,
    }
}

fn folder() -> PathBuf {
    PathBuf::from("root").join("App")
}

impl PeekFs {
    fn add(&mut self, name: &str, content: &[u8]) -> &mut Self {
        let m = meta(EntryKind::File, content.len() as u64, CloudState::Local);
        self.add_meta(name, m, content)
    }

    fn add_meta(&mut self, name: &str, meta: EntryMeta, content: &[u8]) -> &mut Self {
        self.files
            .insert(folder().join(name), (meta, content.to_vec()));
        self
    }

    fn reads(&self) -> Vec<String> {
        let reads = self.reads.lock().map(|r| r.clone()).unwrap_or_default();
        reads
            .iter()
            .filter_map(|p| p.file_name()?.to_str().map(str::to_owned))
            .collect()
    }
}

impl FsScanner for PeekFs {
    fn metadata(&self, path: &Path) -> Result<EntryMeta, FsError> {
        self.files
            .get(path)
            .map(|(m, _)| m.clone())
            .ok_or(FsError::NotFound)
    }

    fn exists(&self, path: &Path) -> bool {
        self.files.contains_key(path)
    }

    fn read_dir(&self, path: &Path) -> Result<Vec<DirEntryInfo>, FsError> {
        if path != folder() {
            return Err(FsError::NotFound);
        }
        Ok(self
            .files
            .iter()
            .map(|(p, (m, _))| DirEntryInfo {
                path: p.clone(),
                rel: PathBuf::from(p.file_name().unwrap_or_default()),
                depth: 1,
                meta: m.clone(),
            })
            .collect())
    }

    fn walk(
        &self,
        _: &Path,
        _: &WalkOptions,
        _: &mut dyn FnMut(&DirEntryInfo) -> WalkControl,
        _: &CancellationToken,
    ) -> Result<WalkStats, FsError> {
        Ok(WalkStats::default())
    }

    fn read_head(&self, path: &Path, max: usize) -> Result<Vec<u8>, FsError> {
        if let Ok(mut reads) = self.reads.lock() {
            reads.push(path.to_path_buf());
        }
        let (meta, content) = self.files.get(path).ok_or(FsError::NotFound)?;
        if meta.cloud == CloudState::CloudOnly {
            return Err(FsError::CloudOnly);
        }
        Ok(content[..content.len().min(max)].to_vec())
    }

    fn read_small(&self, path: &Path, max: usize) -> Result<Vec<u8>, FsError> {
        self.read_head(path, max)
    }

    fn probe_readable(&self, _: &Path) -> Readability {
        Readability::Ok
    }
}

fn env() -> Environment {
    let mut env = Environment::fake(Path::new(if cfg!(windows) { r"C:\fake" } else { "/fake" }));
    env.user_name = "Max".to_owned();
    env.machine_name = "DESKTOP-01".to_owned();
    env
}

fn peek(fs: &PeekFs, candidate: Category) -> Vec<ContentPeek> {
    content_peek(fs, &folder(), candidate, &env())
}

fn files(peeks: &[ContentPeek]) -> Vec<&str> {
    peeks.iter().map(|p| p.file.as_str()).collect()
}

#[test]
fn takes_up_to_three_text_files_by_name() {
    let mut fs = PeekFs::default();
    fs.add("b.json", b"{\"a\": 1}")
        .add("A.ini", b"x=1")
        .add("c.txt", b"hello")
        .add("d.yml", b"k: v")
        .add("image.png", b"x=1")
        .add("noext", b"x=1");
    let peeks = peek(&fs, Category::Unknown);
    assert_eq!(files(&peeks), ["A.ini", "b.json", "c.txt"]);
    assert_eq!(peeks[0].head, "x=1");
    assert_eq!(fs.reads(), ["A.ini", "b.json", "c.txt"]);
}

#[test]
fn extensions_match_ignoring_case() {
    let mut fs = PeekFs::default();
    for ext in PEEK_EXTENSIONS {
        fs.add(&format!("f.{}", ext.to_uppercase()), b"x");
    }
    assert_eq!(peek(&fs, Category::Unknown).len(), PEEK_MAX_FILES);
}

#[test]
fn skips_large_cloud_only_dirs_and_links_without_reading() {
    let mut fs = PeekFs::default();
    let big = vec![b'a'; PEEK_MAX_FILE_BYTES as usize + 1];
    fs.add("a_big.txt", &big)
        .add_meta(
            "b_cloud.ini",
            meta(EntryKind::File, 3, CloudState::CloudOnly),
            b"x=1",
        )
        .add_meta(
            "c_dir.json",
            meta(EntryKind::Dir, 0, CloudState::Local),
            b"",
        )
        .add_meta(
            "d_link.ini",
            meta(
                EntryKind::Reparse(ReparseKind::Symlink),
                3,
                CloudState::Local,
            ),
            b"x=1",
        )
        .add("e_ok.cfg", b"x=1");
    let max = vec![b'a'; PEEK_MAX_FILE_BYTES as usize];
    fs.add("f_full.txt", &max);
    assert_eq!(
        files(&peek(&fs, Category::Unknown)),
        ["e_ok.cfg", "f_full.txt"]
    );
    assert_eq!(fs.reads(), ["e_ok.cfg", "f_full.txt"]);
}

#[test]
fn credentials_are_never_read() {
    let mut fs = PeekFs::default();
    fs.add("vault.kdbx", b"x")
        .add("id.key", b"x")
        .add("a.ini", b"x=1");
    assert!(peek(&fs, Category::Credentials).is_empty());
    assert!(fs.reads().is_empty());
    assert_eq!(files(&peek(&fs, Category::Unknown)), ["a.ini"]);
    assert_eq!(fs.reads(), ["a.ini"]);
    assert!(PEEK_EXTENSIONS
        .iter()
        .all(|e| !CREDENTIAL_EXTENSIONS.contains(e)));
}

#[test]
fn reads_only_the_head() {
    let mut fs = PeekFs::default();
    let text = "word ".repeat(1000);
    fs.add("a.txt", text.as_bytes());
    let peeks = peek(&fs, Category::Unknown);
    assert_eq!(peeks[0].head.len(), PEEK_HEAD_BYTES);
}

#[test]
fn binary_and_blank_files_are_skipped() {
    let mut fs = PeekFs::default();
    fs.add("a.json", &[0xFF, 0x00, 0x12])
        .add("b.txt", b"a\0b")
        .add("c.ini", b"   \n")
        .add("d.ini", b"ok=1");
    assert_eq!(files(&peek(&fs, Category::Unknown)), ["d.ini"]);
}

#[test]
fn secrets_are_scrubbed_and_names_redacted() {
    let mut fs = PeekFs::default();
    fs.add("Max.ini", b"user=bob\npassword=hunter2\n");
    let peeks = peek(&fs, Category::Unknown);
    assert_eq!(peeks[0].file, "<redacted>.ini");
    assert_eq!(peeks[0].head, "user=bob\npassword=<redacted>\n");
}

#[test]
fn utf16_le_with_bom_is_converted() {
    let mut bytes = vec![0xFF, 0xFE];
    for u in "token=abc\nname=Привет".encode_utf16() {
        bytes.extend(u.to_le_bytes());
    }
    let mut fs = PeekFs::default();
    fs.add("a.ini", &bytes);
    assert_eq!(
        peek(&fs, Category::Unknown)[0].head,
        "token=<redacted>\nname=Привет"
    );
}

#[test]
fn decoding() {
    assert_eq!(decode_head(b"\xEF\xBB\xBFabc").as_deref(), Some("abc"));
    // "aП" with the last byte of "П" cut by the read limit.
    assert_eq!(decode_head(b"a\xD0").as_deref(), Some("a"));
    assert_eq!(decode_head(b"a\xFFb"), None);
    assert_eq!(decode_head(b"a\0b"), None);
    // UTF-16 LE: odd last byte and a cut surrogate pair are dropped.
    assert_eq!(
        decode_head(&[0xFF, 0xFE, b'h', 0, b'i', 0, 0x3D]).as_deref(),
        Some("hi")
    );
    assert_eq!(
        decode_head(&[0xFF, 0xFE, b'h', 0, 0x3D, 0xD8]).as_deref(),
        Some("h")
    );
    // An unpaired low surrogate is not text.
    assert_eq!(decode_head(&[0xFF, 0xFE, 0x00, 0xDC, b'h', 0]), None);
    // UTF-16 without a BOM contains NUL bytes.
    assert_eq!(decode_head(&[b'h', 0, b'i', 0]), None);
}

#[test]
fn unreadable_folder_gives_nothing() {
    let fs = PeekFs::default();
    assert!(content_peek(&fs, Path::new("missing"), Category::Unknown, &env()).is_empty());
}

#[test]
fn heads_failing_the_final_check_are_skipped() {
    // `\t` + `om` is serialized as `\tom`: the raw view of `final_check`
    // sees the user name `tom`.
    let mut env = env();
    env.user_name = "tom".to_owned();
    let mut fs = PeekFs::default();
    fs.add("a.txt", b"x\tom").add("b.txt", b"ok");
    let peeks = content_peek(&fs, &folder(), Category::Unknown, &env);
    assert_eq!(files(&peeks), ["b.txt"]);
    assert_eq!(fs.reads(), ["a.txt", "b.txt"]);
}

#[test]
fn fuzz_content_peek_never_panics() {
    use super::scrub_round2_tests::{random_text, Lcg};
    let env = env();
    let mut rng = Lcg(42);
    for round in 0..500 {
        let mut fs = PeekFs::default();
        for f in 0..4 {
            let mut bytes: Vec<u8> = match rng.below(4) {
                0 => vec![0xFF, 0xFE],
                1 => vec![0xEF, 0xBB, 0xBF],
                _ => Vec::new(),
            };
            if rng.below(3) == 0 {
                let n = rng.below(3000);
                bytes.extend((0..n).map(|_| rng.below(256) as u8));
            } else if bytes.starts_with(&[0xFF, 0xFE]) {
                for u in random_text(&mut rng).encode_utf16() {
                    bytes.extend(u.to_le_bytes());
                }
            } else {
                bytes.extend(random_text(&mut rng).repeat(1 + rng.below(80)).as_bytes());
            }
            let ext = PEEK_EXTENSIONS[(round + f) % PEEK_EXTENSIONS.len()];
            fs.add(&format!("f{f}.{ext}"), &bytes);
        }
        for p in content_peek(&fs, &folder(), Category::Unknown, &env) {
            let json = serde_json::json!({ "file": p.file, "head": p.head }).to_string();
            assert_eq!(crate::privacy::final_check(&json, &env), Ok(()));
        }
    }
}

#[test]
fn docker_compose_yaml_is_peeked() {
    let mut fs = PeekFs::default();
    let yaml = "version: \"3\"\nservices:\n  web:\n    image: nginx:1.25\n    ports:\n      - \"80:80\"\n  db:\n    image: postgres\n    environment:\n      POSTGRES_PASSWORD: hunter2\n";
    fs.add("docker-compose.yml", yaml.as_bytes());
    let peeks = peek(&fs, Category::Unknown);
    assert_eq!(files(&peeks), ["docker-compose.yml"]);
    assert!(peeks[0]
        .head
        .contains("services:\n  web:\n    image: nginx:1.25"));
    assert!(peeks[0].head.contains("POSTGRES_PASSWORD: <redacted>"));
    assert!(!peeks[0].head.contains("hunter2"));
}
