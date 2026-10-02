//! Test infrastructure: fixtures and fakes; use only as a dev-dependency (SPEC-12).
//!
//! - [`FakeProfile`]: a user profile from `fixtures/profiles/<name>.yaml`
//!   materialized in a temporary folder, with a matching `Environment::fake`.
//! - [`collect_ctx`], [`drain_events`]: a collector context and its events.
//! - [`tree_hash`]: content hashes of a folder tree.
//! - `RegTestKey` (Windows): a registry key deleted on drop.
//!
//! Helpers panic on failure: they are meant for tests only.

mod context;
mod fixture;
mod profile;
#[cfg(windows)]
mod reg;
mod tree;

pub use context::{collect_ctx, drain_events};
pub use profile::FakeProfile;
#[cfg(windows)]
pub use reg::{RegTestKey, REG_TEST_PARENT};
pub use tree::tree_hash;

#[cfg(test)]
mod tests;
