//! The Score phase of `ScanPipeline` (SPEC-01 §4.4, SPEC-09 T-09-08).

// Helpers outside `#[test]` functions are not covered by `allow-unwrap-in-tests`.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use sk_core::collector::{CollectContext, CollectOutput, Collector};
use sk_core::config::Config;
use sk_core::env::{Environment, KnownFolder};
use sk_core::error::CollectorError;
use sk_core::events::{Event, ScanPhase};
use sk_core::model::{Category, Evidence, EvidenceSource, Finding, FindingId, Sensitivity, Target};
use sk_core::registry::MemRegistry;
use sk_core::template::PathTemplate;
use sk_core::CancellationToken;
use sk_engine::{ScanOptions, ScanPipeline};
use sk_scan::MemFs;
use sk_system::SystemCollector;
use tokio::sync::mpsc::unbounded_channel;

fn env() -> Environment {
    Environment::fake(Path::new(if cfg!(windows) { r"C:\fake" } else { "/fake" }))
}

fn app_data(rel: &str) -> PathBuf {
    env().known_folder(KnownFolder::AppData).unwrap().join(rel)
}

fn finding(rel: &str, category: Category, file: bool) -> Finding {
    let resolved = app_data(rel);
    let template = PathTemplate::from_path(&resolved, &env());
    let target = if file {
        Target::File {
            path: template,
            resolved,
        }
    } else {
        Target::FileSet {
            root: template,
            resolved,
            include: vec![],
            exclude: vec![],
        }
    };
    Finding {
        id: FindingId::for_target(&target),
        target,
        category,
        app: None,
        title: rel.to_owned(),
        evidence: vec![Evidence {
            source: EvidenceSource::Rule {
                rule_id: rel.to_owned(),
            },
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

struct Rules;

#[async_trait]
impl Collector for Rules {
    fn id(&self) -> &'static str {
        "rules"
    }
    fn display_key(&self) -> &'static str {
        "collector.rules"
    }
    async fn collect(&self, _: &CollectContext) -> Result<CollectOutput, CollectorError> {
        let mut keys = finding("Keys", Category::Credentials, false);
        keys.sensitivity = Sensitivity::High;
        // The same id seen again as Cache with a weaker evidence: merged.
        let mut save_dup = finding("Game", Category::Cache, false);
        save_dup.evidence[0].source = EvidenceSource::Heuristic {
            heuristic_id: "h".to_owned(),
        };
        save_dup.evidence[0].confidence = 0.5;
        Ok(CollectOutput {
            findings: vec![
                finding("Cache", Category::Cache, false),
                finding("Game", Category::GameSave, false),
                finding("Game/slot1.sav", Category::GameSave, true),
                save_dup,
                keys,
            ],
            claimed_paths: vec![app_data("Cache"), app_data("Game"), app_data("Keys")],
            ..CollectOutput::default()
        })
    }
}

fn scanner() -> Arc<MemFs> {
    let mut fs = MemFs::new();
    let s = |rel: &str| app_data(rel).to_string_lossy().into_owned();
    fs.add_file(&s("Cache/blob.bin"), 1000, "-1d", None)
        .add_file(&s("Game/slot1.sav"), 30, "-1d", None)
        .add_file(&s("Game/slot2.sav"), 20, "-1d", None)
        .add_file(&s("Keys/db.kdbx"), 5, "-1d", None);
    Arc::new(fs)
}

#[tokio::test]
async fn score_phase_merges_scores_selects_and_counts() {
    let pipeline = ScanPipeline::new(Arc::new(Config::default()))
        .with_environment(env())
        .with_scanner(scanner())
        .with_games_registry(Arc::new(MemRegistry::new()))
        .with_system_registry(Arc::new(MemRegistry::new()))
        .with_collector(Arc::new(SystemCollector::with_exporters(Vec::new())))
        .with_collector(Arc::new(Rules));
    let (tx, mut rx) = unbounded_channel();
    let report = pipeline
        .run(ScanOptions::default(), tx, CancellationToken::new())
        .await
        .unwrap();

    // Merged by id and sorted by SPEC-09 §4.8 (Credentials first, unlike the
    // declaration order of `Category`).
    let titles: Vec<_> = report.findings.iter().map(|f| f.title.as_str()).collect();
    assert_eq!(titles, ["Keys", "Game", "Game/slot1.sav", "Cache"]);

    let by = |title: &str| report.findings.iter().find(|f| f.title == title).unwrap();
    assert!(report.findings.iter().all(|f| f.score.is_some()));
    let game = by("Game");
    assert_eq!(game.category, Category::GameSave);
    // Its own rule, the merged heuristic and the rule of the nested child.
    assert_eq!(game.evidence.len(), 3);
    assert!(game.default_selected);
    // `now` is the scan clock: files changed a day ago are recent.
    assert_eq!(game.score.as_ref().unwrap().components["recency"], 1.0);
    let slot = by("Game/slot1.sav");
    assert!(slot.tags.iter().any(|t| t == "nested"));
    assert_eq!(game.children, vec![slot.id.clone()]);
    assert!(!slot.default_selected);
    let cache = by("Cache");
    assert!(!cache.default_selected);
    assert!(cache.tags.iter().any(|t| t == "restorable"));
    assert!(by("Keys").default_selected);

    // Totals over the top-level findings.
    assert_eq!(report.totals.all.count, 3);
    assert_eq!(report.totals.all.bytes, 1055);
    assert_eq!(report.totals.all.selected_count, 2);
    assert_eq!(report.totals.all.selected_bytes, 55);
    assert_eq!(report.totals.sensitive_selected, 1);

    let issue = report
        .issues
        .iter()
        .find(|i| i.message_key == "scoring.sensitive_selected")
        .unwrap();
    assert_eq!(issue.message_args["count"], "1");

    // The issue is also sent as an event inside the Score phase.
    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }
    let phase = |started: bool| {
        events
            .iter()
            .position(|e| match e {
                Event::PhaseStarted { phase } => started && *phase == ScanPhase::Score,
                Event::PhaseFinished { phase, .. } => !started && *phase == ScanPhase::Score,
                _ => false,
            })
            .unwrap()
    };
    let inside = &events[phase(true)..phase(false)];
    assert!(inside.iter().any(|e| matches!(
        e,
        Event::Issue { issue } if issue.message_key == "scoring.sensitive_selected"
    )));
}
