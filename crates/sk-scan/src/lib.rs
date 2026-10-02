//! File system traversal, measurement, folder summaries and exclusions (SPEC-03).
//!
//! The `FsScanner` trait and its types are defined in `sk-core::fs` and
//! re-exported here; implementations follow in later tasks of SPEC-03.

pub use sk_core::fs::{
    CloudState, DirEntryInfo, EntryKind, EntryMeta, FsError, FsScanner, PathFilter, Readability,
    ReparseKind, WalkControl, WalkOptions, WalkStats,
};
pub use sk_core::CancellationToken;
