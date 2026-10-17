//! The persistent tree of the H-USR benchmark (SPEC-07 §6).
//!
//! Shape of the SPEC-03 walk tree: `levels` of folders with [`FAN_OUT`]
//! children each, [`FILES_PER_DIR`] files in each leaf of the last level.
//! File `f` of leaf `k` is `f<fff>.<ext>`, the extension cycling over
//! [`EXTENSIONS`]; files are empty except:
//! - leaf `k % 100 == 0`: `f000.docx` of [`DOC_BYTES`] — a cluster;
//! - leaf `k % 1000 == 500` (also a cluster leaf): `f001.exe` of
//!   [`BINARY_LEAF_BYTES`], so the leaf is ≥ 30% binary and is no cluster;
//! - cluster leaves `k % 1000 == 300` and `k % 1000 == 700`: their files
//!   from `f050` on are in a junk subfolder `cache` / `logs`, `f050.dat` of
//!   [`JUNK_BYTES`] (≥ 1 MiB: the cluster gets it in `exclude`, T-07-13);
//! - leaf `k % 1000 == 905`: `f000.kdbx` of [`KDBX_BYTES`] (written), a
//!   valuable file outside any cluster;
//! - every first-level folder (when there are at least two levels) holds
//!   `bin.exe` of [`BLOCKER_BYTES`]: it and the tree root are ≥ 30% binary,
//!   so they are no clusters and the clusters are the leaves (§4.3.3 p. 3).
//!
//! Sized files are made by `set_len`; no data is written to them.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf, Prefix};
use std::time::{Duration, Instant};

/// Files in one leaf folder.
const FILES_PER_DIR: u64 = 100;
/// Subfolders of one inner folder.
const FAN_OUT: u64 = 10;
/// Version of the generator; bump it when the layout of the tree changes.
const GENERATOR_VERSION: u32 = 1;
/// Extensions of the files: document, photo, code, binary, no group.
const EXTENSIONS: [&str; 5] = ["docx", "jpg", "rs", "exe", "dat"];
const MIB: u64 = 1024 * 1024;
/// The document of a cluster leaf: weight 21 ≥ `min_weight` 20.
const DOC_BYTES: u64 = 21 * MIB;
/// The program of a binary leaf: 10 / 31 ≥ 30%.
const BINARY_LEAF_BYTES: u64 = 10 * MIB;
/// The program of a first-level folder: with its 210 MiB of documents and
/// 4 MiB of junk, 138 / 352 ≥ 30%.
const BLOCKER_BYTES: u64 = 128 * MIB;
/// The file of a junk subfolder.
const JUNK_BYTES: u64 = 2 * MIB;
/// First file of a leaf that goes to its junk subfolder.
const JUNK_FROM: u64 = 50;
/// A small password database.
const KDBX_BYTES: u64 = 2048;
/// Worker threads of the generator.
const THREADS: u64 = 8;

/// A file to create: name and size; `write` when the bytes are written.
struct FileSpec {
    /// Junk subfolder of the leaf that holds the file.
    sub: Option<&'static str>,
    name: String,
    size: u64,
    write: bool,
}

/// Shape of a generated tree.
#[derive(Debug, Clone, Copy)]
pub struct TreeShape {
    pub files: u64,
    leaves: u64,
    levels: u32,
}

impl TreeShape {
    pub fn new(files: u64) -> Self {
        let leaves = files.div_ceil(FILES_PER_DIR).max(1);
        let mut levels = 1;
        while FAN_OUT.pow(levels) < leaves {
            levels += 1;
        }
        Self {
            files,
            leaves,
            levels,
        }
    }

    /// Folders at all levels plus the junk subfolders.
    pub fn dirs(&self) -> u64 {
        let tree: u64 = (1..=self.levels)
            .map(|l| self.leaves.div_ceil(FAN_OUT.pow(self.levels - l)))
            .sum();
        let junk = (0..self.leaves)
            .filter(|k| junk_dir(*k).is_some() && self.leaf_count(*k) > JUNK_FROM)
            .count();
        tree + junk as u64
    }

