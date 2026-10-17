//! Tests of H-USR (T-07-08): the cases of SPEC-07 §6 for the clustering of
//! §4.3.3 and helpers for the other H-USR tests. Everything runs on `MemFs`
//! with a fake environment without drives: only the roots of a test are walked.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sk_core::config::HeuristicsConfig;
use sk_core::env::Environment;
use sk_core::fs::FsScanner;
use sk_core::model::{Category, Finding, IssueSeverity, Target};
use sk_core::path::PathSet;
use sk_core::CancellationToken;
use sk_scan::{ExcludeSet, MemFs};

use super::*;
use crate::collector::HeuristicsCollector;
use crate::walk::RootsWalkCache;

pub(super) const MIB: u64 = 1024 * 1024;

pub(super) fn root() -> &'static Path {
    Path::new(if cfg!(windows) { r"C:\fake" } else { "/fake" })
}

/// The drive of [`root`]: `C:\` or `/`.
pub(super) fn drive() -> PathBuf {
    root()
        .ancestors()
        .last()
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

/// A folder outside the known folders of the fake environment.
pub(super) fn data() -> PathBuf {
    root().join("Data")
}

pub(super) fn add(fs: &mut MemFs, path: &Path, size: u64) {
    fs.add_file(
        path.to_str().unwrap_or_default(),
        size,
        "2026-01-01T00:00:00Z",
        None,
    );
}

/// What H-USR needs besides the scanner.
pub(super) struct Case {
    pub env: Environment,
    pub cfg: HeuristicsConfig,
    pub claimed: PathSet,
    pub cancel: CancellationToken,
    pub roots: Vec<PathBuf>,
}

impl Case {
    /// The fake environment without drives; [`root`] is walked. (`MemFs`
    /// cannot hold `/`, so the drive itself is walked only on Windows.)
    pub(super) fn new() -> Self {
        let mut env = Environment::fake(root());
        env.drives.clear();
        Self {
            env,
            cfg: HeuristicsConfig::default(),
            claimed: PathSet::new(),
            cancel: CancellationToken::new(),
            roots: vec![root().to_path_buf()],
        }
    }

    pub(super) fn run(&self, fs: &dyn FsScanner) -> HeuristicOutput {
        let excludes = Arc::new(ExcludeSet::builtin(&self.env));
        let cache = RootsWalkCache::new(self.roots.clone(), 32);
        let hctx = HeuristicContext {
            env: &self.env,
            scanner: fs,
            excludes: &excludes,
            claimed: &self.claimed,
            prior: &[],
            cfg: &self.cfg,
            cancel: &self.cancel,
            progress: &|_, _| {},
            roots_walk: &cache,
        };
        HUsr::new().run(&hctx)
    }
}

/// Path of the finding's target relative to [`drive`], with `/`.
pub(super) fn rel(f: &Finding) -> String {
    let path = match &f.target {
        Target::FileSet { resolved, .. } | Target::File { resolved, .. } => resolved.as_path(),
        other => panic!("{other:?}"),
    };
    let parts: Vec<String> = path
        .strip_prefix(drive())
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts.join("/")
}

/// Path, category and reason of every finding.
pub(super) fn found(out: &HeuristicOutput) -> Vec<(String, Category, String)> {
    out.findings
        .iter()
        .map(|f| (rel(f), f.category, f.evidence[0].message_key.clone()))
        .collect()
}

pub(super) fn cluster(path: &str) -> (String, Category, String) {
    (
        path.to_owned(),
        Category::UserFiles,
        REASON_CLUSTER.to_owned(),
    )
}

pub(super) fn exclude_of(f: &Finding) -> Vec<String> {
    match &f.target {
        Target::FileSet { exclude, .. } => exclude.clone(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn work_tree_is_one_cluster() {
    let mut fs = MemFs::new();
    let work = data().join("Work");
    for client in ["ClientA", "ClientB"] {
        for doc in ["contract.docx", "offer.docx", "invoice.docx"] {
            add(&mut fs, &work.join(client).join(doc), 5 * MIB);
        }
    }
    let out = Case::new().run(&fs);
    assert_eq!(found(&out), [cluster("fake/Data/Work")]);
    assert_eq!(out.claimed, [work]);
    assert!(out.issues.is_empty(), "{:?}", out.issues);
    assert!(out.unknown.is_empty());
    let finding = &out.findings[0];
    assert_eq!(finding.title, "Work");
    assert_eq!(finding.app, None);
    assert_eq!(finding.evidence[0].confidence, 0.7);
    assert_eq!(
        finding.evidence[0].source,
        sk_core::model::EvidenceSource::Heuristic {
            heuristic_id: "h-usr".to_owned()
        }
    );
    let args = &finding.evidence[0].message_args;
    assert_eq!(args["files"], "6");
    assert_eq!(args["bytes"], (30 * MIB).to_string());
    assert_eq!(args["group1"], "document");
    assert_eq!(args["group1_files"], "6");
    assert!(!args.contains_key("group2"));
}

#[test]
fn dominant_child_takes_the_place() {
    let mut fs = MemFs::new();
    let photos = data().join("Photos");
    for n in 0..3 {
        add(
            &mut fs,
            &photos.join("2024").join(format!("IMG_{n}.cr2")),
            20 * MIB,
        );
    }
    add(&mut fs, &photos.join("notes.txt"), MIB);
    add(&mut fs, &photos.join("misc").join("todo.txt"), MIB);
    let out = Case::new().run(&fs);
    // Photos: 62, its child 2024: 60 ≥ 0.9 × 62; Data holds only Photos.
    assert_eq!(found(&out), [cluster("fake/Data/Photos/2024")]);
    assert_eq!(
        out.findings[0].evidence[0].message_args["group1"],
        "photo_raw"
    );

    // Two children of the same weight: none dominates.
    add(&mut fs, &photos.join("2025").join("IMG_9.cr2"), 60 * MIB);
    add(&mut fs, &photos.join("2025").join("IMG_8.cr2"), 10 * MIB);
    let out = Case::new().run(&fs);
    assert_eq!(found(&out), [cluster("fake/Data/Photos")]);
}

#[test]
fn dominant_child_does_not_leave_a_cluster_behind() {
    let mut fs = MemFs::new();
    let media = root().join("Media");
    for n in 0..8 {
        add(
            &mut fs,
            &media.join("Photos").join(format!("IMG_{n}.cr2")),
            50 * MIB,
        );
    }
    add(&mut fs, &media.join("Docs").join("thesis.docx"), 30 * MIB);
    // Photos: 400 ≥ 0.9 × 430, but Docs (30) would be a cluster of its own.
    let out = Case::new().run(&fs);
    assert_eq!(found(&out), [cluster("fake/Media")]);

    // A rest below `min_weight` is left behind.
    let mut case = Case::new();
    case.cfg.usr.min_weight = 31;
    assert_eq!(found(&case.run(&fs)), [cluster("fake/Media/Photos")]);
}

#[test]
fn drive_root_is_never_a_cluster() {
    assert!(is_drive_root(Path::new("/")));
    assert!(!is_drive_root(&data()));
    if cfg!(windows) {
        assert!(is_drive_root(Path::new(r"C:\")));
        assert!(is_drive_root(Path::new(r"\\?\D:\")));
        let mut fs = MemFs::new();
        let drive = drive();
        add(&mut fs, &drive.join("report.docx"), 40 * MIB);
        add(&mut fs, &drive.join("Work").join("a.docx"), 30 * MIB);
        let mut case = Case::new();
        case.roots = vec![drive];
        // The root's own files are not a cluster; its child is.
        assert_eq!(found(&case.run(&fs)), [cluster("Work")]);
    }

    // A root given by the user that is not a drive root may be a cluster.
    let mut case = Case::new();
    let mut fs = MemFs::new();
    add(&mut fs, &data().join("a.docx"), 15 * MIB);
    add(&mut fs, &data().join("b.docx"), 15 * MIB);
    case.roots = vec![data()];
    assert_eq!(found(&case.run(&fs)), [cluster("fake/Data")]);
}

#[test]
fn max_candidates_gives_an_issue() {
    let mut fs = MemFs::new();
    // Children of a folder holding known folders: each is looked at alone.
    for (name, mib) in [("A", 30), ("B", 50), ("C", 40), ("D", 25)] {
        add(&mut fs, &root().join(name).join("doc.pdf"), mib * MIB);
        add(&mut fs, &root().join(name).join("more.pdf"), mib * MIB);
    }
    let mut case = Case::new();
    case.cfg.usr.max_candidates = 2;
    let out = case.run(&fs);
    // The heaviest two, in walk order.
    assert_eq!(found(&out), [cluster("fake/B"), cluster("fake/C")]);
    let [issue] = out.issues.as_slice() else {
        panic!("{:?}", out.issues)
    };
    assert_eq!(issue.message_key, "heuristic.usr_truncated");
    assert_eq!(issue.severity, IssueSeverity::Warning);
    assert_eq!(issue.source, "heuristics");
    assert_eq!(issue.message_args["count"], "2");
    assert_eq!(issue.message_args["limit"], "2");

    case.cfg.usr.max_candidates = 4;
    let out = case.run(&fs);
    assert_eq!(out.findings.len(), 4);
    assert!(out.issues.is_empty());
}

#[test]
fn default_collector_runs_h_usr() {
    let collector = HeuristicsCollector::new(HeuristicsConfig::default());
    assert_eq!(collector.ids(), ["h-git", "h-web", "h-unk", "h-usr"]);
}
