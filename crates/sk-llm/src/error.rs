//! Errors of classifiers and providers (SPEC-08 §4.1).

use std::time::Duration;

/// Error of a [`Classifier`](crate::Classifier) call.
///
/// Messages never contain the API key (NFR-08-04).
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum LlmError {
    /// The provider cannot be reached (connection refused, DNS ...).
    #[error("provider unreachable: {0}")]
    Unreachable(String),
    /// The configured model is not available on the provider.
    #[error("model not found: {0}")]
    ModelNotFound(String),
    /// The API key was rejected (401/403); never retried.
    #[error("auth failed")]
    Auth,
    /// Rate limit (429), with the `retry-after` delay if the provider sent one.
    #[error("rate limited, retry after {0:?}")]
    RateLimited(Option<Duration>),
    /// The provider is overloaded (529).
    #[error("overloaded")]
    Overloaded,
    /// The response is not what was expected (invalid JSON, HTTP 400 ...).
    #[error("bad response: {0}")]
    BadResponse(String),
    /// The request took longer than `timeout_s`.
    #[error("timeout")]
    Timeout,
    /// The cancellation token fired.
    #[error("cancelled")]
    Cancelled,
}
