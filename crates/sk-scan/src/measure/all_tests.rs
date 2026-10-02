//! `measure_all` (SPEC-03 §4.3, §6).

use std::collections::BTreeMap;
use std::sync::atomic::Ordering as AtomicOrdering;

use sk_core::events::Event;
use sk_core::model::{Category, Evidence, EvidenceSource, FindingId, RegHive, Sensitivity};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

use super::*;
use crate::measure::tests::{file, opts, s, set, Wrap};
use crate::MemFs;

fn finding(target: Target) -> Finding {
    Finding {
        id: FindingId::for_target(&target),
        target,
        category: Category::AppData,
        app: None,
        title: "t".to_owned(),
        evidence: vec![Evidence {
            source: EvidenceSource::User,
            message_key: "evidence.test".to_owned(),
            message_args: BTreeMap::new(),
            confidence: 1.0,
            importance: None,
        }],
        stats: None,
        sensitivity: Sensitivity::None,
        score: None,
        default_selected: false,
        requires_elevation: false,
        tags: vec![],
        children: vec![],
        notes_key: None,
    }
}

fn registry() -> Finding {
    finding(Target::Registry {
        hive: RegHive::Hkcu,
        key: r"Software\X".to_owned(),
        recursive: true,
    })
}

fn events(rx: &mut UnboundedReceiver<Event>) -> Vec<Event> {
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() {
        out.push(e);
    }
    out
}

fn run_all(fs: &dyn FsScanner, findings: &mut [Finding]) -> (Vec<ScanIssue>, Vec<Event>) {
    let (tx, mut rx) = unbounded_channel();
    let issues = measure_all(fs, findings, &opts(), &tx, &CancellationToken::new());
    (issues, events(&mut rx))
}

/// «Готово, когда»: nested roots are walked once (FR-03-08).
#[test]
fn nested_roots_are_walked_once() {
    let mut fs = MemFs::new();
    fs.add_file(&s("game/saves/slot1/a.sav"), 10, "-1d", None)
        .add_file(&s("game/saves/b.sav"), 20, "-1d", None)
        .add_file(&s("game/config/c.ini"), 1, "-1d", None)
        .add_file(&s("other/d.txt"), 5, "-1d", None);
    // Inner roots first, to show that the order of findings does not matter.
    let mut findings = vec![
        finding(set("game/saves/slot1", &[], &[])),
        finding(set("game/saves", &[], &[])),
        finding(set("game", &[], &[])),
        finding(set("other", &[], &[])),
        finding(set("game/saves", &[], &[])),
    ];
    let titles: Vec<_> = findings.iter().map(|f| f.target.clone()).collect();
    let (issues, _) = run_all(&fs, &mut findings);
    assert!(issues.is_empty(), "{issues:?}");
    // game, game/saves, game/saves/slot1, game/config, other.
    assert_eq!(fs.calls().read_dir, 5, "each folder is listed once");

    let order: Vec<_> = findings.iter().map(|f| f.target.clone()).collect();
    assert_eq!(order, titles, "order of findings is kept");
    let bytes: Vec<_> = findings
        .iter()
        .map(|f| f.stats.as_ref().unwrap().total_bytes)
        .collect();
    assert_eq!(bytes, [10, 30, 31, 5, 30]);
    let slot = findings[0].stats.as_ref().unwrap();
    assert_eq!((slot.file_count, slot.dir_count), (1, 0));
    assert_eq!(findings[2].stats.as_ref().unwrap().dir_count, 3);
}

/// A chain of nested roots runs level by level: C waits for B, which waits for A.
#[test]
fn chained_nested_roots_are_walked_once() {
    for _ in 0..20 {
        let mut fs = MemFs::new();
        fs.add_file(&s("proj/x.txt"), 1, "-1d", None).add_file(
            &s("proj/node_modules/a/b/c/f.js"),
            2,
            "-1d",
            None,
        );
        // node_modules is excluded from A's walk, so B walks it and caches
        // C, which is 3 levels below B and 4 below A.
        let mut findings = vec![
            finding(set("proj/node_modules/a/b/c", &[], &[])),
            finding(set("proj/node_modules", &[], &[])),
            finding(set("proj", &[], &[])),
            finding(set("proj/node_modules/a/b/c", &[], &[])),
        ];
        let (issues, _) = run_all(&fs, &mut findings);
        assert!(issues.is_empty(), "{issues:?}");
        // proj, node_modules, a, b, c: each listed once.
        assert_eq!(fs.calls().read_dir, 5);
        let bytes: Vec<_> = findings
            .iter()
            .map(|f| f.stats.as_ref().unwrap().total_bytes)
            .collect();
        assert_eq!(bytes, [2, 2, 1, 2]);
    }
}

#[test]
fn levels_follow_nesting_chains() {
    let job = |idx: usize, rel: &str| Job {
        idx,
        key: Some(CacheKey::new(
            &crate::measure::tests::p(rel),
            &[],
            &[],
            Mode::Full,
        )),
        template: String::new(),
    };
    let mut jobs = vec![
        job(0, "r"),
        job(1, "r/1"),
        job(2, "r/1/2/3/4"),
        job(3, "r/1/2/3/4"),
        job(4, "q"),
        Job {
            idx: 5,
            key: None,
            template: String::new(),
        },
    ];
    jobs.sort_by_key(|j| (j.key.as_ref().map_or(0, |k| k.path.len()), j.idx));
    let by_idx: BTreeMap<usize, usize> = jobs
        .iter()
        .zip(levels(&jobs))
        .map(|(j, l)| (j.idx, l))
        .collect();
    assert_eq!(
        by_idx.into_iter().collect::<Vec<_>>(),
        [(0, 0), (1, 1), (2, 2), (3, 3), (4, 0), (5, 0)]
    );
}

