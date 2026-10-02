//! Logging: a daily rotated file and `Event::Log` for warnings (SPEC-01 §4.8.3).
//!
//! Every line is anonymized by the writer: known folder paths become tokens,
//! the user and machine names become `<redacted>`, so logs can be attached to
//! bug reports.

mod redact;

use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;

use tracing::field::{Field, Visit};
use tracing::{Level, Subscriber};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{Builder, Rotation};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::{EnvFilter, Registry};

use crate::env::Environment;
use crate::events::{Event, EventSink, LogLevel};
use redact::{LineRedactor, RedactingMakeWriter};

/// Number of daily log files kept.
const MAX_LOG_FILES: usize = 7;

/// Keeps the background log writer alive; dropping it flushes the file.
#[must_use = "logs are lost when the guard is dropped"]
#[derive(Debug)]
pub struct LogGuard {
    _worker: WorkerGuard,
}

/// Logging could not be set up.
#[derive(Debug, thiserror::Error)]
pub enum LogError {
    /// The log folder or file cannot be created.
    #[error("log file: {0}")]
    Io(#[from] std::io::Error),
    /// `SK_LOG` is not a valid filter.
    #[error("invalid SK_LOG filter: {0}")]
    Filter(String),
    /// A global subscriber is already installed.
    #[error("logging is already initialized")]
    AlreadyInitialized,
}

/// Installs the global subscriber: `<logs_dir>/savekeeper.YYYY-MM-DD.log`
/// (7 files kept) and `Event::Log` for warnings and errors in `events`.
/// The level comes from `SK_LOG` (`info` by default).
pub fn init(
    logs_dir: &Path,
    env: &Environment,
    events: Option<EventSink>,
) -> Result<LogGuard, LogError> {
    let filter = std::env::var("SK_LOG").unwrap_or_else(|_| "info".to_owned());
    let (subscriber, guard) = build(logs_dir, env, events, &filter)?;
    tracing::subscriber::set_global_default(subscriber)
        .map_err(|_| LogError::AlreadyInitialized)?;
    Ok(guard)
}

/// The subscriber without installing it (tests use it with `with_default`).
fn build(
    logs_dir: &Path,
    env: &Environment,
    events: Option<EventSink>,
    filter: &str,
) -> Result<(impl Subscriber + Send + Sync, LogGuard), LogError> {
    let filter = EnvFilter::try_new(filter).map_err(|e| LogError::Filter(e.to_string()))?;
    std::fs::create_dir_all(logs_dir)?;
    let appender = Builder::new()
        .rotation(Rotation::DAILY)
        .filename_prefix("savekeeper")
        .filename_suffix("log")
        .max_log_files(MAX_LOG_FILES)
        .build(logs_dir)
        .map_err(std::io::Error::other)?;
    let (writer, worker) = tracing_appender::non_blocking(appender);
    let redactor = Arc::new(LineRedactor::new(env));

    let file = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(RedactingMakeWriter {
            inner: writer,
            redactor: Arc::clone(&redactor),
        });
    let events = events.map(|sink| EventLayer { sink, redactor });
    let subscriber = Registry::default().with(filter).with(file).with(events);
    Ok((subscriber, LogGuard { _worker: worker }))
}

/// Sends warnings and errors as `Event::Log`.
struct EventLayer {
    sink: EventSink,
    redactor: Arc<LineRedactor>,
}

impl<S: Subscriber> Layer<S> for EventLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let level = match *event.metadata().level() {
            Level::ERROR => LogLevel::Error,
            Level::WARN => LogLevel::Warn,
            _ => return,
        };
        let mut message = MessageVisitor::default();
        event.record(&mut message);
        let message = self.redactor.apply(&message.finish());
        // Nobody listening is not an error.
        let _ = self.sink.send(Event::Log { level, message });
    }
}

/// `message` first, then the other fields as `name=value`.
#[derive(Default)]
struct MessageVisitor {
    text: String,
    fields: String,
}

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.text, "{value:?}");
        } else {
            let _ = write!(self.fields, " {}={value:?}", field.name());
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.text.push_str(value);
        } else {
            let _ = write!(self.fields, " {}={value}", field.name());
        }
    }
}

impl MessageVisitor {
    fn finish(mut self) -> String {
        self.text.push_str(&self.fields);
        self.text
    }
}

#[cfg(test)]
mod tests;
