//! Events and progress sent from the engine to the UI and CLI (SPEC-01 §4.5).

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::Serialize;
use specta::Type;

use crate::model::ScanIssue;

/// An event of a scan or backup job.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// A phase began.
    PhaseStarted {
        /// The phase.
        phase: ScanPhase,
    },
    /// A phase ended.
    PhaseFinished {
        /// The phase.
        phase: ScanPhase,
        /// Time the phase took.
        elapsed_ms: u64,
    },
    /// Progress inside a phase; throttled.
    Progress {
        /// The phase.
        phase: ScanPhase,
        /// Units done.
        done: u64,
        /// Units in total, if known.
        total: Option<u64>,
        /// What is being processed, anonymized.
        current: Option<String>,
    },
    /// Findings were added, in a batch.
    FindingsAdded {
        /// Number of new findings.
        count: u32,
    },
    /// A problem or note.
    Issue {
        /// The issue.
        issue: ScanIssue,
    },
    /// Backup progress; throttled.
    BackupProgress {
        /// Bytes written.
        bytes_done: u64,
        /// Bytes to write.
        bytes_total: u64,
        /// Files written.
        files_done: u64,
        /// Files to write.
        files_total: u64,
        /// File being written, anonymized.
        current: Option<String>,
    },
    /// A log line for the UI.
    Log {
        /// Severity.
        level: LogLevel,
        /// Message text.
        message: String,
    },
}

/// Phases of a scan, the rows of SPEC-01 §4.4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ScanPhase {
    /// Known folders, user, drives, launchers.
    Environment,
    /// Rules, games and system collectors.
    Collect,
    /// Unknown folders.
    Heuristics,
    /// Sizes and file counts.
    Measure,
    /// LLM classification.
    Classify,
    /// Merging, scoring, default selection.
    Score,
    /// Building and saving the report.
    Done,
}

/// Severity of an [`Event::Log`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    /// Details for diagnostics.
    Debug,
    /// Normal operation.
    Info,
    /// Something unexpected, the job continues.
    Warn,
    /// A step failed.
    Error,
}

/// Receiver side is the UI bridge or the CLI.
pub type EventSink = tokio::sync::mpsc::UnboundedSender<Event>;

/// Minimum time between two throttled events with the same key (10 per second).
const INTERVAL: Duration = Duration::from_millis(100);

/// Throttling key: the kind of a frequent event and its phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Progress(ScanPhase),
    Backup,
}

fn key(event: &Event) -> Option<Key> {
    match event {
        Event::Progress { phase, .. } => Some(Key::Progress(*phase)),
        Event::BackupProgress { .. } => Some(Key::Backup),
        _ => None,
    }
}

#[derive(Debug, Default)]
struct Slot {
    last_sent: Option<Instant>,
    pending: Option<Event>,
}

/// [`EventSink`] wrapper for sources of frequent progress (SPEC-01 §4.5).
///
/// `Progress` and `BackupProgress` go out at most every 100 ms per phase;
/// in between the newest one is kept and sent by [`flush`](Self::flush),
/// on drop, or before any other event. Can be shared between threads.
#[derive(Debug)]
pub struct ThrottledSink {
    sink: EventSink,
    slots: Mutex<BTreeMap<Key, Slot>>,
}

impl ThrottledSink {
    /// Wraps a sink.
    pub fn new(sink: EventSink) -> Self {
        Self {
            sink,
            slots: Mutex::new(BTreeMap::new()),
        }
    }

    /// Sends an event, throttling frequent progress.
    pub fn send(&self, event: Event) {
        self.send_at(event, Instant::now());
    }

    /// Sends the progress events held back by throttling.
    pub fn flush(&self) {
        let mut slots = self.lock();
        self.flush_pending(&mut slots, Instant::now());
    }

    fn send_at(&self, event: Event, now: Instant) {
        let mut slots = self.lock();
        match key(&event) {
            Some(key) => {
                let slot = slots.entry(key).or_default();
                let due = slot
                    .last_sent
                    .is_none_or(|last| now.saturating_duration_since(last) >= INTERVAL);
                if due {
                    // A newer event replaces the held one.
                    slot.pending = None;
                    slot.last_sent = Some(now);
                    self.deliver(event);
                } else {
                    slot.pending = Some(event);
                }
            }
            None => {
                self.flush_pending(&mut slots, now);
                self.deliver(event);
            }
        }
    }

    fn flush_pending(&self, slots: &mut BTreeMap<Key, Slot>, now: Instant) {
        for slot in slots.values_mut() {
            if let Some(event) = slot.pending.take() {
                slot.last_sent = Some(now);
                self.deliver(event);
            }
        }
    }

