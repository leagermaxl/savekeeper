//! Loading the manifest ahead of the collection (SPEC-05 T-05-15, NFR-05-01):
//! the engine starts [`GamesCollector::preload`] at the start of the
//! `Environment` phase, and `collect` awaits that load instead of loading
//! the manifest again.

use std::sync::{Arc, PoisonError};

use sk_core::CancellationToken;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;

use super::GamesCollector;
use crate::manifest::Manifest;
use crate::GamesError;

/// A running [`ManifestStore::load`](crate::ManifestStore::load); dropping
/// it aborts the task (the scan was dropped before the collection).
pub(super) struct Preload {
    task: JoinHandle<Result<Arc<Manifest>, GamesError>>,
}

impl Drop for Preload {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl GamesCollector {
    /// Starts loading the manifest in a separate task, so that the next
    /// [`collect`](sk_core::collector::Collector::collect) uses it instead of
    /// loading it itself (SPEC-05 §4.7 step 1). `cancel` stops the load as it
    /// stops the load of `collect`. Nothing happens when a preload is already
    /// pending or outside a Tokio runtime (then `collect` loads the manifest).
    pub fn preload(&self, cancel: &CancellationToken) {
        let Ok(runtime) = Handle::try_current() else {
            return;
        };
        let mut slot = self.preload.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.is_some() {
            return;
        }
        let store = Arc::clone(&self.store);
        let allow_network = self.allow_network;
        let cancel = cancel.clone();
        let task = runtime.spawn(async move { store.load(allow_network, &cancel).await });
        *slot = Some(Preload { task });
    }

    /// The manifest of one collection: the result of the pending preload,
    /// else a load now. A cancelled `cancel` while waiting gives
    /// [`GamesError::Cancelled`]; a panic of the preload task is resumed.
    pub(super) async fn manifest(
        &self,
        cancel: &CancellationToken,
    ) -> Result<Arc<Manifest>, GamesError> {
        let pending = self
            .preload
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let Some(mut pending) = pending else {
            return self.store.load(self.allow_network, cancel).await;
        };
        let joined = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(GamesError::Cancelled),
            joined = &mut pending.task => joined,
        };
        match joined {
            Ok(result) => result,
            Err(err) if err.is_panic() => std::panic::resume_unwind(err.into_panic()),
            // Aborted from outside the collector: as a cancelled load.
            Err(_) => Err(GamesError::Cancelled),
        }
    }
}
