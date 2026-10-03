//! Helpers shared by the tests of the built-in rule files: a fake root, the
//! collector run, OS-independent snapshots and file-set measurement.

// Every test crate uses only some of the helpers.
#![allow(dead_code)]
// Helpers outside `#[test]` functions are not covered by `allow-unwrap-in-tests`.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};
use sk_core::collector::{CollectContext, CollectOutput, Collector};
use sk_core::config::Config;
use sk_core::env::{Environment, KnownFolder};
use sk_core::model::{EvidenceSource, Finding};
use sk_core::registry::MemRegistry;
use sk_core::CancellationToken;
use sk_rules::{RuleSet, RulesCollector};
use sk_scan::{measure, DirStatsCache, ExcludeSet, MeasureOptions, MemFs};
use tokio::sync::mpsc::unbounded_channel;

/// Root of the fake profile.
pub fn root() -> PathBuf {
    PathBuf::from(if cfg!(windows) { r"C:\fake" } else { "/fake" })
}

/// Absolute path of `rel` (`/`-separated) under a known folder of `env`.
pub fn under(env: &Environment, folder: KnownFolder, rel: &str) -> PathBuf {
    let base = env.known_folder(folder).unwrap().to_path_buf();
    rel.split('/').fold(base, |p, c| p.join(c))
}

/// `path` as a string.
pub fn s(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Replaces the fake root at the start of every string with `[root]` and
/// makes the rest `/`-separated, so the snapshot is the same on every OS.
pub fn redact(value: &mut Value, root: &str) {
    match value {
        Value::String(s) => {
            if let Some(rest) = s.strip_prefix(root) {
                *s = format!("[root]{}", rest.replace('\\', "/"));
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|v| redact(v, root)),
        Value::Object(map) => map.values_mut().for_each(|v| redact(v, root)),
        _ => {}
    }
}

/// Findings, claimed paths and issues of `out` as one OS-independent value.
pub fn snapshot(out: &CollectOutput) -> Value {
    let mut snapshot = json!({
        "findings": out.findings,
        "claimed_paths": out.claimed_paths,
        "issues": out.issues,
    });
    redact(&mut snapshot, &root().to_string_lossy());
    snapshot
}

/// Runs `set` through `RulesCollector` on the given profile.
pub async fn collect(
    set: RuleSet,
    fs: MemFs,
    env: Environment,
    registry: MemRegistry,
) -> CollectOutput {
    let (events, _rx) = unbounded_channel();
    let ctx = CollectContext {
        env: Arc::new(env),
        config: Arc::new(Config::default()),
        scanner: Arc::new(fs),
        events,
        cancel: CancellationToken::new(),
    };
    let collector = RulesCollector::new(Arc::new(set)).with_registry(Arc::new(registry));
    collector.collect(&ctx).await.unwrap()
}

/// Loads one rule file as it would be loaded from `rules.d`; the file must
/// give no issues.
pub fn load_file(file: &str, text: &str) -> RuleSet {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(file), text).unwrap();
    let (set, issues) = RuleSet::load(false, Some(dir.path()));
    assert_eq!(issues, vec![], "{file}");
    set
}

/// Ids of the rules behind the findings.
pub fn finding_rules(out: &CollectOutput) -> BTreeSet<String> {
    out.findings
        .iter()
        .flat_map(|f| &f.evidence)
        .filter_map(|e| match &e.source {
            EvidenceSource::Rule { rule_id } => Some(rule_id.clone()),
            _ => None,
        })
        .collect()
}

/// Findings of rule `rule`, in the output order.
pub fn findings_of<'a>(out: &'a CollectOutput, rule: &str) -> Vec<&'a Finding> {
    out.findings
        .iter()
        .filter(|f| {
            f.evidence
                .iter()
                .any(|e| matches!(&e.source, EvidenceSource::Rule { rule_id } if rule_id == rule))
        })
        .collect()
}

/// File count and size of `finding` as the Measure phase sees them. Global
/// exclusions are off, so only the rule's own globs decide.
pub fn measure_finding(fs: &MemFs, env: &Environment, finding: &Finding) -> (u64, u64) {
    let mut opts = MeasureOptions::new(Arc::new(ExcludeSet::builtin(env)), 64);
    opts.include_excluded = true;
    opts.probe_locks = false;
    let cache = DirStatsCache::new();
    let cancel = CancellationToken::new();
    let stats = measure(fs, &finding.target, &cache, &opts, &cancel)
        .unwrap()
        .unwrap();
    (stats.file_count, stats.total_bytes)
}
