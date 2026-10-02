//! File system traversal, measurement, folder summaries and exclusions (SPEC-03).
//!
//! The `FsScanner` trait and its types are defined in `sk-core::fs` and
//! re-exported here, next to their implementations: [`MemFs`] for tests;
//! `RealFs`, measurement and summaries follow in later tasks of SPEC-03.

mod mem;

pub use mem::{MemFs, MemFsCalls};
pub use sk_core::fs::{
    CloudState, DirEntryInfo, EntryKind, EntryMeta, FsError, FsScanner, PathFilter, Readability,
    ReparseKind, WalkControl, WalkOptions, WalkStats,
};
pub use sk_core::CancellationToken;
