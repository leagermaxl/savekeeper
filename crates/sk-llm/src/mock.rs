//! `MockClassifier`: a scripted [`Classifier`] for tests (SPEC-08 §4.2.4).
//!
//! Public under the `mock` feature; always available in this crate's tests.
//! Answers come from a table keyed by the item's `summary.path`; delays,
//! provider errors (429, 529, invalid JSON ...) and cancellation are simulated.

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use sk_core::template::PathTemplate;
use sk_core::CancellationToken;

use crate::{Classification, Classifier, ClassifierInfo, ClassifyItem, HealthReport, LlmError};

/// A provider failure the mock returns instead of answering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MockFailure {
    /// HTTP 429 with an optional `retry-after`.
    RateLimited(Option<Duration>),
    /// HTTP 529.
    Overloaded,
    /// The model answered with something that is not valid JSON.
    InvalidJson,
    /// Connection refused.
    Unreachable,
    /// HTTP 401/403.
    Auth,
    /// The model is not installed on the provider.
    ModelNotFound,
    /// The request timed out.
    Timeout,
}

impl MockFailure {
    fn into_error(self, model: &str) -> LlmError {
        match self {
            Self::RateLimited(after) => LlmError::RateLimited(after),
            Self::Overloaded => LlmError::Overloaded,
            Self::InvalidJson => LlmError::BadResponse("invalid JSON".to_owned()),
            Self::Unreachable => LlmError::Unreachable("mock".to_owned()),
            Self::Auth => LlmError::Auth,
            Self::ModelNotFound => LlmError::ModelNotFound(model.to_owned()),
            Self::Timeout => LlmError::Timeout,
        }
    }
}

/// Scripted classifier.
///
/// - An item whose `summary.path` is in the table gets that answer with the
///   item's `id` and `from_cache = false`; other items are left out of the
///   result, as a model that skipped an id would.
/// - Failures queued with [`fail_next`](Self::fail_next) are returned by the
///   next `classify` calls, one per call, in order.
/// - A batch larger than `max_batch` is a [`LlmError::BadResponse`].
/// - Cancellation before or during the simulated delay is [`LlmError::Cancelled`].
#[derive(Debug)]
pub struct MockClassifier {
    responses: HashMap<PathTemplate, Classification>,
    model: String,
    is_local: bool,
    max_batch: usize,
    max_concurrency: usize,
    delay: Duration,
    health_failure: Option<MockFailure>,
    failures: Mutex<VecDeque<MockFailure>>,
    calls: Mutex<Vec<Vec<String>>>,
}

impl Default for MockClassifier {
    fn default() -> Self {
        Self::new()
    }
}

impl MockClassifier {
    /// Empty table, model `"mock"`, local, `max_batch = 8`, `max_concurrency = 1`, no delay.
    pub fn new() -> Self {
        Self {
            responses: HashMap::new(),
            model: "mock".to_owned(),
            is_local: true,
            max_batch: 8,
            max_concurrency: 1,
            delay: Duration::ZERO,
            health_failure: None,
            failures: Mutex::new(VecDeque::new()),
            calls: Mutex::new(Vec::new()),
        }
    }

    /// Answer `classification` for items whose summary path is `path`.
    #[must_use]
    pub fn with_response(mut self, path: PathTemplate, classification: Classification) -> Self {
        self.responses.insert(path, classification);
        self
    }

    /// Model id reported by [`Classifier::info`].
    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Report the classifier as local or cloud.
    #[must_use]
    pub fn with_local(mut self, is_local: bool) -> Self {
        self.is_local = is_local;
        self
    }

    /// Batch size limit (at least 1).
    #[must_use]
    pub fn with_max_batch(mut self, max_batch: usize) -> Self {
        self.max_batch = max_batch.max(1);
        self
    }

    /// Concurrency limit (at least 1).
    #[must_use]
    pub fn with_max_concurrency(mut self, max_concurrency: usize) -> Self {
        self.max_concurrency = max_concurrency.max(1);
        self
    }

    /// Simulated response time of every `classify` call.
    #[must_use]
    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    /// Make [`Classifier::health`] fail with `failure`.
    #[must_use]
    pub fn with_health_failure(mut self, failure: MockFailure) -> Self {
        self.health_failure = Some(failure);
        self
    }

    /// Queue a failure for the next `classify` call.
    pub fn fail_next(&self, failure: MockFailure) {
        lock(&self.failures).push_back(failure);
    }

    /// Ids of every `classify` call that reached the provider, in call order.
    pub fn calls(&self) -> Vec<Vec<String>> {
        lock(&self.calls).clone()
    }

    /// Number of `classify` calls that reached the provider.
    pub fn call_count(&self) -> usize {
        lock(&self.calls).len()
    }

    async fn wait(&self, cancel: &CancellationToken) -> Result<(), LlmError> {
        if self.delay.is_zero() {
            return Ok(());
        }
        tokio::select! {
            () = cancel.cancelled() => Err(LlmError::Cancelled),
            () = tokio::time::sleep(self.delay) => Ok(()),
        }
    }
}

#[async_trait]
impl Classifier for MockClassifier {
    fn info(&self) -> ClassifierInfo {
        ClassifierInfo {
            provider: "mock",
            model: self.model.clone(),
            is_local: self.is_local,
            max_batch: self.max_batch,
            max_concurrency: self.max_concurrency,
        }
    }

    async fn health(&self) -> Result<HealthReport, LlmError> {
        match &self.health_failure {
            Some(failure) => Err(failure.clone().into_error(&self.model)),
            None => Ok(HealthReport {
                model_available: true,
                latency_ms: u64::try_from(self.delay.as_millis()).unwrap_or(u64::MAX),
                server_version: Some("mock".to_owned()),
            }),
        }
    }

    async fn classify(
        &self,
        items: &[ClassifyItem],
        cancel: &CancellationToken,
    ) -> Result<Vec<Classification>, LlmError> {
        if cancel.is_cancelled() {
            return Err(LlmError::Cancelled);
        }
        lock(&self.calls).push(items.iter().map(|item| item.id.clone()).collect());
        let failure = lock(&self.failures).pop_front();
        self.wait(cancel).await?;
        if let Some(failure) = failure {
            return Err(failure.into_error(&self.model));
        }
        if items.len() > self.max_batch {
            return Err(LlmError::BadResponse(format!(
                "batch of {} items exceeds max_batch {}",
                items.len(),
                self.max_batch
            )));
        }
        Ok(items
            .iter()
            .filter_map(|item| {
                self.responses
                    .get(&item.summary.path)
                    .map(|answer| Classification {
                        id: item.id.clone(),
                        from_cache: false,
                        ..answer.clone()
                    })
            })
            .collect())
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
#[path = "mock_tests.rs"]
mod tests;