#[test]
fn issues_and_results_by_kind() {
    let mut fs = MemFs::new();
    fs.add_file(&s("ok/a.txt"), 3, "-1d", None)
        .add_reparse(&s("My Music"), ReparseKind::Junction)
        .add_file(&s("lnk.cfg"), 9, "-1d", None)
        .add_reparse(&s("lnk.cfg"), ReparseKind::Symlink)
        .add_file(&s("od.txt"), 4, "-1d", None)
        .add_reparse(&s("od.txt"), ReparseKind::CloudPlaceholder)
        .add_dir(&s("folder"));
    let fs = Wrap::new(fs);
    let mut findings = vec![
        finding(set("ok", &[], &[])),
        finding(set("missing", &[], &[])),
        finding(set("My Music", &[], &[])),
        finding(file("lnk.cfg")),
        finding(file("folder")),
        registry(),
        finding(file("od.txt")),
    ];
    let (issues, _) = run_all(&fs, &mut findings);

    assert_eq!(findings[0].stats.as_ref().unwrap().total_bytes, 3);
    assert_eq!(findings[1].stats, None);
    let zero = Acc::default().stats(false);
    assert_eq!(findings[2].stats.as_ref(), Some(&zero));
    assert_eq!(findings[2].tags, ["reparse_root"]);
    assert_eq!(findings[3].stats.as_ref(), Some(&zero));
    assert_eq!(findings[3].tags, ["reparse_root"]);
    assert_eq!(findings[4].stats, None);
    assert_eq!(findings[5].stats, None);
    // A cloud placeholder file is a file, not a reparse root.
    assert_eq!(findings[6].stats.as_ref().unwrap().file_count, 1);
    assert!(findings[6].tags.is_empty());

    let summary: Vec<_> = issues
        .iter()
        .map(|i| (i.severity, i.message_key.as_str(), i.path.clone()))
        .collect();
    let template = |f: &Finding| match &f.target {
        Target::FileSet { root, .. } => Some(root.as_str().to_owned()),
        Target::File { path, .. } => Some(path.as_str().to_owned()),
        _ => None,
    };
    assert_eq!(
        summary,
        [
            (
                IssueSeverity::Info,
                "issue.scan.reparse_root",
                template(&findings[2])
            ),
            (
                IssueSeverity::Info,
                "issue.scan.reparse_root",
                template(&findings[3])
            ),
            (
                IssueSeverity::Warning,
                "issue.scan.measure_failed",
                template(&findings[4])
            ),
        ]
    );
    assert!(issues.iter().all(|i| i.source == "measure"));
    assert!(issues[2].message_args.contains_key("error"));
}

#[test]
fn unreadable_folders_give_one_warning_per_finding() {
    let mut fs = MemFs::new();
    fs.add_file(&s("a/x.txt"), 1, "-1d", None)
        .add_file(&s("b/y.txt"), 1, "-1d", None);
    let fs = Wrap::new(fs);
    fs.walk_errors.store(3, AtomicOrdering::Relaxed);
    let mut findings = vec![finding(set("a", &[], &[])), finding(set("b", &[], &[]))];
    let (issues, _) = run_all(&fs, &mut findings);
    assert_eq!(issues.len(), 2);
    for (issue, f) in issues.iter().zip(&findings) {
        assert_eq!(issue.severity, IssueSeverity::Warning);
        assert_eq!(issue.message_key, "issue.scan.dirs_unreadable");
        assert_eq!(issue.message_args["count"], "3");
        let Target::FileSet { root, .. } = &f.target else {
            panic!()
        };
        assert_eq!(issue.path.as_deref(), Some(root.as_str()));
        assert!(f.stats.as_ref().unwrap().truncated);
    }
}

#[test]
fn progress_counts_file_system_findings() {
    let mut fs = MemFs::new();
    fs.add_file(&s("a/x.txt"), 1, "-1d", None)
        .add_file(&s("f.cfg"), 1, "-1d", None);
    let mut findings = vec![
        registry(),
        finding(set("a", &[], &[])),
        finding(file("f.cfg")),
        finding(set("missing", &[], &[])),
    ];
    let (_, events) = run_all(&fs, &mut findings);
    let progress: Vec<_> = events
        .iter()
        .map(|e| match e {
            Event::Progress {
                phase,
                done,
                total,
                current,
            } => (*phase, *done, *total, current.clone()),
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert!(!progress.is_empty());
    let (phase, done, total, current) = progress.last().unwrap().clone();
    assert_eq!((phase, done, total), (ScanPhase::Measure, 3, Some(3)));
    let templates: Vec<_> = findings[1..]
        .iter()
        .map(|f| match &f.target {
            Target::FileSet { root, .. } => root.as_str().to_owned(),
            Target::File { path, .. } => path.as_str().to_owned(),
            _ => unreachable!(),
        })
        .collect();
    assert!(templates.contains(&current.unwrap()));
    assert!(progress.windows(2).all(|w| w[0].1 < w[1].1));
}

#[test]
fn cancellation_leaves_stats_empty() {
    let mut fs = MemFs::new();
    fs.add_file(&s("a/x.txt"), 1, "-1d", None);
    let mut findings = vec![finding(set("a", &[], &[])), finding(file("a/x.txt"))];
    findings[0].stats = Some(Acc::default().stats(true));
    let (tx, mut rx) = unbounded_channel();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let issues = measure_all(&fs, &mut findings, &opts(), &tx, &cancel);
    assert!(issues.is_empty());
    assert!(findings.iter().all(|f| f.stats.is_none()));
    assert!(events(&mut rx).is_empty());
    assert_eq!(fs.calls().read_dir, 0);
}