    fn deliver(&self, event: Event) {
        // A closed channel means nobody listens any more; that is not an error.
        let _ = self.sink.send(event);
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<Key, Slot>> {
        // The map stays consistent even if a sender panicked while holding the lock.
        self.slots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Drop for ThrottledSink {
    fn drop(&mut self) {
        self.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

    fn progress(phase: ScanPhase, done: u64) -> Event {
        Event::Progress {
            phase,
            done,
            total: Some(1000),
            current: None,
        }
    }

    fn drain(rx: &mut UnboundedReceiver<Event>) -> Vec<Event> {
        let mut out = Vec::new();
        while let Ok(event) = rx.try_recv() {
            out.push(event);
        }
        out
    }

    /// T-01-02: 1000 events within 100 ms → at most 2 delivered + the last one.
    #[test]
    fn throttles_1000_events_in_100_ms() {
        let (tx, mut rx) = unbounded_channel();
        let sink = ThrottledSink::new(tx);
        let start = Instant::now();
        for i in 0..1000u64 {
            sink.send_at(
                progress(ScanPhase::Measure, i),
                start + Duration::from_micros(i * 100),
            );
        }
        let during = drain(&mut rx);
        assert_eq!(during, [progress(ScanPhase::Measure, 0)]);
        drop(sink);
        assert_eq!(drain(&mut rx), [progress(ScanPhase::Measure, 999)]);
    }

    #[test]
    fn throttles_in_real_time() {
        let (tx, mut rx) = unbounded_channel();
        let sink = ThrottledSink::new(tx);
        for i in 0..1000u64 {
            sink.send(progress(ScanPhase::Collect, i));
        }
        sink.flush();
        let events = drain(&mut rx);
        assert!(events.len() <= 3, "{} events", events.len());
        assert_eq!(events.last(), Some(&progress(ScanPhase::Collect, 999)));
    }

    #[test]
    fn sends_again_after_the_interval() {
        let (tx, mut rx) = unbounded_channel();
        let sink = ThrottledSink::new(tx);
        let t0 = Instant::now();
        sink.send_at(progress(ScanPhase::Measure, 1), t0);
        sink.send_at(
            progress(ScanPhase::Measure, 2),
            t0 + Duration::from_millis(50),
        );
        sink.send_at(
            progress(ScanPhase::Measure, 3),
            t0 + Duration::from_millis(100),
        );
        // 2 was superseded by 3.
        assert_eq!(
            drain(&mut rx),
            [
                progress(ScanPhase::Measure, 1),
                progress(ScanPhase::Measure, 3)
            ]
        );
        sink.flush();
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn phases_and_backup_are_throttled_separately() {
        let (tx, mut rx) = unbounded_channel();
        let sink = ThrottledSink::new(tx);
        let t0 = Instant::now();
        let backup = |bytes_done| Event::BackupProgress {
            bytes_done,
            bytes_total: 10,
            files_done: 0,
            files_total: 1,
            current: None,
        };
        sink.send_at(progress(ScanPhase::Collect, 1), t0);
        sink.send_at(progress(ScanPhase::Measure, 1), t0);
        sink.send_at(backup(1), t0);
        sink.send_at(backup(2), t0);
        assert_eq!(
            drain(&mut rx),
            [
                progress(ScanPhase::Collect, 1),
                progress(ScanPhase::Measure, 1),
                backup(1)
            ]
        );
        sink.flush();
        assert_eq!(drain(&mut rx), [backup(2)]);
    }

    #[test]
    fn other_events_flush_pending_progress_first() {
        let (tx, mut rx) = unbounded_channel();
        let sink = ThrottledSink::new(tx);
        let t0 = Instant::now();
        let finished = Event::PhaseFinished {
            phase: ScanPhase::Measure,
            elapsed_ms: 5,
        };
        sink.send_at(progress(ScanPhase::Measure, 1), t0);
        sink.send_at(progress(ScanPhase::Measure, 2), t0);
        sink.send_at(finished.clone(), t0);
        sink.send_at(Event::FindingsAdded { count: 3 }, t0);
        assert_eq!(
            drain(&mut rx),
            [
                progress(ScanPhase::Measure, 1),
                progress(ScanPhase::Measure, 2),
                finished,
                Event::FindingsAdded { count: 3 }
            ]
        );
    }

    #[test]
    fn closed_channel_is_ignored() {
        let (tx, rx) = unbounded_channel();
        drop(rx);
        let sink = ThrottledSink::new(tx);
        sink.send(progress(ScanPhase::Score, 1));
        sink.send(progress(ScanPhase::Score, 2));
        sink.send(Event::FindingsAdded { count: 1 });
    }

    #[test]
    fn json_shape() {
        let json = serde_json::to_value(progress(ScanPhase::Measure, 5)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "type": "progress", "phase": "measure", "done": 5, "total": 1000, "current": null })
        );
        let log = Event::Log {
            level: LogLevel::Warn,
            message: "x".to_owned(),
        };
        assert_eq!(
            serde_json::to_value(log).unwrap(),
            serde_json::json!({ "type": "log", "level": "warn", "message": "x" })
        );
    }
}
