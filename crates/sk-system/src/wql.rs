//! WMI queries for exporters (SPEC-06 §4.3: `printers`, `disks-info`, `windows-info`).
//!
//! Exporters query WMI through [`WmiSource`], so tests replace it with [`FakeWmi`]
//! (SPEC-06 §6). The real source, [`SystemWmi`], runs every query on a thread of its
//! own: COM is initialized there in the multithreaded apartment (no clash with the
//! apartment of the caller's thread), and the caller waits at most the given timeout —
//! the connection to the WMI service can take seconds when it starts cold. A query that
//! times out keeps running on its thread until WMI answers; its result is dropped.
//! Queries only read (principle P1).

#[cfg(windows)]
mod win;

use std::collections::BTreeMap;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use sk_core::CancellationToken;

use crate::error::ExportError;

/// The default WMI namespace (`Win32_*` classes, `SoftwareLicensing*`).
pub(crate) const CIMV2: &str = r"ROOT\CIMV2";

/// One value of a WMI property, reduced to what the exporters read.
// Built by the Windows source (and the test fake) only.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WmiValue {
    /// No value (`NULL` or empty).
    Null,
    /// A boolean.
    Bool(bool),
    /// An integer of any width (unsigned 64-bit values above `i64::MAX` saturate).
    Int(i64),
    /// A string.
    Text(String),
    /// Anything else (arrays, reals, objects): not read by the exporters.
    Other,
}

/// One result object of a query: property name → value.
pub(crate) type WmiRow = BTreeMap<String, WmiValue>;

/// The string property `name` of `row`; `None` if missing, `NULL` or not a string.
pub(crate) fn text<'a>(row: &'a WmiRow, name: &str) -> Option<&'a str> {
    match row.get(name) {
        Some(WmiValue::Text(s)) => Some(s),
        _ => None,
    }
}

/// The integer property `name` of `row`; `None` if missing, `NULL` or not an integer.
pub(crate) fn int(row: &WmiRow, name: &str) -> Option<i64> {
    match row.get(name) {
        Some(WmiValue::Int(n)) => Some(*n),
        _ => None,
    }
}

/// Why a WMI query gave no result.
// `AccessDenied` comes from the Windows source (and the test fake) only.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum WmiError {
    /// No answer within the timeout.
    #[error("WMI query timed out")]
    Timeout,
    /// The query was cancelled (principle P8).
    #[error("WMI query was cancelled")]
    Cancelled,
    /// WMI refused the query: the class needs administrator rights.
    #[error("WMI access denied")]
    AccessDenied,
    /// There is no WMI on this OS.
    #[error("WMI is not available on this OS")]
    Unsupported,
    /// Any other failure (unknown namespace or class, COM error), with its text.
    #[error("WMI query failed: {0}")]
    Failed(String),
}

impl From<WmiError> for ExportError {
    fn from(err: WmiError) -> Self {
        match err {
            WmiError::Timeout => ExportError::Timeout,
            WmiError::Cancelled => ExportError::Cancelled,
            WmiError::AccessDenied => ExportError::Io(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                err.to_string(),
            )),
            WmiError::Unsupported => ExportError::NotAvailable,
            WmiError::Failed(_) => ExportError::Io(std::io::Error::other(err.to_string())),
        }
    }
}

/// Runs WQL queries; replaced by [`FakeWmi`] in tests.
pub(crate) trait WmiSource: Send + Sync {
    /// Runs `query` in `namespace` and returns all result objects, waiting at most
    /// `timeout`. Blocking: call it from `detect` or through [`query_async`].
    fn query(
        &self,
        namespace: &str,
        query: &str,
        timeout: Duration,
    ) -> Result<Vec<WmiRow>, WmiError>;
}

/// The WMI of this machine (Windows); [`WmiError::Unsupported`] elsewhere.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SystemWmi;

