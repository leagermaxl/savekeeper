//! Heuristics for unknown folders, git repositories, user files and junk (SPEC-07).
//!
//! [`HeuristicsCollector`] is the `heuristics` post-collector of the scan
//! pipeline (SPEC-01 §4.3): it runs the single [`Heuristic`]s one after
//! another, in the order `h-git` → `h-web` → `h-junk` → `h-unk` → `h-usr`,
//! each seeing the paths claimed so far (SPEC-07 §4.1).

mod classify;
mod collector;
mod data;
mod finding;
mod heuristic;
mod junk;
mod unk;
mod unk_zones;
mod verdict;
mod web;

pub use classify::{classify_by_markers, classify_by_markers_with};
pub use collector::HeuristicsCollector;
pub use heuristic::{Heuristic, HeuristicContext, HeuristicOutput};
pub use junk::{junk_verdict, junk_verdict_with_siblings};
/// Configuration of the heuristics; defined in `sk-core::config` (SPEC-07 §4.9).
pub use sk_core::config::HeuristicsConfig;
pub use verdict::MarkerVerdict;
