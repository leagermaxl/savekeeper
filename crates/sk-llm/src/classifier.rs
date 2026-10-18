//! The `Classifier` trait (SPEC-08 §4.1).

use async_trait::async_trait;
use sk_core::CancellationToken;

use crate::{Classification, ClassifierInfo, ClassifyItem, HealthReport, LlmError};

/// An LLM provider that classifies batches of folder summaries.
#[async_trait]
pub trait Classifier: Send + Sync {
    /// Provider, model and batching limits.
    fn info(&self) -> ClassifierInfo;

    /// Checks that the provider is reachable and the model is available (FR-08-08).
    async fn health(&self) -> Result<HealthReport, LlmError>;

    /// Classifies ONE batch (at most `info().max_batch` items).
    ///
    /// The order of the results is not guaranteed: they are matched by `id`.
    /// Ids the model skipped are simply missing from the result.
    async fn classify(
        &self,
        items: &[ClassifyItem],
        cancel: &CancellationToken,
    ) -> Result<Vec<Classification>, LlmError>;
}
