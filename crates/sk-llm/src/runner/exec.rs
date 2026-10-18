//! One batch against the provider (SPEC-08 §4.6, §4.12, §5): retries with
//! backoff, one retry of an invalid answer, one mini-batch for missing ids,
//! halving on `max_tokens`.

use std::collections::hash_map::RandomState;
use std::collections::{HashSet, VecDeque};
use std::hash::{BuildHasher, Hasher};
use std::time::Duration;

use sk_core::CancellationToken;

use crate::{Classification, Classifier, ClassifyItem, LlmError};

/// Retries of a failed request after the first attempt (backoff `1 s, 2 s, 4 s`).
pub const MAX_RETRIES: u32 = 3;
/// First backoff delay; doubled on every retry, ±20% jitter.
pub const BACKOFF_BASE: Duration = Duration::from_secs(1);
/// Upper bound of a `retry-after` delay that is honoured.
pub const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);
/// Relative jitter of a backoff delay.
const JITTER: f64 = 0.2;

/// Why items of a batch got no result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Failure {
    /// 401/403.
    Auth,
    /// No connection, after retries.
    Unreachable(String),
    /// The model is not installed / not known to the provider.
    ModelMissing,
    /// `stop_reason: refusal`.
    Refusal,
    /// `max_tokens` even for a single item.
    Truncated,
    /// The answer was not valid JSON, also on the retry.
    BadJson(String),
    /// Other request failure (HTTP 4xx, 429/5xx/timeout after retries).
    Request(String),
    /// The model skipped the id, also in the mini-batch.
    Missing,
}

/// What became of one batch.
#[derive(Debug, Default)]
pub(crate) struct BatchOutcome {
    /// Answers for the sent items (their ids), `from_cache = false`.
    pub results: Vec<Classification>,
    /// Items without an answer, by reason.
    pub failures: Vec<(Failure, Vec<String>)>,
    /// The cancellation token fired.
    pub cancelled: bool,
}

impl BatchOutcome {
    /// The batch got nothing and ended with `Unreachable`/`Auth` (§4.12:
    /// three such batches in a row stop the phase).
    pub fn provider_failed(&self) -> bool {
        self.results.is_empty()
            && self
                .failures
                .iter()
                .any(|(f, _)| matches!(f, Failure::Auth | Failure::Unreachable(_)))
    }
}

/// Classifies `items` (at most `max_batch`), splitting and re-asking as needed.
pub(crate) async fn run_batch(
    classifier: &dyn Classifier,
    items: Vec<ClassifyItem>,
    cancel: &CancellationToken,
) -> BatchOutcome {
    let mut out = BatchOutcome::default();
    // (items, already re-asked for missing ids)
    let mut jobs: VecDeque<(Vec<ClassifyItem>, bool)> = VecDeque::from([(items, false)]);
    while let Some((job, reasked)) = jobs.pop_front() {
        match call(classifier, &job, cancel).await {
            Ok(answers) => {
                let wanted: HashSet<String> = job.iter().map(|i| i.id.clone()).collect();
                let mut seen: HashSet<String> = HashSet::new();
                for answer in answers {
                    // Foreign and repeated ids are ignored (§5).
                    if wanted.contains(&answer.id) && seen.insert(answer.id.clone()) {
                        out.results.push(Classification {
                            from_cache: false,
                            ..answer
                        });
                    }
                }
                let missing: Vec<ClassifyItem> =
                    job.into_iter().filter(|i| !seen.contains(&i.id)).collect();
                if missing.is_empty() {
                    continue;
                }
                if reasked {
                    out.failures.push((Failure::Missing, ids(&missing)));
                } else {
                    // §4.6 step 2: one mini-batch for the missing ids.
                    jobs.push_back((missing, true));
                }
            }
            Err(LlmError::MaxTokens) if job.len() > 1 => {
                // §5: halve and retry both parts, recursively down to one item.
                let mut first = job;
                let second = first.split_off(first.len() / 2);
                jobs.push_front((second, reasked));
                jobs.push_front((first, reasked));
            }
            Err(LlmError::Cancelled) => {
                out.cancelled = true;
                return out;
            }
            Err(e) => out.failures.push((failure_of(&e), ids(&job))),
        }
    }
    out
}

/// One request with retries: transient errors get up to [`MAX_RETRIES`]
/// retries with backoff, an invalid answer one immediate retry (§4.6 step 1).
async fn call(
    classifier: &dyn Classifier,
    items: &[ClassifyItem],
    cancel: &CancellationToken,
) -> Result<Vec<Classification>, LlmError> {
    let mut retries = 0u32;
    let mut answer_retried = false;
    loop {
        let err = match classifier.classify(items, cancel).await {
            Ok(answers) => return Ok(answers),
            Err(e) => e,
        };
        match &err {
            LlmError::BadResponse(msg) if !is_http_error(msg) && !answer_retried => {
                answer_retried = true;
            }
            e if is_transient(e) && retries < MAX_RETRIES => {
                let delay = retry_delay(e, retries);
                retries += 1;
                tokio::select! {
                    () = cancel.cancelled() => return Err(LlmError::Cancelled),
                    () = tokio::time::sleep(delay) => {}
                }
            }
            _ => return Err(err),
        }
    }
}

/// Errors worth retrying after a pause.
fn is_transient(e: &LlmError) -> bool {
    matches!(
        e,
        LlmError::RateLimited(_)
            | LlmError::Overloaded
            | LlmError::Timeout
            | LlmError::Unreachable(_)
    )
}

/// `BadResponse("HTTP n: …")` (§4.12) is a rejected request, not an invalid
/// answer: it is not retried.
fn is_http_error(msg: &str) -> bool {
    msg.starts_with("HTTP ")
}

/// Delay before retry number `retry + 1`: `retry-after` (capped at
/// [`MAX_RETRY_AFTER`]) for a 429 that has one, else exponential backoff.
pub(crate) fn retry_delay(e: &LlmError, retry: u32) -> Duration {
    if let LlmError::RateLimited(Some(after)) = e {
        return (*after).min(MAX_RETRY_AFTER);
    }
    let base = BACKOFF_BASE.saturating_mul(1u32 << retry.min(16));
    base.mul_f64(1.0 - JITTER + 2.0 * JITTER * random_unit())
}

/// A pseudo-random number in `[0, 1]` from the randomly keyed std hasher.
fn random_unit() -> f64 {
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u8(0);
    #[allow(clippy::cast_precision_loss)] // only a jitter
    let unit = hasher.finish() as f64 / u64::MAX as f64;
    unit
}

fn failure_of(e: &LlmError) -> Failure {
    match e {
        LlmError::Auth => Failure::Auth,
        LlmError::Unreachable(msg) => Failure::Unreachable(msg.clone()),
        LlmError::ModelNotFound(_) => Failure::ModelMissing,
        LlmError::Refusal => Failure::Refusal,
        LlmError::MaxTokens => Failure::Truncated,
        LlmError::BadResponse(msg) if !is_http_error(msg) => Failure::BadJson(msg.clone()),
        other => Failure::Request(other.to_string()),
    }
}

fn ids(items: &[ClassifyItem]) -> Vec<String> {
    items.iter().map(|i| i.id.clone()).collect()
}
