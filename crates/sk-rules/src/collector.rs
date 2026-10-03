//! The rules engine as a scan collector (SPEC-04 §4.5, SPEC-01 §4.3).
//!
//! [`RulesCollector`] runs every active rule of a [`RuleSet`]: it evaluates
//! the conditions, expands the targets of matched rules into findings and
//! claimed paths, and merges the results of all rules. Rules are independent
//! and run in parallel (rayon) on a blocking thread, so the async runtime
//! stays free for the other collectors.
//!
//! Two rules giving the same `FindingId` keep one finding: the one from the
//! rule with the higher `priority` (on a tie, the rule that comes first in
//! the set, i.e. the smaller `id`); the evidence of the other is appended to
//! it (§4.5 step 2).
//!
//! Issues do not depend on thread scheduling: the issues of the rules come
//! in set order, then the issues of the conditions and the once-per-scan
//! issues (also an unreadable registry key of a target), sorted; a
//! once-per-scan issue names the earliest rule of the set that met it.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use rayon::prelude::*;
use sk_core::collector::{CollectContext, CollectOutput, Collector};
use sk_core::env::Environment;
use sk_core::error::CollectorError;
use sk_core::events::{Event, EventSink, ScanPhase, ThrottledSink};
use sk_core::fs::FsScanner;
use sk_core::model::{FindingId, ScanIssue};
use sk_core::template::ResolveContext;
use sk_core::CancellationToken;

use crate::compile::CompiledRule;
use crate::conditions::ConditionEvaluator;
use crate::expand::{RuleOutput, TargetExpander};
use crate::registry::{RegistryProbe, SystemRegistry};
use crate::set::RuleSet;

/// `Collector::id` of the rules collector.
const COLLECTOR_ID: &str = "rules";

/// i18n key of the collector name.
const DISPLAY_KEY: &str = "collector.rules";

/// A `Progress` event is sent after every this many rules, and after the
/// last one (§4.5 step 3); `ThrottledSink` limits the rate further.
const PROGRESS_EVERY: u64 = 10;

/// `id` of the launcher whose accounts give `{STEAM_USERID}` (SPEC-05).
const STEAM_LAUNCHER: &str = "steam";

/// Collector of findings from the known-location rules.
pub struct RulesCollector {
    set: Arc<RuleSet>,
    /// Registry access for `registry_exists` and `Registry` targets.
    registry: Arc<dyn RegistryProbe>,
}

impl RulesCollector {
    /// A collector over `set` that probes the registry of this machine
    /// ([`SystemRegistry`]).
    pub fn new(set: Arc<RuleSet>) -> Self {
        Self {
            set,
            registry: Arc::new(SystemRegistry),
        }
    }

    /// Replaces the registry, e.g. with a `MemRegistry` in tests.
    pub fn with_registry(mut self, registry: Arc<dyn RegistryProbe>) -> Self {
        self.registry = registry;
        self
    }
}

#[async_trait]
impl Collector for RulesCollector {
    fn id(&self) -> &'static str {
        COLLECTOR_ID
    }

    fn display_key(&self) -> &'static str {
        DISPLAY_KEY
    }

    /// Runs all rules; never fails. Problems of single rules are issues.
    /// When the scan is cancelled the remaining rules are skipped and the
    /// output is empty.
    async fn collect(&self, ctx: &CollectContext) -> Result<CollectOutput, CollectorError> {
        let set = Arc::clone(&self.set);
        let registry = Arc::clone(&self.registry);
        let ctx = ctx.clone();
        let task = tokio::task::spawn_blocking(move || {
            run(
                &set,
                &ctx.env,
                ctx.scanner.as_ref(),
                registry.as_ref(),
                &ctx.events,
                &ctx.cancel,
            )
        });
        match task.await {
            Ok(output) => Ok(output),
            // The engine reports a panicking collector (`collector.panicked`).
            Err(err) if err.is_panic() => std::panic::resume_unwind(err.into_panic()),
            Err(err) => Err(CollectorError::Other(format!(
                "rules task did not finish: {err}"
            ))),
        }
    }
}

