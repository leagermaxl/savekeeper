//! Tests of H-JUNK over the subfolders of H-UNK findings (T-07-13, SPEC-07
//! §4.5): junk direct children of at least 1 MiB among `top_children` go into
//! the `exclude` of the candidate, without findings of their own.

use sk_core::env::KnownFolder;
use sk_core::model::Category;
use sk_scan::MemFs;

use super::tests::{abs, at, excludes_of, file, one, Setup, MIB};

const MTIME: &str = "2026-01-01T00:00:00Z";

/// `{APPDATA}\Tool` with settings, data and junk children.
fn tool() -> MemFs {
    let mut fs = MemFs::new();
    let f = KnownFolder::AppData;
    file(&mut fs, f, "Tool/settings.json", 2048, MTIME);
    file(&mut fs, f, "Tool/data/notes.txt", 20 * MIB, MTIME);
    file(&mut fs, f, "Tool/logs/app.log", 2 * MIB, MTIME);
    file(&mut fs, f, "Tool/Cache/blob.bin", 3 * MIB, MTIME);
    file(&mut fs, f, "Tool/Temp/x.tmp", 2 * MIB, MTIME);
    // Small junk stays in the finding.
    file(&mut fs, f, "Tool/log/a.log", 1024, MTIME);
    // Grandchildren are not looked at.
    file(&mut fs, f, "Tool/data/cache/c.bin", 2 * MIB, MTIME);
    // A global exclusion: neither summarized nor excluded.
    file(&mut fs, f, "Tool/node_modules/pkg/index.js", 5 * MIB, MTIME);
    fs
}

fn sorted(globs: &[String]) -> Vec<String> {
    let mut globs = globs.to_vec();
    globs.sort();
    globs
}

#[test]
fn junk_children_of_a_candidate_are_excluded() {
    let out = Setup::new(tool()).run();
    let tool = abs(KnownFolder::AppData, "Tool");
    let finding = one(&out, &tool);
    assert!(!matches!(
        finding.category,
        Category::Cache | Category::Reinstallable
    ));
    assert_eq!(sorted(excludes_of(finding)), ["Cache", "Temp", "logs"]);
    // No findings for the junk, and it is not claimed on its own.
    assert_eq!(out.findings.len(), 1);
    for junk in ["logs", "Cache", "Temp", "node_modules"] {
        let path = tool.join(junk);
        assert!(at(&out, &path).is_empty());
        assert!(!out.claimed.contains(&path));
    }
}

#[test]
fn claimed_children_are_not_repeated() {
    let mut setup = Setup::new(tool());
    setup
        .claimed
        .insert(abs(KnownFolder::AppData, "Tool/Cache"));
    let out = setup.run();
    let finding = one(&out, &abs(KnownFolder::AppData, "Tool"));
    assert_eq!(sorted(excludes_of(finding)), ["Cache", "Temp", "logs"]);
}

#[test]
fn junk_candidate_gets_no_child_excludes() {
    let mut fs = MemFs::new();
    file(
        &mut fs,
        KnownFolder::AppData,
        "Crashpad/reports/r.dmp",
        MIB,
        MTIME,
    );
    file(
        &mut fs,
        KnownFolder::AppData,
        "Crashpad/logs/x.log",
        2 * MIB,
        MTIME,
    );
    let out = Setup::new(fs).run();
    let finding = one(&out, &abs(KnownFolder::AppData, "Crashpad"));
    assert_eq!(finding.category, Category::Cache);
    assert!(excludes_of(finding).is_empty());
}

#[test]
fn no_junk_excludes_when_h_junk_is_off() {
    let mut setup = Setup::new(tool());
    setup.cfg.enabled.junk = false;
    let out = setup.run();
    let finding = one(&out, &abs(KnownFolder::AppData, "Tool"));
    assert!(excludes_of(finding).is_empty());
}
