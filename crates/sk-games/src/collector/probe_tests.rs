//! Segment-prefix pruning of the include probe (T-05-14).

use sk_core::env::Environment;
use sk_core::registry::MemRegistry;
use sk_core::CancellationToken;
use sk_scan::MemFs;

use super::*;
use crate::manifest::{Manifest, ManifestSource};

fn root() -> PathBuf {
    PathBuf::from(if cfg!(windows) { r"C:\fake" } else { "/fake" })
}

/// `rel` (`/`-separated) under [`root`].
fn at(rel: &str) -> PathBuf {
    rel.split('/').fold(root(), |p, c| p.join(c))
}

fn add_file(fs: &mut MemFs, rel: &str) {
    fs.add_file(&at(rel).to_string_lossy(), 1, "-1d", None);
}

fn globs(include: &[&str]) -> Vec<String> {
    include.iter().map(|g| (*g).to_owned()).collect()
}

fn may_contain(include: &[&str], rel: &str) -> bool {
    let rel: PathBuf = rel.split('/').collect();
    Prefixes::new(&globs(include)).may_contain(&rel)
}

/// Runs [`probe_path`] on `rel` with `include`; the match and the folders
/// the probe listed (the root included).
fn probe(fs: &MemFs, rel: &str, include: &[&str]) -> (bool, u64) {
    let manifest =
        Manifest::parse(b"".as_slice(), ManifestSource::Cache).unwrap_or_else(|e| panic!("{e}"));
    let env = Environment::fake(&root());
    let registry = MemRegistry::new();
    let cancel = CancellationToken::new();
    let scan = Scan {
        manifest: &manifest,
        version: "test".to_owned(),
        env: &env,
        fs,
        registry: &registry,
        cancel: &cancel,
        max_depth: 32,
    };
    let before = fs.calls().read_dir;
    let found = probe_path(&scan, &at(rel), &globs(include)).is_some();
    (found, fs.calls().read_dir - before)
}

#[test]
fn folders_must_match_the_leading_segments() {
    let user = [
        "[User]/AppData/Roaming/Nitroplus",
        "[User]/AppData/Roaming/Nitroplus/**",
    ];
    assert!(may_contain(&user, "U"));
    assert!(may_contain(&user, "u/appdata/ROAMING"));
    assert!(may_contain(&user, "U/AppData/Roaming/Nitroplus"));
    assert!(may_contain(&user, "U/AppData/Roaming/Nitroplus/deep/er"));
    assert!(!may_contain(&user, "00eag"));
    assert!(!may_contain(&user, "Public"));
    assert!(!may_contain(&user, "U/Documents"));

    let saves = ["*/saves", "*/saves/**"];
    assert!(may_contain(&saves, "profile1"));
    assert!(may_contain(&saves, "profile1/Saves"));
    assert!(!may_contain(&saves, "profile1/logs"));

    // A folder as deep as a glob without `**` cannot hold a match of it.
    assert!(!may_contain(&["*.ess"], "Old.ess"));
    assert!(!may_contain(&["a/*.sav"], "a/b"));
    assert!(may_contain(&["a/*.sav"], "a"));

    // `**` (or a segment with it) matches anything below.
    assert!(may_contain(&["**/Saved", "**/Saved/**"], "a/b/c"));
    assert!(may_contain(&["Game/**/Saved"], "Game/x/y"));
    assert!(!may_contain(&["Game/**/Saved"], "Other"));
    assert!(may_contain(&["Game/x**y/z"], "Game/anything/else"));

    // Escaped braces and a class split by `/` (not judged alone: no pruning).
    assert!(may_contain(&["[{]id[}]/save"], "{id}"));
    assert!(!may_contain(&["[{]id[}]/save"], "id"));
    assert!(may_contain(&["a[/]b/c"], "x/y/z"));

    // Any of several globs is enough.
    assert!(may_contain(&["One/*.sav", "Two/*.sav"], "two"));
}

#[test]
fn probe_does_not_enter_folders_that_cannot_match() {
    let mut fs = MemFs::new();
    for user in ["00eag", "Public", "Default"] {
        for sub in ["AppData/Roaming/Other", "Documents/a/b", "Desktop/c/d"] {
            add_file(&mut fs, &format!("Users/{user}/{sub}/file.txt"));
        }
    }
    let user = [
        "[User]/AppData/Roaming/Nitroplus",
        "[User]/AppData/Roaming/Nitroplus/**",
    ];
    // Only `Users` is listed: no user folder is one character long.
    assert_eq!(probe(&fs, "Users", &user), (false, 1));

    add_file(&mut fs, "Users/U/AppData/Roaming/Nitroplus/save.dat");
    // `Users`, `U`, `AppData`, `Roaming`, `Nitroplus`.
    assert_eq!(probe(&fs, "Users", &user), (true, 5));
}

#[test]
fn probe_still_descends_for_folder_globs() {
    let mut fs = MemFs::new();
    add_file(&mut fs, "Game/a/b/Saved/sub/x.sav");
    add_file(&mut fs, "Game/a/notes.txt");
    assert_eq!(probe(&fs, "Game", &["**/Saved", "**/Saved/**"]), (true, 5));
    let mut fs = MemFs::new();
    add_file(&mut fs, "Studio/profile1/logs/l/x.log");
    add_file(&mut fs, "Studio/profile1/saves/auto/slot.sav");
    // `Studio`, `profile1`, `saves`, `auto`; `logs` is skipped.
    assert_eq!(probe(&fs, "Studio", &["*/saves", "*/saves/**"]), (true, 4));
}
