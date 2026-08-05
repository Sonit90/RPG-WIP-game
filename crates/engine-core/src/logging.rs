//! Minimal logging backend for the `log` facade.
//!
//! Design constraints:
//!
//! * No third-party logger. The only dependency is the `log` facade itself.
//! * Output goes to stderr so that stdout stays free for benchmark reports.
//! * The logger is a zero-sized static: installing it performs no allocation.
//!
//! The log macros themselves format into a temporary buffer, so logging *does*
//! allocate. Logging is therefore forbidden in the per-frame hot path; frame
//! metrics are accumulated in plain counters and only logged on an interval.

use std::io::Write;
use std::sync::OnceLock;
use std::time::Instant;

use log::{LevelFilter, Log, Metadata, Record, SetLoggerError};

/// Environment variable used to override the log level at startup.
pub const LOG_LEVEL_ENV: &str = "MYGAME_LOG";

/// Process start time, used to render a monotonic timestamp per record.
static START: OnceLock<Instant> = OnceLock::new();

static LOGGER: ConsoleLogger = ConsoleLogger;

/// Stderr logger with a `[seconds] LEVEL target: message` layout.
struct ConsoleLogger;

impl Log for ConsoleLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let elapsed = START.get_or_init(Instant::now).elapsed().as_secs_f64();
        // Locking stderr keeps a single record on one line under concurrency.
        let mut out = std::io::stderr().lock();
        // A failed write to stderr is not actionable and must never abort the engine.
        let _ = writeln!(
            out,
            "[{elapsed:>9.3}] {:<5} {}: {}",
            record.level(),
            record.target(),
            record.args()
        );
    }

    fn flush(&self) {
        let _ = std::io::stderr().flush();
    }
}

/// Parses a log level name, case-insensitively.
///
/// Accepts `off`, `error`, `warn`, `info`, `debug`, `trace`.
#[must_use]
pub fn parse_level(value: &str) -> Option<LevelFilter> {
    match value.trim().to_ascii_lowercase().as_str() {
        "off" => Some(LevelFilter::Off),
        "error" => Some(LevelFilter::Error),
        "warn" | "warning" => Some(LevelFilter::Warn),
        "info" => Some(LevelFilter::Info),
        "debug" => Some(LevelFilter::Debug),
        "trace" => Some(LevelFilter::Trace),
        _ => None,
    }
}

/// Resolves the startup level from the environment, falling back to `default`.
///
/// An unparsable value is ignored in favour of `default`; startup must not fail
/// because of a typo in an environment variable.
#[must_use]
pub fn level_from_env(default: LevelFilter) -> LevelFilter {
    std::env::var(LOG_LEVEL_ENV)
        .ok()
        .and_then(|value| parse_level(&value))
        .unwrap_or(default)
}

/// Installs the console logger with an explicit level filter.
///
/// # Errors
///
/// Returns [`SetLoggerError`] if a logger has already been installed in this process.
pub fn init_with(filter: LevelFilter) -> Result<(), SetLoggerError> {
    START.get_or_init(Instant::now);
    log::set_logger(&LOGGER)?;
    log::set_max_level(filter);
    Ok(())
}

/// Installs the console logger, taking the level from [`LOG_LEVEL_ENV`].
///
/// Defaults to [`LevelFilter::Info`] in release builds and [`LevelFilter::Debug`] otherwise.
///
/// # Errors
///
/// Returns [`SetLoggerError`] if a logger has already been installed in this process.
pub fn init() -> Result<(), SetLoggerError> {
    let default = if cfg!(debug_assertions) {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    };
    init_with(level_from_env(default))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_levels_case_insensitively() {
        assert_eq!(parse_level("info"), Some(LevelFilter::Info));
        assert_eq!(parse_level("INFO"), Some(LevelFilter::Info));
        assert_eq!(parse_level("  Trace "), Some(LevelFilter::Trace));
        assert_eq!(parse_level("warning"), Some(LevelFilter::Warn));
        assert_eq!(parse_level("off"), Some(LevelFilter::Off));
    }

    #[test]
    fn rejects_unknown_levels() {
        assert_eq!(parse_level("verbose"), None);
        assert_eq!(parse_level(""), None);
    }

    #[test]
    fn falls_back_to_default_when_env_is_unset_or_invalid() {
        // SAFETY: the process-wide environment is mutated here. Tests in this module
        // are the only readers of this variable and Rust runs them in one process,
        // so the value is restored before the function returns.
        unsafe {
            std::env::remove_var(LOG_LEVEL_ENV);
        }
        assert_eq!(level_from_env(LevelFilter::Warn), LevelFilter::Warn);

        // SAFETY: see above.
        unsafe {
            std::env::set_var(LOG_LEVEL_ENV, "not-a-level");
        }
        assert_eq!(level_from_env(LevelFilter::Warn), LevelFilter::Warn);

        // SAFETY: see above.
        unsafe {
            std::env::set_var(LOG_LEVEL_ENV, "trace");
        }
        assert_eq!(level_from_env(LevelFilter::Warn), LevelFilter::Trace);

        // SAFETY: see above.
        unsafe {
            std::env::remove_var(LOG_LEVEL_ENV);
        }
    }
}
