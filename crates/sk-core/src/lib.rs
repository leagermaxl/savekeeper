//! Domain types, path templates, environment, registry access, config, events and errors
//! (SPEC-01, SPEC-02).

pub mod collector;
pub mod config;
pub mod env;
pub mod error;
pub mod events;
pub mod fs;
pub mod logging;
pub mod model;
pub mod path;
pub mod privacy;
pub mod registry;
pub mod template;
pub mod win;

mod serde_util;

/// Cancellation of long operations (SPEC-01 §4.6).
pub use tokio_util::sync::CancellationToken;
