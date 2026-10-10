//! Tests of H-USR (T-07-08): thresholds, claimed paths, standard user
//! folders, single valuable files, junk clusters, categories and tags, walk
//! issues, cancellation and determinism.

use sk_core::env::KnownFolder;
use sk_core::model::{Category, Sensitivity, Target};
use sk_scan::MemFs;

use super::tests::{add, cluster, data, exclude_of, found, rel, root, Case, MIB};
use super::*;

fn known(case: &Case, folder: KnownFolder) -> std::path::PathBuf {
    case.env
        .known_folder(folder)
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

#[test]
fn light_and_binary_heavy_folders_are_not_clusters() {
    let mut fs = MemFs::new();
    // 19 MiB of documents: below `min_weight`.
    add(&mut fs, &root().join("Light").join("a.docx"), 19 * MIB);
    // 40 MiB of documents and 20 MiB of programs: weight 20, but a third
    // of the bytes are programs; the documents alone are a cluster.
    let mixed = root().join("Mixed");
    add(&mut fs, &mixed.join("docs").join("a.pdf"), 40 * MIB);
    add(&mut fs, &mixed.join("tools").join("setup.exe"), 20 * MIB);
    let out = Case::new().run(&fs);
    assert_eq!(found(&out), [cluster("fake/Mixed/docs")]);

    let mut case = Case::new();
    case.cfg.usr.min_weight = 19;
    let out = case.run(&fs);
    assert_eq!(
        found(&out),
        [cluster("fake/Light"), cluster("fake/Mixed/docs")]
    );
}

#[test]
fn claimed_folders_are_left_out() {
    let mut fs = MemFs::new();
    let work = data().join("Work");
    for (client, mib) in [("ClientA", 15), ("ClientB", 25), ("ClientC", 15)] {
        add(&mut fs, &work.join(client).join("a.docx"), mib * MIB);
    }
    add(
        &mut fs,
        &work.join("ClientA").join("Repo").join("x.docx"),
        MIB,
    );
    let mut case = Case::new();
    case.claimed.insert(work.join("ClientA"));
    let out = case.run(&fs);
    assert_eq!(found(&out), [cluster("fake/Data/Work")]);
    assert_eq!(exclude_of(&out.findings[0]), ["ClientA"]);
    assert_eq!(out.findings[0].evidence[0].message_args["files"], "2");

    // Without ClientA and ClientC, ClientB is all that is left.
    case.claimed.insert(work.join("ClientC"));
    let out = case.run(&fs);
    assert_eq!(found(&out), [cluster("fake/Data/Work/ClientB")]);
    assert!(exclude_of(&out.findings[0]).is_empty());

    case.claimed.insert(data());
    assert!(case.run(&fs).findings.is_empty());
}

#[test]
fn standard_folders_give_one_finding_each() {
    let case = Case::new();
    let documents = known(&case, KnownFolder::Documents);
    let downloads = known(&case, KnownFolder::Downloads);
    let mut fs = MemFs::new();
    for project in ["Novel", "Taxes"] {
        add(&mut fs, &documents.join(project).join("a.docx"), 30 * MIB);
    }
    add(&mut fs, &downloads.join("setup.exe"), 30 * MIB);
    add(
        &mut fs,
        &known(&case, KnownFolder::Music).join("desktop.ini"),
        0,
    );
    let out = case.run(&fs);
    let user_folder = |path: &str| {
        (
            path.to_owned(),
            Category::UserFiles,
            REASON_USER_FOLDER.to_owned(),
        )
    };
    assert_eq!(
        found(&out),
        [
            user_folder("fake/Users/user/Documents"),
            user_folder("fake/Users/user/Downloads"),
            user_folder("fake/Users/user/Music"),
        ]
    );

    // Claimed by a rule: nothing is looked for inside.
    let mut case = Case::new();
    case.claimed.insert(documents.clone());
    case.claimed.insert(downloads);
    case.claimed.insert(known(&case, KnownFolder::Music));
    assert!(case.run(&fs).findings.is_empty());

    // Partly claimed: the rest, without the claimed part.
    let mut case = Case::new();
    case.claimed.insert(documents.join("Novel"));
    let out = case.run(&fs);
    assert_eq!(rel(&out.findings[0]), "fake/Users/user/Documents");
    assert_eq!(exclude_of(&out.findings[0]), ["Novel"]);
}

#[test]
fn folders_holding_known_folders_are_never_clusters() {
    let case = Case::new();
    let home = known(&case, KnownFolder::Home);
    let mut fs = MemFs::new();
    add(&mut fs, &home.join("big.pdf"), 40 * MIB);
    add(&mut fs, &home.join("Projects").join("plan.pdf"), 40 * MIB);
    let out = case.run(&fs);
    assert_eq!(found(&out), [cluster("fake/Users/user/Projects")]);
}

#[test]
fn valuable_files_outside_clusters_get_findings() {
    let mut fs = MemFs::new();
    let work = data().join("Work");
    add(&mut fs, &work.join("a.docx"), 30 * MIB);
    add(&mut fs, &work.join("keys").join("vault.kdbx"), 64 * 1024);
    add(&mut fs, &root().join("Keys").join("id.pem"), 2048);
    add(&mut fs, &root().join("Mail").join("old.pst"), MIB);
    let case = Case::new();
    let out = case.run(&fs);
    let file = |path: &str, category| {
        (
            path.to_owned(),
            category,
            "heuristic.usr.valuable_file".to_owned(),
        )
    };
    assert_eq!(
        found(&out),
        [
            cluster("fake/Data/Work"),
            file("fake/Keys/id.pem", Category::Credentials),
            file("fake/Mail/old.pst", Category::UserFiles),
        ]
    );
    let work = &out.findings[0];
    assert_eq!(work.sensitivity, Sensitivity::High);
    assert_eq!(work.tags, ["credentials-inside"]);
    let pem = &out.findings[1];
    assert!(
        matches!(pem.target, Target::File { .. }),
        "{:?}",
        pem.target
    );
    assert_eq!(pem.sensitivity, Sensitivity::High);
    assert_eq!(pem.title, "id.pem");
    assert_eq!(pem.evidence[0].message_args["group"], "credentials");
    assert_eq!(pem.evidence[0].message_args["bytes"], "2048");
    assert_eq!(out.findings[2].sensitivity, Sensitivity::None);
    assert_eq!(out.claimed.len(), 3);

    let mut case = Case::new();
    case.claimed.insert(root().join("Keys"));
    let out = case.run(&fs);
    assert_eq!(out.findings.len(), 2, "a claimed file has no finding");
}

#[test]
fn junk_clusters_are_findings_without_their_valuable_files() {
    let mut fs = MemFs::new();
    let logs = root().join("Tool").join("logs");
    add(&mut fs, &logs.join("today.txt"), 40 * MIB);
    add(&mut fs, &logs.join("keys").join("cert.pfx"), 4096);
    let out = Case::new().run(&fs);
    assert_eq!(
        found(&out),
        [
            (
                "fake/Tool/logs".to_owned(),
                Category::Cache,
                "heuristic.junk.cache_name".to_owned()
            ),
            (
                "fake/Tool/logs/keys/cert.pfx".to_owned(),
                Category::Credentials,
                "heuristic.usr.valuable_file".to_owned()
            ),
        ]
    );
    assert_eq!(exclude_of(&out.findings[0]), ["keys/cert.pfx"]);
    assert!(out.findings[0].tags.is_empty());
    assert_eq!(out.findings[0].sensitivity, Sensitivity::None);

    // A junk folder that is not a candidate gives no finding.
    let mut fs = MemFs::new();
    add(&mut fs, &root().join("Temp").join("a.txt"), 10 * MIB);
    assert!(Case::new().run(&fs).findings.is_empty());
}

#[test]
fn junk_is_found_on_the_way_down_and_never_entered() {
    let junk = |path: &str| {
        (
            path.to_owned(),
            Category::Cache,
            "heuristic.junk.cache_name".to_owned(),
        )
    };
    // Tool qualifies, its only child `cache` dominates: `cache` is junk and
    // its child `thumbs` is not a cluster of user files.
    let mut fs = MemFs::new();
    let thumbs = root().join("Tool").join("cache").join("thumbs");
    add(&mut fs, &thumbs.join("a.jpg"), 50 * MIB);
    let out = Case::new().run(&fs);
    assert_eq!(found(&out), [junk("fake/Tool/cache")]);

    // A junk folder looked at directly: one finding, nothing below it.
    let mut fs = MemFs::new();
    let temp = root().join("Temp");
    add(&mut fs, &temp.join("Novel").join("draft.docx"), 40 * MIB);
    add(&mut fs, &temp.join("Taxes").join("2025.docx"), 40 * MIB);
    let out = Case::new().run(&fs);
    assert_eq!(found(&out), [junk("fake/Temp")]);

    // Too light junk is skipped, also when its subfolders are heavy enough
    // (Temp weighs 40 − 30 = 10, and 3/7 of its bytes are programs).
    let mut fs = MemFs::new();
    add(&mut fs, &temp.join("Novel").join("draft.docx"), 40 * MIB);
    add(&mut fs, &temp.join("setup.exe"), 30 * MIB);
    assert!(Case::new().run(&fs).findings.is_empty());
}

#[test]
fn category_and_tags_come_from_the_groups() {
    let mut fs = MemFs::new();
    let code = root().join("Code");
    add(&mut fs, &code.join("main.rs"), 30 * MIB);
    add(&mut fs, &code.join("notes.md"), 5 * MIB);
    add(&mut fs, &code.join("logo.png"), 5 * MIB);
    add(&mut fs, &code.join("data.zip"), 5 * MIB);
    let cloud = root().join("Cloud");
    add(&mut fs, &cloud.join("a.docx"), 30 * MIB);
    fs.set_cloud_only(cloud.join("a.docx").to_str().unwrap_or_default());
    let out = Case::new().run(&fs);
    let code = &out.findings[1];
    assert_eq!(rel(code), "fake/Code");
    assert_eq!(code.category, Category::DevEnvironment);
    let args = &code.evidence[0].message_args;
    let groups: Vec<(&str, &str)> = (1..=4)
        .filter_map(|n| {
            let kind = args.get(&format!("group{n}"))?;
            Some((kind.as_str(), args[&format!("group{n}_files")].as_str()))
        })
        .collect();
    assert_eq!(
        groups,
        [("code", "1"), ("document", "1"), ("photo", "1")],
        "top three by weight: 15, 5, 3 (archive: 1)"
    );
    let cloud = &out.findings[0];
    assert_eq!(rel(cloud), "fake/Cloud");
    assert_eq!(cloud.tags, ["cloud-synced"]);
    assert_eq!(cloud.evidence[0].message_args["cloud_only_files"], "1");
}

#[test]
fn walk_issues_are_forwarded() {
    let mut fs = MemFs::new();
    add(&mut fs, &data(), 10);
    let mut case = Case::new();
    case.roots = vec![data()];
    let out = case.run(&fs);
    assert!(out.findings.is_empty());
    let [issue] = out.issues.as_slice() else {
        panic!("{:?}", out.issues)
    };
    assert_eq!(issue.message_key, "heuristic.read_failed");
}

#[test]
fn cancelled_scan_gives_no_clusters() {
    let mut fs = MemFs::new();
    add(&mut fs, &data().join("Work").join("a.docx"), 30 * MIB);
    let case = Case::new();
    case.cancel.cancel();
    let out = case.run(&fs);
    assert!(out.findings.is_empty());
    assert!(out.claimed.is_empty());
}

#[test]
fn two_runs_give_the_same_output() {
    let mut fs = MemFs::new();
    for (dir, file, mib) in [
        ("Work/ClientA", "a.docx", 12),
        ("Work/ClientB", "b.pdf", 12),
        ("Photos/2024", "x.cr2", 60),
        ("Keys", "k.ppk", 1),
        ("Old/logs", "log.txt", 30),
    ] {
        add(&mut fs, &root().join(dir).join(file), mib * MIB);
    }
    let case = Case::new();
    let first = case.run(&fs);
    assert_eq!(first.findings.len(), 4, "{:?}", found(&first));
    assert_eq!(case.run(&fs), first);
}
