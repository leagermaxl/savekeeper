//! Events and cancellation of one run of the pipeline (SPEC-01 §4.4–§4.6).

use std::future::Future;
use std::time::Instant;

use sk_core::error::EngineError;
use sk_core::events::{Event, EventSink, ScanPhase};
use sk_core::CancellationToken;

/// Events and cancellation of one run.
pub(super) struct Run<'a> {
    pub(super) events: &'a EventSink,
    pub(super) cancel: &'a CancellationToken,
}

impl Run<'_> {
    pub(super) fn send(&self, event: Event) {
        // Nobody listening is not an error.
        let _ = self.events.send(event);
    }

    /// Runs `work` as `phase`: events around it, cancellation before and during it.
    pub(super) async fn phase<T>(
        &self,
        phase: ScanPhase,
        work: impl Future<Output = T>,
    ) -> Result<T, EngineError> {
        if self.cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        self.send(Event::PhaseStarted { phase });
        let start = Instant::now();
        let out = tokio::select! {
            biased;
            () = self.cancel.cancelled() => return Err(EngineError::Cancelled),
            out = work => out,
        };
        let elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.send(Event::PhaseFinished { phase, elapsed_ms });
        Ok(out)
    }
}
