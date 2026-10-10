//! A single heuristic and what it sees and returns (SPEC-07 §4.1).

use std::path::PathBuf;

use sk_core::config::HeuristicsConfig;
use sk_core::env::Environment;
use sk_core::fs::FsScanner;
use sk_core::model::{Finding, FolderSummary, ScanIssue};
use sk_core::path::PathSet;
use sk_core::CancellationToken;

/// One heuristic of the collector: `h-unk`, `h-usr`, `h-git`, `h-junk` or
/// `h-web`. Internal to the collector; public for tests.
pub trait Heuristic: Send + Sync {
    /// Stable identifier: "h-unk", "h-usr", "h-git", "h-junk", "h-web".
    fn id(&self) -> &'static str;

    /// Looks for findings. Synchronous: the collector calls it on a blocking
    /// thread (`spawn_blocking`). On cancellation it returns what it has so far.
    fn run(&self, hctx: &HeuristicContext<'_>) -> HeuristicOutput;
}

/// Everything a heuristic may use.
pub struct HeuristicContext<'a> {
    /// Known folders, drives, launchers, installed programs.
    pub env: &'a Environment,
    /// File system access (SPEC-03 §4.1).
    pub scanner: &'a dyn FsScanner,
    /// Paths claimed by the collectors and by the heuristics that ran before.
    pub claimed: &'a PathSet,
    /// Findings of the collectors and of the heuristics that ran before.
    pub prior: &'a [Finding],
    /// Configuration of the heuristics (SPEC-07 §4.9).
    pub cfg: &'a HeuristicsConfig,
    /// Cancellation of the scan.
    pub cancel: &'a CancellationToken,
    /// Progress of the `Heuristics` phase: entries done and the current item.
    pub progress: &'a dyn Fn(u64, Option<&str>),
}

/// What a heuristic found.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HeuristicOutput {
    /// Findings.
    pub findings: Vec<Finding>,
    /// Summaries of the `Unknown` findings, for the LLM (SPEC-08).
    pub unknown: Vec<FolderSummary>,
    /// Paths explained by this heuristic, so that the next ones skip them.
    pub claimed: Vec<PathBuf>,
    /// Problems and notes.
    pub issues: Vec<ScanIssue>,
}