    /// Entries a full walk visits: every folder and every file.
    pub fn entries(&self) -> u64 {
        self.files + self.dirs() + self.blockers()
    }

    /// Bytes of all files.
    pub fn bytes(&self) -> u64 {
        let leaves: u64 = (0..self.leaves)
            .flat_map(|k| self.leaf_files(k))
            .map(|f| f.size)
            .sum();
        leaves + self.blockers() * BLOCKER_BYTES
    }

    /// First-level folders, each with a blocker program.
    fn blockers(&self) -> u64 {
        if self.levels < 2 {
            0
        } else {
            self.leaves.div_ceil(FAN_OUT.pow(self.levels - 1))
        }
    }

    /// Path of leaf `k` below `root`: its digits in base [`FAN_OUT`].
    fn leaf_path(&self, root: &Path, k: u64) -> PathBuf {
        let mut path = root.to_path_buf();
        for l in (0..self.levels).rev() {
            path.push(format!("d{}", (k / FAN_OUT.pow(l)) % FAN_OUT));
        }
        path
    }

    /// Number of files of leaf `k`: [`FILES_PER_DIR`], the last leaf takes the rest.
    fn leaf_count(&self, k: u64) -> u64 {
        FILES_PER_DIR.min(self.files - k * FILES_PER_DIR)
    }

    /// Files of leaf `k`.
    fn leaf_files(&self, k: u64) -> Vec<FileSpec> {
        (0..self.leaf_count(k))
            .map(|f| {
                let ext = EXTENSIONS[((f + k) % EXTENSIONS.len() as u64) as usize];
                let sub = junk_dir(k).filter(|_| f >= JUNK_FROM);
                let spec = |ext: &str, size, write| FileSpec {
                    sub,
                    name: format!("f{f:03}.{ext}"),
                    size,
                    write,
                };
                let r = k % 1000;
                match f {
                    0 if k.is_multiple_of(100) => spec("docx", DOC_BYTES, false),
                    JUNK_FROM if sub.is_some() => spec("dat", JUNK_BYTES, false),
                    0 if r == 905 => spec("kdbx", KDBX_BYTES, true),
                    1 if r == 500 => spec("exe", BINARY_LEAF_BYTES, false),
                    _ => spec(ext, 0, false),
                }
            })
            .collect()
    }

    /// Contents of the marker file that describes this tree.
    fn marker(&self) -> String {
        format!(
            "savekeeper sk-heuristics usr bench tree\ngenerator={GENERATOR_VERSION}\n\
             files={}\nfiles_per_dir={FILES_PER_DIR}\nfan_out={FAN_OUT}\nlevels={}\n",
            self.files, self.levels
        )
    }
}

/// The junk subfolder that holds the files of leaf `k`, if any.
fn junk_dir(k: u64) -> Option<&'static str> {
    match k % 1000 {
        300 => Some("cache"),
        700 => Some("logs"),
        _ => None,
    }
}

/// Creates a new file of `size` bytes; fails if it already exists (never
/// overwrites). Only `write` files get data; the others are extended.
fn create(dir: &Path, f: &FileSpec) -> io::Result<()> {
    let path = match f.sub {
        Some(sub) => dir.join(sub).join(&f.name),
        None => dir.join(&f.name),
    };
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    if f.write {
        let size = usize::try_from(f.size).map_err(io::Error::other)?;
        file.write_all(&vec![b'k'; size])
    } else if f.size > 0 {
        file.set_len(f.size)
    } else {
        Ok(())
    }
}

