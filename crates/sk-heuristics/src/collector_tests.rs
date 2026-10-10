//! Tests of the collector skeleton (T-07-01): order of the heuristics,
//! accumulation of `claimed`, switches, cancellation, progress.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use sk_core::collector::{CollectContext, PostCollector, PriorResults};
use sk_core::config::{Config, HeuristicsConfig};
use sk_core::env::{Environment, KnownFolder};
use sk_core::events::{Event, ScanPhase};
use sk_core::model::{
    Category, Evidence, EvidenceSource, Finding, FindingId, FolderSummary, IssueSeverity,
    ScanIssue, Sensitivity, Target,
};
use sk_core::path::PathSet;
use sk_core::template::PathTemplate;
use sk_core::CancellationToken;
use sk_scan::MemFs;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

use crate::collector::{run, HeuristicsCollector};
use crate::heuristic::{Heuristic, HeuristicContext, HeuristicOutput};

fn env() -> Environment {
    Environment::fake(Path::new(if cfg!(windows) { r"C:\fake" } else { "/fake" }))
}

fn app_data(name: &str) -> PathBuf {
    env().known_folder(KnownFolder::AppData).unwrap().join(name)
}

fn finding(name: &str, source: EvidenceSource) -> Finding {
    let target = Target::FileSet {
        root: PathTemplate::parse(&format!(r"{{APPDATA}}\{name}")).unwrap(),
        resolved: app_data(name),
        include: vec![],
        exclude: vec![],
    };
    Finding {
        id: FindingId::for_target(&target),
        target,
        category: Category::Unknown,
        app: None,
        title: name.to_owned(),
        evidence: vec![Evidence {
            source,
            message_key: "heuristic.unknown".to_owned(),
            message_args: BTreeMap::new(),
            confidence: 0.0,
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

/// What a fake heuristic saw when it ran.
#[derive(Debug, Clone, PartialEq)]
struct Seen {
    id: &'static str,
    /// Folders of [`NAMES`] already claimed.
    claimed: Vec<&'static str>,
    /// Titles of the prior findings.
    prior: Vec<String>,
}

/// Folders the fake heuristics claim, one per heuristic id.
const NAMES: [&str; 6] = ["Rules", "Git", "Web", "Junk", "Unk", "Usr"];

fn name_of(id: &str) -> &'static str {
    match id {
        "h-git" => "Git",
        "h-web" => "Web",
        "h-junk" => "Junk",
        "h-unk" => "Unk",
        "h-usr" => "Usr",
        _ => "Other",
    }
}

/// Claims `{APPDATA}\<name>`, adds a finding, an issue and an unknown summary
/// for it, and records what it saw.
struct Fake {
    id: &'static str,
    log: Arc<Mutex<Vec<Seen>>>,
}

impl Heuristic for Fake {
    fn id(&self) -> &'static str {
        self.id
    }

    fn run(&self, hctx: &HeuristicContext<'_>) -> HeuristicOutput {
        let claimed = NAMES
            .into_iter()
            .filter(|n| hctx.claimed.covers(&app_data(n).join("sub")))
            .collect();
        let prior = hctx.prior.iter().map(|f| f.title.clone()).collect();
        self.log.lock().unwrap().push(Seen {
            id: self.id,
            claimed,
            prior,
        });
        (hctx.progress)(1, Some(self.id));
        let name = name_of(self.id);
        HeuristicOutput {
            findings: vec![finding(
                name,
                EvidenceSource::Heuristic {
                    heuristic_id: self.id.to_owned(),
                },
            )],
            unknown: vec![],
            claimed: vec![app_data(name)],
            issues: vec![ScanIssue {
                severity: IssueSeverity::Info,
                source: self.id.to_owned(),
                path: None,
                message_key: "test.ran".to_owned(),
                message_args: BTreeMap::new(),
            }],
        }
    }
}

/// The five heuristics of SPEC-07, given in a shuffled order.
fn fakes(log: &Arc<Mutex<Vec<Seen>>>) -> Vec<Box<dyn Heuristic>> {
    ["h-usr", "h-unk", "h-web", "h-junk", "h-git"]
        .into_iter()
        .map(|id| {
            Box::new(Fake {
                id,
                log: Arc::clone(log),
            }) as Box<dyn Heuristic>
        })
        .collect()
}

fn context() -> (CollectContext, UnboundedReceiver<Event>) {
    let (events, rx) = unbounded_channel();
    let ctx = CollectContext {
        env: Arc::new(env()),
        config: Arc::new(Config::default()),
        scanner: Arc::new(MemFs::new()),
        events,
        cancel: CancellationToken::new(),
    };
    (ctx, rx)
}

fn prior() -> (Vec<Finding>, PathSet) {
    let findings = vec![finding("Rules", EvidenceSource::User)];
    let claimed: PathSet = [app_data("Rules")].into_iter().collect();
    (findings, claimed)
}

#[tokio::test]
async fn empty_collector_finds_nothing() {
    let (ctx, _rx) = context();
    let (findings, claimed) = prior();
    let collector = HeuristicsCollector::new(HeuristicsConfig::default());
    assert_eq!(collector.id(), "heuristics");
    let prior = PriorResults {
        findings: &findings,
        claimed: &claimed,
    };
    let out = collector.collect(&ctx, &prior).await.unwrap();
    assert_eq!(out, Default::default());
}

#[tokio::test]
async fn heuristics_run_in_order_and_accumulate_claimed() {
    let (ctx, mut rx) = context();
    let (findings, claimed) = prior();
    let log = Arc::new(Mutex::new(Vec::new()));
    let collector = HeuristicsCollector::with_heuristics(HeuristicsConfig::default(), fakes(&log));
    let prior = PriorResults {
        findings: &findings,
        claimed: &claimed,
    };
    let out = collector.collect(&ctx, &prior).await.unwrap();

    let seen = log.lock().unwrap().clone();
    let ids: Vec<_> = seen.iter().map(|s| s.id).collect();
    assert_eq!(ids, ["h-git", "h-web", "h-junk", "h-unk", "h-usr"]);
    // Each heuristic sees the claims and findings of the collectors and of
    // the heuristics before it.
    for (i, s) in seen.iter().enumerate() {
        let expected: Vec<&str> = NAMES[..=i].to_vec();
        assert_eq!(s.claimed, expected, "{}", s.id);
        assert_eq!(s.prior, expected, "{}", s.id);
    }

    // Only the heuristics' own results come out, in order.
    let titles: Vec<_> = out.findings.iter().map(|f| f.title.as_str()).collect();
    assert_eq!(titles, ["Git", "Web", "Junk", "Unk", "Usr"]);
    let expected_claims: Vec<_> = NAMES[1..].iter().map(|n| app_data(n)).collect();
    assert_eq!(out.claimed_paths, expected_claims);
    let sources: Vec<_> = out.issues.iter().map(|i| i.source.as_str()).collect();
    assert_eq!(sources, ids);

    let mut progress = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let Event::Progress { phase, current, .. } = event {
            assert_eq!(phase, ScanPhase::Heuristics);
            progress.push(current);
        }
    }
    // The first one goes out at once; the throttled ones are flushed when
    // the sink is dropped, the newest last.
    assert_eq!(progress.first().cloned(), Some(Some("h-git".to_owned())));
    assert_eq!(progress.last().cloned(), Some(Some("h-usr".to_owned())));
}

#[tokio::test]
async fn switched_off_heuristics_do_not_run() {
    let (ctx, _rx) = context();
    let (findings, claimed) = prior();
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut cfg = HeuristicsConfig::default();
    cfg.enabled.git = false;
    cfg.enabled.junk = false;
    cfg.enabled.usr = false;
    let collector = HeuristicsCollector::with_heuristics(cfg, fakes(&log));
    let prior = PriorResults {
        findings: &findings,
        claimed: &claimed,
    };
    let out = collector.collect(&ctx, &prior).await.unwrap();
    let ids: Vec<_> = log.lock().unwrap().iter().map(|s| s.id).collect();
    assert_eq!(ids, ["h-web", "h-unk"]);
    assert_eq!(out.findings.len(), 2);
}

/// Cancels the scan when it runs.
struct Canceller(CancellationToken);

impl Heuristic for Canceller {
    fn id(&self) -> &'static str {
        "h-web"
    }

    fn run(&self, _: &HeuristicContext<'_>) -> HeuristicOutput {
        self.0.cancel();
        HeuristicOutput {
            claimed: vec![app_data("Web")],
            ..HeuristicOutput::default()
        }
    }
}

