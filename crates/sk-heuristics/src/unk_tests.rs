//! Tests of H-UNK (SPEC-07 §4.2, §6): vendor containers, partial coverage,
//! unknown folders, claims, cancellation. Shared helpers of the H-UNK tests.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sk_core::config::HeuristicsConfig;
use sk_core::env::{Environment, KnownFolder};
use sk_core::fs::FsScanner;
use sk_core::model::{Category, Finding, Target};
use sk_core::path::{eq_ci, PathSet};
use sk_core::CancellationToken;
use sk_scan::{ExcludeSet, MemFs};
use time::OffsetDateTime;

use super::*;
use crate::heuristic::{Heuristic, HeuristicContext, HeuristicOutput};

pub(super) const MIB: u64 = 1024 * 1024;

pub(super) fn env() -> Environment {
    Environment::fake(Path::new(if cfg!(windows) { r"C:\fake" } else { "/fake" }))
}

/// `rel` (with `/`) under a known folder of [`env`].
pub(super) fn abs(folder: KnownFolder, rel: &str) -> PathBuf {
    let base = env().known_folder(folder).unwrap().to_path_buf();
    rel.split('/').fold(base, |p, c| p.join(c))
}

pub(super) fn file(fs: &mut MemFs, folder: KnownFolder, rel: &str, size: u64, mtime: &str) {
    fs.add_file(abs(folder, rel).to_str().unwrap(), size, mtime, None);
}

/// Everything H-UNK needs, for one run.
pub(super) struct Setup {
    pub(super) env: Environment,
    pub(super) fs: Box<dyn FsScanner>,
    pub(super) claimed: PathSet,
    pub(super) cfg: HeuristicsConfig,
    pub(super) excludes: Arc<ExcludeSet>,
    pub(super) cancel: CancellationToken,
}

impl Setup {
    pub(super) fn new(fs: MemFs) -> Self {
        let env = env();
        Self {
            excludes: Arc::new(ExcludeSet::builtin(&env)),
            env,
            fs: Box::new(fs),
            claimed: PathSet::new(),
            cfg: HeuristicsConfig::default(),
            cancel: CancellationToken::new(),
        }
    }

    pub(super) fn run_with(&self, progress: &dyn Fn(u64, Option<&str>)) -> HeuristicOutput {
        let hctx = HeuristicContext {
            env: &self.env,
            scanner: self.fs.as_ref(),
            excludes: &self.excludes,
            claimed: &self.claimed,
            prior: &[],
            cfg: &self.cfg,
            cancel: &self.cancel,
            progress,
        };
        HUnk::at(OffsetDateTime::now_utc()).run(&hctx)
    }

    pub(super) fn run(&self) -> HeuristicOutput {
        self.run_with(&|_, _| {})
    }
}

/// The findings whose root is `path`.
pub(super) fn at<'a>(out: &'a HeuristicOutput, path: &Path) -> Vec<&'a Finding> {
    out.findings
        .iter()
        .filter(|f| match &f.target {
            Target::FileSet { resolved, .. } | Target::File { resolved, .. } => {
                eq_ci(resolved, path)
            }
            _ => false,
        })
        .collect()
}

/// The only finding at `path`.
pub(super) fn one<'a>(out: &'a HeuristicOutput, path: &Path) -> &'a Finding {
    match at(out, path).as_slice() {
        [f] => f,
        other => panic!("{} findings at {}: {other:?}", other.len(), path.display()),
    }
}

pub(super) fn excludes_of(f: &Finding) -> &[String] {
    match &f.target {
        Target::FileSet { exclude, .. } => exclude,
        _ => &[],
    }
}

pub(super) fn includes_of(f: &Finding) -> &[String] {
    match &f.target {
        Target::FileSet { include, .. } => include,
        _ => &[],
    }
}

pub(super) fn claimed(out: &HeuristicOutput, path: &Path) -> bool {
    out.claimed.iter().any(|c| eq_ci(c, path))
}