/// Creates the tree below the empty folder `root`.
fn generate(shape: &TreeShape, root: &Path) -> io::Result<()> {
    let chunk = shape.leaves.div_ceil(THREADS);
    std::thread::scope(|s| {
        let workers: Vec<_> = (0..THREADS)
            .map(|t| {
                s.spawn(move || -> io::Result<()> {
                    for k in t * chunk..((t + 1) * chunk).min(shape.leaves) {
                        let dir = shape.leaf_path(root, k);
                        match junk_dir(k).filter(|_| shape.leaf_count(k) > JUNK_FROM) {
                            Some(junk) => fs::create_dir_all(dir.join(junk))?,
                            None => fs::create_dir_all(&dir)?,
                        }
                        for f in shape.leaf_files(k) {
                            create(&dir, &f)?;
                        }
                    }
                    Ok(())
                })
            })
            .collect();
        workers.into_iter().try_for_each(|w| {
            w.join()
                .map_err(|_| io::Error::other("generator panicked"))?
        })
    })?;
    for d in 0..shape.blockers() {
        let blocker = FileSpec {
            sub: None,
            name: "bin.exe".to_owned(),
            size: BLOCKER_BYTES,
            write: false,
        };
        create(&root.join(format!("d{d}")), &blocker)?;
    }
    Ok(())
}

/// The tree of `shape` in `base`: reused when its marker matches, generated
/// when neither the tree nor its marker exists. Returns the tree's root and
/// the generation time (`None` when reused). Never deletes or overwrites.
pub fn prepare_tree(base: &Path, shape: &TreeShape) -> Result<(PathBuf, Option<Duration>), String> {
    let root = base.join(format!("usr-{}", shape.files));
    let marker = base.join(format!("usr-{}.marker", shape.files));
    let fix = format!(
        "nothing was changed; delete {} and {} yourself to regenerate the tree, \
         or set SK_BENCH_DIR to another folder",
        root.display(),
        marker.display()
    );
    let root_exists = exists(&root)?;
    let marker_text = match fs::read_to_string(&marker) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("cannot read {}: {e}", marker.display())),
    };
    match (root_exists, marker_text) {
        (true, Some(text)) if text == shape.marker() => {
            if root.is_dir() {
                Ok((root, None))
            } else {
                Err(format!("{} is not a folder; {fix}", root.display()))
            }
        }
        (true, Some(_)) => Err(format!(
            "{} describes a different tree than requested; {fix}",
            marker.display()
        )),
        (true, None) => Err(format!(
            "{} exists but has no marker {} (an interrupted generation or a foreign \
             folder); {fix}",
            root.display(),
            marker.display()
        )),
        (false, Some(_)) => Err(format!(
            "marker {} exists without its tree {}; {fix}",
            marker.display(),
            root.display()
        )),
        (false, None) => {
            let start = Instant::now();
            let io_err = |what: &str, e: io::Error| format!("{what}: {e}");
            fs::create_dir_all(base).map_err(|e| io_err("cannot create SK_BENCH_DIR", e))?;
            fs::create_dir(&root).map_err(|e| io_err("cannot create the tree folder", e))?;
            generate(shape, &root).map_err(|e| io_err("tree generation failed", e))?;
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&marker)
                .and_then(|mut f| f.write_all(shape.marker().as_bytes()))
                .map_err(|e| io_err("cannot write the marker", e))?;
            Ok((root, Some(start.elapsed())))
        }
    }
}

/// Whether anything (also a link) exists at `path`.
fn exists(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("cannot inspect {}: {e}", path.display())),
    }
}

/// Uppercase drive letter of `C:\...`, `\\?\C:\...` or `\\.\C:\...`.
pub fn drive_letter(path: &Path) -> Option<char> {
    match path.components().next()? {
        Component::Prefix(p) => match p.kind() {
            Prefix::Disk(d) | Prefix::VerbatimDisk(d) => Some(char::from(d).to_ascii_uppercase()),
            Prefix::DeviceNS(name) => {
                let name = name.to_str()?;
                let mut chars = name.chars();
                match (chars.next(), chars.next(), chars.next()) {
                    (Some(d), Some(':'), None) if d.is_ascii_alphabetic() => {
                        Some(d.to_ascii_uppercase())
                    }
                    _ => None,
                }
            }
            _ => None,
        },
        _ => None,
    }
}
