//! The last errors, kept in memory for the dashboard.
//!
//! A tracing layer copies every ERROR-level event into a ring buffer, and the realm bridge
//! adds its failed passes (which log at WARN) by hand. Nothing here is persisted: it answers
//! "what went wrong in the last few minutes" without opening the logs.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::{Mutex, OnceLock};

use serde::Serialize;
use time::OffsetDateTime;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

/// Entries kept.
pub const CAPACITY: usize = 50;

/// One error line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ErrorEntry {
    /// Unix seconds of the latest occurrence.
    pub at: i64,
    /// Where it came from: a tracing target, or `bridge`.
    pub source: String,
    pub message: String,
    /// Consecutive repeats folded into this entry.
    pub count: u32,
}

/// A bounded, newest-last list of errors.
#[derive(Debug, Default)]
pub struct ErrorLog {
    entries: Mutex<VecDeque<ErrorEntry>>,
}

impl ErrorLog {
    /// Records an error. A repeat of the newest entry is folded into it rather than evicting
    /// older, different errors.
    pub fn push(&self, source: &str, message: &str) {
        let at = OffsetDateTime::now_utc().unix_timestamp();
        let Ok(mut entries) = self.entries.lock() else {
            return;
        };
        if let Some(last) = entries.back_mut() {
            if last.source == source && last.message == message {
                last.count += 1;
                last.at = at;
                return;
            }
        }
        if entries.len() >= CAPACITY {
            entries.pop_front();
        }
        entries.push_back(ErrorEntry {
            at,
            source: source.to_owned(),
            message: message.to_owned(),
            count: 1,
        });
    }

    /// The entries, newest first.
    pub fn recent(&self) -> Vec<ErrorEntry> {
        self.entries
            .lock()
            .map(|entries| entries.iter().rev().cloned().collect())
            .unwrap_or_default()
    }
}

/// The process-wide log. A global because the tracing layer is built before the app state.
pub fn error_log() -> &'static ErrorLog {
    static LOG: OnceLock<ErrorLog> = OnceLock::new();
    LOG.get_or_init(ErrorLog::default)
}

/// Copies ERROR events into [`error_log`].
pub struct ErrorLogLayer;

#[derive(Default)]
struct MessageVisitor {
    message: String,
    fields: String,
}

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.message, "{value:?}");
        } else {
            let _ = write!(self.fields, " {}={value:?}", field.name());
        }
    }
}

impl<S: Subscriber> Layer<S> for ErrorLogLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if *event.metadata().level() != Level::ERROR {
            return;
        }
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        let message = format!("{}{}", visitor.message, visitor.fields);
        error_log().push(event.metadata().target(), message.trim());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::prelude::*;

    #[test]
    fn the_buffer_keeps_the_last_fifty_newest_first() {
        let log = ErrorLog::default();
        for i in 0..(CAPACITY + 10) {
            log.push("t", &format!("error {i}"));
        }
        let recent = log.recent();
        assert_eq!(recent.len(), CAPACITY);
        assert_eq!(recent[0].message, format!("error {}", CAPACITY + 9));
        assert_eq!(recent[CAPACITY - 1].message, "error 10");
    }

    #[test]
    fn a_repeat_is_folded_into_the_newest_entry() {
        let log = ErrorLog::default();
        log.push("bridge", "down");
        log.push("bridge", "down");
        log.push("bridge", "other");
        log.push("bridge", "down");
        let recent = log.recent();
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[2].count, 2);
    }

    #[test]
    fn the_layer_captures_errors_and_ignores_the_rest() {
        let subscriber = tracing_subscriber::registry().with(ErrorLogLayer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!("a layer-test warning");
            tracing::error!(code = 7, "a layer-test failure");
        });
        let recent = error_log().recent();
        assert!(recent
            .iter()
            .any(|e| e.message.contains("a layer-test failure") && e.message.contains("code=7")));
        assert!(!recent
            .iter()
            .any(|e| e.message.contains("a layer-test warning")));
    }
}