#[test]
fn cancellation_keeps_partial_output() {
    let (ctx, _rx) = context();
    let (findings, claimed) = prior();
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut heuristics = fakes(&log);
    heuristics.retain(|h| h.id() != "h-web");
    heuristics.push(Box::new(Canceller(ctx.cancel.clone())));
    let collector = HeuristicsCollector::with_heuristics(HeuristicsConfig::default(), heuristics);
    let collected = run(
        &collector.heuristics,
        &collector.cfg,
        &ctx,
        findings,
        claimed,
    );
    let ids: Vec<_> = log.lock().unwrap().iter().map(|s| s.id).collect();
    assert_eq!(ids, ["h-git"]);
    let titles: Vec<_> = collected
        .output
        .findings
        .iter()
        .map(|f| f.title.as_str())
        .collect();
    assert_eq!(titles, ["Git"]);
    assert_eq!(
        collected.output.claimed_paths,
        [app_data("Git"), app_data("Web")]
    );
}

#[test]
fn already_cancelled_runs_nothing() {
    let (ctx, _rx) = context();
    ctx.cancel.cancel();
    let (findings, claimed) = prior();
    let log = Arc::new(Mutex::new(Vec::new()));
    let collector = HeuristicsCollector::with_heuristics(HeuristicsConfig::default(), fakes(&log));
    let collected = run(
        &collector.heuristics,
        &collector.cfg,
        &ctx,
        findings,
        claimed,
    );
    assert!(log.lock().unwrap().is_empty());
    assert_eq!(collected.output, Default::default());
    assert!(collected.unknown.is_empty());
}

/// Reports its folder as unknown.
struct Unknown;

impl Heuristic for Unknown {
    fn id(&self) -> &'static str {
        "h-unk"
    }

    fn run(&self, _: &HeuristicContext<'_>) -> HeuristicOutput {
        HeuristicOutput {
            unknown: vec![summary()],
            ..HeuristicOutput::default()
        }
    }
}

fn summary() -> FolderSummary {
    FolderSummary {
        path: PathTemplate::parse(r"{APPDATA}\Mystery").unwrap(),
        total_bytes: 10,
        file_count: 1,
        dir_count: 0,
        max_depth: 0,
        newest_mtime: None,
        oldest_mtime: None,
        ext_histogram: vec![],
        sample_names: vec![],
        top_children: vec![],
        markers: vec![],
        truncated: false,
    }
}

#[test]
fn unknown_summaries_are_gathered() {
    let (ctx, _rx) = context();
    let collector =
        HeuristicsCollector::with_heuristics(HeuristicsConfig::default(), vec![Box::new(Unknown)]);
    let collected = run(
        &collector.heuristics,
        &collector.cfg,
        &ctx,
        Vec::new(),
        PathSet::new(),
    );
    assert_eq!(collected.unknown, [summary()]);
}
