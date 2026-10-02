//! Domain types, path templates, environment, config, events and errors (SPEC-01, SPEC-02).

pub mod config;
pub mod env;
pub mod events;
pub mod fs;
pub mod model;
pub mod path;
pub mod privacy;
pub mod template;

mod serde_util;
#[cfg(windows)]
mod win;

/// Cancellation of long operations (SPEC-01 §4.6).
pub use tokio_util::sync::CancellationToken;
