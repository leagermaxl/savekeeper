//! File system traversal, measurement, folder summaries and exclusions (SPEC-03).
//!
//! The `FsScanner` trait and its types are defined in `sk-core::fs` and
//! re-exported here, next to their implementations: [`RealFs`] for the real
//! file system, [`MemFs`] for tests and [`ExcludeSet`] for global exclusions;
//! [`measure`] and [`measure_all`] fill `TargetStats` of findings;
//! [`summarize`] builds a `FolderSummary` with markers.

mod exclude;
mod measure;
mod mem;
mod real;
mod summary;
mod win;

pub use exclude::ExcludeSet;
pub use measure::{measure, measure_all, DirStatsCache, MeasureOptions};
pub use mem::{MemFs, MemFsCalls};
pub use real::RealFs;
pub use sk_core::fs::{
    CloudState, DirEntryInfo, EntryKind, EntryMeta, Exclusion, FsError, FsScanner, PathFilter,
    Readability, ReparseKind, WalkControl, WalkOptions, WalkStats,
};
pub use sk_core::CancellationToken;
pub use summary::{summarize, SummaryOptions};
