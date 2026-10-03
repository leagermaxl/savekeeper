//! Scan pipeline and backup job orchestration (SPEC-01 §4.4).

mod pipeline;
mod reports;
mod rules;

pub use pipeline::{ScanOptions, ScanPipeline};
pub use sk_core::error::EngineError;
