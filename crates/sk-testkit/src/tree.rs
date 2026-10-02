//! Content hashes of a folder tree, for comparing trees in backup tests (SPEC-12 §4.2).

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// Relative path (with `/` separators) → blake3 hex of every file under `dir`.
///
/// Folders appear only through their files. A symbolic link is not followed:
/// its value is `symlink:<target>`. Panics on I/O errors.
pub fn tree_hash(dir: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    visit(dir, "", &mut out);
    out
}

fn visit(dir: &Path, prefix: &str, out: &mut BTreeMap<String, String>) {
    let entries =
        fs::read_dir(dir).unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()));
    for entry in entries {
        let entry = entry.unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()));
        let path = entry.path();
        let name = format!("{prefix}{}", entry.file_name().to_string_lossy());
        let file_type = entry
            .file_type()
            .unwrap_or_else(|e| panic!("cannot stat {}: {e}", path.display()));
        if file_type.is_symlink() {
            let target = fs::read_link(&path)
                .unwrap_or_else(|e| panic!("cannot read link {}: {e}", path.display()));
            out.insert(name, format!("symlink:{}", target.to_string_lossy()));
        } else if file_type.is_dir() {
            visit(&path, &format!("{name}/"), out);
        } else {
            out.insert(name, hash_file(&path));
        }
    }
}

fn hash_file(path: &Path) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher
        .update_reader(
            fs::File::open(path).unwrap_or_else(|e| panic!("cannot open {}: {e}", path.display())),
        )
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    hasher.finalize().to_hex().to_string()
}