#[test]
fn vendor_containers_give_second_level_candidates() {
    let mut fs = MemFs::new();
    file(
        &mut fs,
        KnownFolder::AppData,
        "Adobe/Lightroom/prefs.json",
        900,
        "-1d",
    );
    file(
        &mut fs,
        KnownFolder::AppData,
        "Adobe/Photoshop/a.bin",
        900,
        "-1d",
    );
    file(&mut fs, KnownFolder::AppData, "Adobe/loose.txt", 10, "-1d");
    file(
        &mut fs,
        KnownFolder::LocalAppData,
        "JetBrains/Rider/x.bin",
        10,
        "-1d",
    );
    let out = Setup::new(fs).run();

    assert!(at(&out, &abs(KnownFolder::AppData, "Adobe")).is_empty());
    let lightroom = one(&out, &abs(KnownFolder::AppData, "Adobe/Lightroom"));
    assert_eq!(lightroom.category, Category::AppConfig);
    assert_eq!(lightroom.title, "Lightroom");
    let photoshop = one(&out, &abs(KnownFolder::AppData, "Adobe/Photoshop"));
    assert_eq!(photoshop.category, Category::Unknown);
    one(&out, &abs(KnownFolder::LocalAppData, "JetBrains/Rider"));
    assert_eq!(out.findings.len(), 3);
}

#[test]
fn every_finding_has_heuristic_evidence() {
    let mut fs = MemFs::new();
    file(
        &mut fs,
        KnownFolder::AppData,
        "Tool/settings.json",
        100,
        "-1d",
    );
    file(&mut fs, KnownFolder::AppData, "Blob/data.bin", 100, "-1d");
    let out = Setup::new(fs).run();
    assert_eq!(out.findings.len(), 2);
    for f in &out.findings {
        let [e] = f.evidence.as_slice() else {
            panic!("{:?}", f.evidence)
        };
        assert_eq!(
            e.source,
            sk_core::model::EvidenceSource::Heuristic {
                heuristic_id: "h-unk".to_owned()
            }
        );
        assert!(e.message_key.starts_with("heuristic."));
    }
}

#[test]
fn unknown_folders_send_their_summary() {
    let mut fs = MemFs::new();
    file(
        &mut fs,
        KnownFolder::AppData,
        "Mystery/data.bin",
        300,
        "-1d",
    );
    file(
        &mut fs,
        KnownFolder::AppData,
        "Tool/settings.json",
        100,
        "-1d",
    );
    let out = Setup::new(fs).run();

    let mystery = one(&out, &abs(KnownFolder::AppData, "Mystery"));
    assert_eq!(mystery.category, Category::Unknown);
    assert_eq!(mystery.evidence[0].message_key, "heuristic.unknown");
    let tool = one(&out, &abs(KnownFolder::AppData, "Tool"));
    assert_eq!(tool.category, Category::AppConfig);
    let [summary] = out.unknown.as_slice() else {
        panic!("{:?}", out.unknown)
    };
    assert_eq!(summary.path.as_str(), r"{APPDATA}\Mystery");
    assert!(claimed(&out, &abs(KnownFolder::AppData, "Mystery")));
    assert!(claimed(&out, &abs(KnownFolder::AppData, "Tool")));
}

#[test]
fn unknown_threshold_comes_from_the_config() {
    let mut fs = MemFs::new();
    file(
        &mut fs,
        KnownFolder::AppData,
        "Tool/settings.json",
        100,
        "-1d",
    );
    let mut setup = Setup::new(fs);
    setup.cfg.unknown_threshold = 0.7;
    let out = setup.run();
    let tool = one(&out, &abs(KnownFolder::AppData, "Tool"));
    // The evidence keeps what the markers said.
    assert_eq!(tool.category, Category::Unknown);
    assert_eq!(tool.evidence[0].message_key, "heuristic.config_like");
    assert_eq!(tool.evidence[0].confidence, 0.6);
    assert_eq!(out.unknown.len(), 1);
}

#[test]
fn empty_folders_are_claimed_without_finding() {
    let mut fs = MemFs::new();
    fs.add_dir(abs(KnownFolder::AppData, "Empty").to_str().unwrap());
    let out = Setup::new(fs).run();
    assert!(out.findings.is_empty());
    assert!(claimed(&out, &abs(KnownFolder::AppData, "Empty")));
}

#[test]
fn junk_is_reported_with_its_reason() {
    let mut fs = MemFs::new();
    file(
        &mut fs,
        KnownFolder::LocalAppData,
        "SquirrelTemp/a.nupkg",
        100,
        "-1d",
    );
    let out = Setup::new(fs).run();
    let junk = one(&out, &abs(KnownFolder::LocalAppData, "SquirrelTemp"));
    assert_eq!(junk.category, Category::Cache);
    assert_eq!(junk.evidence[0].message_key, "heuristic.junk.downloadable");
}

