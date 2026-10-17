//! Tests of H-JUNK over the subfolders of H-USR findings (T-07-13, SPEC-07
//! §4.5): junk subfolders of at least 1 MiB inside clusters and standard
//! folders go into their `exclude`, without findings of their own.

use sk_core::env::KnownFolder;
use sk_core::model::{Category, Target};
use sk_scan::MemFs;

use super::tests::{add, cluster, data, exclude_of, found, rel, Case, MIB};
use super::*;

/// A cluster `Data/Work` with junk of every kind inside.
fn work_tree() -> MemFs {
    let work = data().join("Work");
    let mut fs = MemFs::new();
    add(&mut fs, &work.join("report.docx"), 30 * MIB);
    add(&mut fs, &work.join("logs").join("app.log"), 2 * MIB);
    // Not entered: only the topmost junk subfolder is excluded.
    add(
        &mut fs,
        &work.join("logs").join("cache").join("x.bin"),
        2 * MIB,
    );
    add(&mut fs, &work.join("cache").join("blob.bin"), 3 * MIB);
    add(
        &mut fs,
        &work.join("Sub").join("Temp").join("x.tmp"),
        2 * MIB,
    );
    // Small junk stays in the finding.
    add(&mut fs, &work.join("tiny").join("logs").join("a.log"), 1024);
    // A global exclusion: neither walked nor excluded.
    add(
        &mut fs,
        &work.join("node_modules").join("pkg").join("index.js"),
        5 * MIB,
    );
    fs
}

#[test]
fn junk_subfolders_of_a_cluster_are_excluded() {
    let out = Case::new().run(&work_tree());
    assert_eq!(found(&out), [cluster("fake/Data/Work")]);
    assert_eq!(exclude_of(&out.findings[0]), ["cache", "logs", "Sub/Temp"]);
    // Excluded junk is not claimed, and the aggregates keep it.
    assert_eq!(out.claimed, [data().join("Work")]);
    assert_eq!(out.findings[0].evidence[0].message_args["files"], "6");
}

#[test]
fn junk_subfolders_of_a_standard_folder_are_excluded() {
    let case = Case::new();
    let documents = case
        .env
        .known_folder(KnownFolder::Documents)
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let mut fs = MemFs::new();
    add(&mut fs, &documents.join("Novel").join("a.docx"), 30 * MIB);
    add(&mut fs, &documents.join("logs").join("x.log"), 2 * MIB);
    add(
        &mut fs,
        &documents
            .join("My Games")
            .join("Game")
            .join("Cache")
            .join("c.bin"),
        3 * MIB,
    );
    add(
        &mut fs,
        &documents.join("Code").join("node_modules").join("a.js"),
        5 * MIB,
    );
    let out = case.run(&fs);
    assert_eq!(out.findings.len(), 1);
    assert_eq!(rel(&out.findings[0]), "fake/Users/user/Documents");
    assert_eq!(out.findings[0].evidence[0].message_key, REASON_USER_FOLDER);
    assert_eq!(
        exclude_of(&out.findings[0]),
        ["logs", "My Games/Game/Cache"]
    );

    // Claimed by a rule: H-USR leaves it alone.
    let mut case = Case::new();
    case.claimed.insert(documents);
    assert!(case.run(&fs).findings.is_empty());
}

#[test]
fn claimed_parts_come_first_and_are_not_repeated() {
    let mut case = Case::new();
    case.claimed.insert(data().join("Work").join("cache"));
    let out = case.run(&work_tree());
    assert_eq!(found(&out), [cluster("fake/Data/Work")]);
    assert_eq!(exclude_of(&out.findings[0]), ["cache", "logs", "Sub/Temp"]);
}

#[test]
fn valuable_files_in_excluded_junk_get_own_findings() {
    let mut fs = work_tree();
    let key = data().join("Work").join("logs").join("vault.kdbx");
    add(&mut fs, &key, 1024);
    let other = data().join("Work").join("keys.kdbx");
    add(&mut fs, &other, 1024);
    let out = Case::new().run(&fs);
    assert_eq!(out.findings.len(), 2);
    assert_eq!(rel(&out.findings[0]), "fake/Data/Work");
    assert_eq!(exclude_of(&out.findings[0]), ["cache", "logs", "Sub/Temp"]);
    let file = &out.findings[1];
    assert!(matches!(&file.target, Target::File { resolved, .. } if *resolved == key));
    assert_eq!(file.category, Category::Credentials);
    assert_eq!(file.evidence[0].message_key, "heuristic.usr.valuable_file");
}

#[test]
fn junk_cluster_gets_no_subfolder_excludes() {
    let mut fs = MemFs::new();
    let temp = data().join("Temp");
    add(&mut fs, &temp.join("draft.docx"), 30 * MIB);
    add(&mut fs, &temp.join("logs").join("x.log"), 2 * MIB);
    let out = Case::new().run(&fs);
    assert_eq!(out.findings.len(), 1);
    assert_eq!(out.findings[0].category, Category::Cache);
    assert!(exclude_of(&out.findings[0]).is_empty());
}

#[test]
fn no_junk_excludes_when_h_junk_is_off() {
    let mut case = Case::new();
    case.cfg.enabled.junk = false;
    let out = case.run(&work_tree());
    assert_eq!(found(&out), [cluster("fake/Data/Work")]);
    assert!(exclude_of(&out.findings[0]).is_empty());
}
