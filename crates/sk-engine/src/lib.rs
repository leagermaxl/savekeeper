//! Scan pipeline and backup job orchestration (SPEC-01 §4.4).

mod pipeline;
mod reports;
mod unavailable_fs;

pub use pipeline::{ScanOptions, ScanPipeline};
pub use sk_core::error::EngineError;
