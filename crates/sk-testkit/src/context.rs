//! Collector context and event helpers (SPEC-12 §4.2).

use std::sync::Arc;

use sk_core::collector::CollectContext;
use sk_core::config::Config;
use sk_core::events::Event;
use sk_core::fs::FsScanner;
use sk_core::CancellationToken;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

use crate::FakeProfile;

/// A collector context over `profile` and the receiver of its events.
///
/// The scanner is passed explicitly (`MemFs` or `RealFs`); the cancellation
/// token is fresh and not cancelled.
pub fn collect_ctx(
    profile: &FakeProfile,
    config: Config,
    scanner: Arc<dyn FsScanner>,
) -> (CollectContext, UnboundedReceiver<Event>) {
    let (events, rx) = unbounded_channel();
    let ctx = CollectContext {
        env: Arc::new(profile.env.clone()),
        config: Arc::new(config),
        scanner,
        events,
        cancel: CancellationToken::new(),
    };
    (ctx, rx)
}

/// All events received so far, without waiting for more.
pub fn drain_events(rx: &mut UnboundedReceiver<Event>) -> Vec<Event> {
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    events
}