/// Runs the rules of `set` and merges their results (§4.5).
pub(crate) fn run(
    set: &RuleSet,
    env: &Environment,
    fs: &dyn FsScanner,
    registry: &dyn RegistryProbe,
    events: &EventSink,
    cancel: &CancellationToken,
) -> CollectOutput {
    let resolve = resolve_context(env);
    // Once-per-scan issues name the earliest rule of the set that met them,
    // whatever thread got there first.
    let evaluator = ConditionEvaluator::new(env, fs, registry, &resolve)
        .rank_rules(set.rules().iter().map(CompiledRule::id));
    let expander = TargetExpander::from_evaluator(&evaluator);
    let sink = ThrottledSink::new(events.clone());
    let total = set.len() as u64;
    // Counting and sending under one lock keeps `done` increasing.
    let done = Mutex::new(0u64);

    let outputs: Vec<Option<RuleOutput>> = set
        .rules()
        .par_iter()
        .map(|rule| {
            if cancel.is_cancelled() {
                return None;
            }
            let output = expander.expand(rule, evaluator.evaluate(&rule.rule));
            let mut done = done.lock().unwrap_or_else(PoisonError::into_inner);
            *done += 1;
            if done.is_multiple_of(PROGRESS_EVERY) || *done == total {
                sink.send(Event::Progress {
                    phase: ScanPhase::Collect,
                    done: *done,
                    total: Some(total),
                    current: Some(rule.id().to_owned()),
                });
            }
            Some(output)
        })
        .collect();
    sink.flush();

    if cancel.is_cancelled() {
        return CollectOutput::default();
    }
    let results = set
        .rules()
        .iter()
        .zip(outputs)
        .filter_map(|(rule, output)| output.map(|output| (rule, output)));
    let mut output = merge(results);
    let mut shared = evaluator.take_issues();
    shared.sort_by(|a, b| issue_order(a).cmp(&issue_order(b)));
    output.issues.extend(shared);
    output
}

/// Sort key of the issues of conditions and other once-per-scan issues
/// (§4.5 step 2): `message_key`, `path`, `rule_id`, then the other args so
/// the order never depends on thread scheduling.
fn issue_order(issue: &ScanIssue) -> (&str, Option<&str>, Option<&str>, &BTreeMap<String, String>) {
    (
        &issue.message_key,
        issue.path.as_deref(),
        issue.message_args.get("rule_id").map(String::as_str),
        &issue.message_args,
    )
}

/// Values of the context tokens rules may use: `{STEAM_USERID}` is the id3
/// of every Steam account of the environment (SPEC-02 §3.2, SPEC-05).
fn resolve_context(env: &Environment) -> ResolveContext {
    ResolveContext {
        steam_user_ids: env
            .launchers
            .iter()
            .filter(|launcher| launcher.id == STEAM_LAUNCHER)
            .flat_map(|launcher| launcher.user_ids.iter().map(|user| user.id.clone()))
            .collect(),
        ..ResolveContext::default()
    }
}

/// Joins the outputs of the rules, in set order. A `FindingId` given by
/// several rules keeps the finding of the rule with the higher priority (the
/// earlier one on a tie) with the evidence of the others appended; claimed
/// paths are kept once each.
fn merge<'a>(results: impl Iterator<Item = (&'a CompiledRule, RuleOutput)>) -> CollectOutput {
    let mut out = CollectOutput::default();
    // Finding id → (index in `out.findings`, priority of its rule).
    let mut kept: HashMap<FindingId, (usize, i32)> = HashMap::new();
    let mut claimed = HashSet::new();
    for (rule, output) in results {
        let priority = rule.rule.priority;
        for finding in output.findings {
            let existing = kept.get_mut(&finding.id).and_then(|(idx, kept_priority)| {
                out.findings
                    .get_mut(*idx)
                    .map(|found| (found, kept_priority))
            });
            match existing {
                None => {
                    kept.insert(finding.id.clone(), (out.findings.len(), priority));
                    out.findings.push(finding);
                }
                Some((found, kept_priority)) if priority > *kept_priority => {
                    let loser = std::mem::replace(found, finding);
                    found.evidence.extend(loser.evidence);
                    *kept_priority = priority;
                }
                Some((found, _)) => found.evidence.extend(finding.evidence),
            }
        }
        for path in output.claimed_paths {
            if claimed.insert(path.clone()) {
                out.claimed_paths.push(path);
            }
        }
        out.issues.extend(output.issues);
    }
    out
}

#[cfg(test)]
#[path = "collector_tests.rs"]
mod tests;