#[test]
fn claimed_candidates_and_zones_are_skipped() {
    let mut fs = MemFs::new();
    file(
        &mut fs,
        KnownFolder::AppData,
        "Code/settings.json",
        100,
        "-1d",
    );
    file(&mut fs, KnownFolder::Documents, "Notes/a.bin", 100, "-1d");
    let mut setup = Setup::new(fs);
    setup.claimed.insert(abs(KnownFolder::AppData, "Code"));
    setup.claimed.insert(abs(KnownFolder::Documents, ""));
    let out = setup.run();
    assert!(out.findings.is_empty(), "{:?}", out.findings);
    assert!(out.claimed.is_empty());
}

#[test]
fn partly_claimed_candidate_gives_the_rest() {
    let mut fs = MemFs::new();
    file(
        &mut fs,
        KnownFolder::AppData,
        "Foo/config/a.json",
        2 * MIB,
        "-1d",
    );
    file(&mut fs, KnownFolder::AppData, "Foo/[x]/b.json", MIB, "-1d");
    file(
        &mut fs,
        KnownFolder::AppData,
        "Foo/data/c.bin",
        5 * MIB,
        "-1d",
    );
    file(
        &mut fs,
        KnownFolder::AppData,
        "Bar/config/a.json",
        20 * MIB,
        "-1d",
    );
    file(
        &mut fs,
        KnownFolder::AppData,
        "Bar/rest.bin",
        MIB / 2,
        "-1d",
    );
    let mut setup = Setup::new(fs);
    for claim in ["Foo/config", "Foo/[x]", "Foo/[x]/inner", "Bar/config"] {
        setup.claimed.insert(abs(KnownFolder::AppData, claim));
    }
    let out = setup.run();

    let foo = one(&out, &abs(KnownFolder::AppData, "Foo"));
    // In component order; glob characters escaped.
    assert_eq!(excludes_of(foo), ["[[]x[]]", "config"]);
    // 0.5 MiB of 20.5 MiB: less than 10% and less than 1 MiB.
    assert!(at(&out, &abs(KnownFolder::AppData, "Bar")).is_empty());
    assert!(!claimed(&out, &abs(KnownFolder::AppData, "Bar")));
}

#[test]
fn small_rest_of_a_small_folder_is_kept() {
    let mut fs = MemFs::new();
    file(&mut fs, KnownFolder::AppData, "Foo/config.ini", 800, "-1d");
    file(&mut fs, KnownFolder::AppData, "Foo/state.json", 200, "-1d");
    let mut setup = Setup::new(fs);
    setup
        .claimed
        .insert(abs(KnownFolder::AppData, "Foo/config.ini"));
    let out = setup.run();
    // 200 of 1000 bytes is 20%.
    let foo = one(&out, &abs(KnownFolder::AppData, "Foo"));
    assert_eq!(excludes_of(foo), ["config.ini"]);
}

#[test]
fn cancellation_returns_what_was_found() {
    let mut fs = MemFs::new();
    for name in ["A", "B", "C"] {
        file(
            &mut fs,
            KnownFolder::AppData,
            &format!("{name}/a.bin"),
            1,
            "-1d",
        );
    }
    let setup = Setup::new(fs);
    let cancel = setup.cancel.clone();
    // The progress of the second candidate cancels the scan.
    let out = setup.run_with(&move |done, _| {
        if done == 2 {
            cancel.cancel();
        }
    });
    assert_eq!(out.findings.len(), 1);
    assert_eq!(out.findings[0].title, "A");

    let cancelled = Setup::new(MemFs::new());
    cancelled.cancel.cancel();
    assert_eq!(cancelled.run(), HeuristicOutput::default());
}

#[test]
fn runs_are_deterministic() {
    let make = || {
        let mut fs = MemFs::new();
        for name in ["b", "A", "c", "Adobe/z", "Adobe/y"] {
            file(
                &mut fs,
                KnownFolder::AppData,
                &format!("{name}/f.json"),
                10,
                "-1d",
            );
        }
        Setup::new(fs)
    };
    let first = make().run();
    assert_eq!(first, make().run());
    let titles: Vec<_> = first.findings.iter().map(|f| f.title.as_str()).collect();
    assert_eq!(titles, ["A", "y", "z", "b", "c"]);
}