impl WmiSource for SystemWmi {
    fn query(
        &self,
        namespace: &str,
        query: &str,
        timeout: Duration,
    ) -> Result<Vec<WmiRow>, WmiError> {
        if !cfg!(windows) {
            return Err(WmiError::Unsupported);
        }
        let started = Instant::now();
        let (tx, rx) = mpsc::sync_channel(1);
        let (ns, wql) = (namespace.to_owned(), query.to_owned());
        let spawned = std::thread::Builder::new()
            .name("sk-system-wmi".to_owned())
            .spawn(move || {
                // The receiver may be gone after a timeout: the result is dropped.
                let _ = tx.send(query_on_this_thread(&ns, &wql));
            });
        if let Err(err) = spawned {
            return Err(WmiError::Failed(err.to_string()));
        }
        let result = match rx.recv_timeout(timeout) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => Err(WmiError::Timeout),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(WmiError::Failed("WMI worker thread panicked".to_owned()))
            }
        };
        let elapsed_ms = started.elapsed().as_millis();
        match &result {
            Ok(rows) => {
                tracing::debug!(namespace, query, rows = rows.len(), elapsed_ms, "WMI query")
            }
            Err(err) => {
                tracing::debug!(namespace, query, elapsed_ms, error = %err, "WMI query failed")
            }
        }
        result
    }
}

/// The query on the calling thread: the real one on Windows.
#[cfg(windows)]
fn query_on_this_thread(namespace: &str, query: &str) -> Result<Vec<WmiRow>, WmiError> {
    win::query(namespace, query)
}

/// Outside Windows there is no WMI.
#[cfg(not(windows))]
fn query_on_this_thread(_namespace: &str, _query: &str) -> Result<Vec<WmiRow>, WmiError> {
    Err(WmiError::Unsupported)
}

/// [`WmiSource::query`] from async code (`run`): the blocking query runs on the blocking
/// pool and is abandoned on cancellation ([`WmiError::Cancelled`]); its own timeout still
/// bounds the blocking thread.
pub(crate) async fn query_async(
    source: Arc<dyn WmiSource>,
    namespace: &'static str,
    query: &'static str,
    timeout: Duration,
    cancel: &CancellationToken,
) -> Result<Vec<WmiRow>, WmiError> {
    if cancel.is_cancelled() {
        return Err(WmiError::Cancelled);
    }
    let task = tokio::task::spawn_blocking(move || source.query(namespace, query, timeout));
    tokio::select! {
        biased;
        () = cancel.cancelled() => Err(WmiError::Cancelled),
        joined = task => match joined {
            Ok(result) => result,
            Err(err) => Err(WmiError::Failed(err.to_string())),
        },
    }
}

#[cfg(test)]
pub(crate) use fake::FakeWmi;

#[cfg(test)]
mod fake {
    use std::sync::Mutex;

    use super::*;

    /// A prepared answer of [`FakeWmi`].
    type Answer = Result<Vec<WmiRow>, WmiError>;

    /// Fake WMI: answers queries that mention a class name with prepared rows or an
    /// error, and records the queries. A query without an answer fails.
    #[derive(Default)]
    pub(crate) struct FakeWmi {
        answers: Vec<(String, Answer)>,
        calls: Mutex<Vec<(String, String)>>,
    }

    impl FakeWmi {
        pub(crate) fn new() -> Self {
            Self::default()
        }

        /// Answers queries containing `class` with `rows`.
        pub(crate) fn rows(mut self, class: &str, rows: Vec<WmiRow>) -> Self {
            self.answers.push((class.to_owned(), Ok(rows)));
            self
        }

        /// Answers queries containing `class` with `err`.
        pub(crate) fn error(mut self, class: &str, err: WmiError) -> Self {
            self.answers.push((class.to_owned(), Err(err)));
            self
        }

        /// Queries made so far: `(namespace, query)`, in order.
        pub(crate) fn calls(&self) -> Vec<(String, String)> {
            self.calls
                .lock()
                .map(|calls| calls.clone())
                .unwrap_or_default()
        }
    }

    impl WmiSource for FakeWmi {
        fn query(
            &self,
            namespace: &str,
            query: &str,
            _timeout: Duration,
        ) -> Result<Vec<WmiRow>, WmiError> {
            if let Ok(mut calls) = self.calls.lock() {
                calls.push((namespace.to_owned(), query.to_owned()));
            }
            self.answers
                .iter()
                .find(|(class, _)| query.contains(class.as_str()))
                .map(|(_, answer)| answer.clone())
                .unwrap_or_else(|| Err(WmiError::Failed(format!("fake: no answer for {query}"))))
        }
    }

    /// A row from `(name, value)` pairs.
    pub(crate) fn row<const N: usize>(props: [(&str, WmiValue); N]) -> WmiRow {
        props
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect()
    }

    /// A string value.
    pub(crate) fn s(value: &str) -> WmiValue {
        WmiValue::Text(value.to_owned())
    }
}

#[cfg(test)]
pub(crate) use fake::{row, s};

#[cfg(test)]
mod tests;
