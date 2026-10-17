//! Windows-only: H-WEB and H-UNK on the real profile (SPEC-07 §6, T-07-12).
//!
//! `Environment::detect()` and `RealFs`; read-only, so it runs in the default
//! `cargo test`. Every entry of the `{LOCALAPPDATA}` zone (§4.2.1–§4.2.3,
//! vendor containers and `Packages` expanded into their children) has at
//! most one finding with that root and is claimed, or has claimed
//! descendants (the rest is below the threshold of §5) — FR-07-02.

#![cfg(windows)]
// Helpers outside `#[test]` functions are not covered by `allow-unwrap-in-tests`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use sk_core::collector::{CollectContext, PostCollector, PriorResults};
use sk_core::config::{Config, HeuristicsConfig};
use sk_core::env::{Environment, KnownFolder};
use sk_core::fs::{EntryKind, EntryMeta, Exclusion, FsScanner, PathFilter, ReparseKind};
use sk_core::model::{Finding, Target};
use sk_core::path::{eq_ci, starts_with_ci, PathSet};
use sk_core::template::PathTemplate;
use sk_core::CancellationToken;
use sk_heuristics::HeuristicsCollector;
use sk_scan::{ExcludeSet, RealFs};
use tokio::sync::mpsc::unbounded_channel;

/// The built-in vendor list (SPEC-07 §4.2.2), read independently of the crate.
const VENDORS: &str = include_str!("../data/vendors.txt");
/// `FILE_ATTRIBUTE_DIRECTORY`: tells a cloud placeholder folder from a file.
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
/// Windows' own Store packages, skipped by §4.2.3.
const SYSTEM_PACKAGE_PREFIXES: [&str; 2] = ["microsoft.windows.", "microsoftwindows."];

fn vendors() -> Vec<String> {
    VENDORS
        .lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter(|l| !l.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A folder or a cloud placeholder folder; links and junctions are not.
fn is_dir(meta: &EntryMeta) -> bool {
    match meta.kind {
        EntryKind::Dir => true,
        EntryKind::Reparse(ReparseKind::CloudPlaceholder) => {
            meta.attrs & FILE_ATTRIBUTE_DIRECTORY != 0
        }
        EntryKind::File | EntryKind::Reparse(_) => false,
    }
}

/// Lists the folders of the zone the way §4.2.1 describes them.
struct Zone<'a> {
    fs: &'a RealFs,
    env: &'a Environment,
    excludes: ExcludeSet,
}

impl Zone<'_> {
    /// Candidate folders of `dir`: not excluded globally, not links, not
    /// holding a known folder. An unreadable folder has none.
    fn dirs(&self, dir: &Path) -> Vec<PathBuf> {
        let Ok(listing) = self.fs.read_dir(dir) else {
            return Vec::new();
        };
        listing
            .into_iter()
            .filter(|e| is_dir(&e.meta))
            .map(|e| e.path)
            .filter(|p| {
                let n = p.file_name().unwrap_or(OsStr::new(""));
                self.excludes.check(p, n, true) != Exclusion::Exclude
            })
            .filter(|p| {
                !self
                    .env
                    .known_folders
                    .values()
                    .any(|known| starts_with_ci(known, p))
            })
            .collect()
    }

    /// The entries of `{LOCALAPPDATA}` (§4.2.1–§4.2.3).
    fn entries(&self, root: &Path) -> Vec<PathBuf> {
        let vendors = vendors();
        let mut out = Vec::new();
        for dir in self.dirs(root) {
            let lower = name(&dir).to_lowercase();
            if lower == "packages" {
                out.extend(self.dirs(&dir).into_iter().filter(|p| {
                    let n = name(p).to_lowercase();
                    !SYSTEM_PACKAGE_PREFIXES.iter().any(|s| n.starts_with(s))
                }));
            } else if vendors.contains(&lower) {
                out.extend(self.dirs(&dir));
            } else {
                out.push(dir);
            }
        }
        out
    }
}

fn root_of(f: &Finding) -> Option<&Path> {
    match &f.target {
        Target::FileSet { resolved, .. } | Target::File { resolved, .. } => Some(resolved),
        _ => None,
    }
}

#[tokio::test]
async fn local_app_data_folders_are_found_once_or_claimed() {
    let env = Environment::detect().expect("environment");
    let local = env
        .known_folder(KnownFolder::LocalAppData)
        .expect("{LOCALAPPDATA}")
        .to_path_buf();
    let fs = Arc::new(RealFs::new(&env));

    let mut cfg = HeuristicsConfig::default();
    cfg.enabled.git = false;
    cfg.enabled.usr = false;
    cfg.enabled.web = true;
    cfg.enabled.unk = true;
    let collector = HeuristicsCollector::new(cfg);
    let (events, _rx) = unbounded_channel();
    let ctx = CollectContext {
        env: Arc::new(env.clone()),
        config: Arc::new(Config::default()),
        scanner: Arc::clone(&fs) as Arc<dyn FsScanner>,
        events,
        cancel: CancellationToken::new(),
    };
    let claimed = PathSet::default();
    let prior = PriorResults {
        findings: &[],
        claimed: &claimed,
    };
    let start = Instant::now();
    let out = collector.collect(&ctx, &prior).await.expect("collect");
    let elapsed = start.elapsed();
    println!(
        "h-web + h-unk on the real profile: {:.2} s, {} findings, {} issues \
         (NFR-07-01 reference: <= 10 s)",
        elapsed.as_secs_f64(),
        out.findings.len(),
        out.issues.len()
    );

    let claimed: PathSet = out.claimed_paths.iter().cloned().collect();
    let zone = Zone {
        fs: &fs,
        env: &env,
        excludes: ExcludeSet::builtin(&env),
    };
    let entries = zone.entries(&local);
    assert!(!entries.is_empty(), "{{LOCALAPPDATA}} has no folders");
    println!(
        "{} entries of the {{LOCALAPPDATA}} zone checked",
        entries.len()
    );
    let mut problems = Vec::new();
    for entry in &entries {
        let template = PathTemplate::from_path(entry, &env);
        let found = out
            .findings
            .iter()
            .filter(|f| root_of(f).is_some_and(|r| eq_ci(r, entry)))
            .count();
        if found > 1 {
            problems.push(format!("{template}: {found} findings"));
            continue;
        }
        let explained = claimed.covers(entry) || !claimed.descendants(entry).is_empty();
        // Folders that vanished during the run (temporary folders) are skipped.
        if !explained && fs.metadata(entry).is_ok() {
            problems.push(format!("{template}: neither found nor claimed"));
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");
}
