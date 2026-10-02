//! Test infrastructure: fixtures and fakes; use only as a dev-dependency (SPEC-12).
//!
//! - [`FakeProfile`]: a user profile from `fixtures/profiles/<name>.yaml`
//!   materialized in a temporary folder, with a matching `Environment::fake`.
//! - [`materialize_profile`]: the same into a given folder (`cargo xtask fixtures`).
//! - [`mem_fixture`]: a `fixtures/fs/<name>.yaml` tree in a `sk_scan::MemFs`, without disk.
//! - [`collect_ctx`], [`drain_events`]: a collector context and its events.
//! - [`tree_hash`]: content hashes of a folder tree.
//! - `RegTestKey` (Windows): a registry key deleted on drop.
//!
//! Helpers panic on failure: they are meant for tests only.

mod cache;
mod context;
mod fixture;
mod fixture_fs;
mod git;
mod materialize;
mod mem;
mod profile;
#[cfg(windows)]
mod reg;
mod tree;

pub use context::{collect_ctx, drain_events};
pub use materialize::materialize_profile;
pub use mem::mem_fixture;
pub use profile::FakeProfile;
#[cfg(windows)]
pub use reg::{RegTestKey, REG_TEST_PARENT};
pub use tree::tree_hash;

#[cfg(test)]
mod tests;
