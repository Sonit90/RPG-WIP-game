//! Minimal logging backend for the `log` facade.
//!
//! Design constraints:
//!
//! * No third-party logger. The only dependency is the `log` facade itself.
//! * Output goes to stderr so that stdout stays free for benchmark reports.
//! * The logger is a zero-sized static: installing it performs no allocation.
//!
//! ## Record layout
//!
//! ```text
//! [    1.234] INFO  [main] engine_renderer::device: D3D12 device created
//!  ^ seconds  ^level ^thread ^subsystem              ^message
//! ```
//!
//! The subsystem is the `log` target, which defaults to the module path — so
//! `log::info!` inside `engine_renderer::device` is already attributed correctly without
//! anyone passing a subsystem name by hand. An explicit `target: "Memory"` overrides it.
//!
//! The timestamp is monotonic seconds since logger installation, not wall-clock time: log
//! lines are read against frame numbers and each other, and a wall clock can step
//! backwards mid-run.
//!
//! ## Cost when disabled
//!
//! Two filters, in order:
//!
//! 1. **Compile time.** `engine-app` enables the `log` crate's `release_max_level_info`
//!    feature, so `trace!` and `debug!` calls compile to nothing in release. There is no
//!    branch and no argument evaluation left in the binary.
//! 2. **Run time.** Below that, each macro checks a single relaxed atomic before
//!    evaluating its arguments, so a filtered-out `info!` costs one load and a branch.
//!
//! What is *not* free is a record that passes both filters: formatting allocates. Logging
//! is therefore banned from the per-frame hot path; frame metrics accumulate in plain
//! counters and are logged on an interval.

use std::io::Write;
use std::sync::OnceLock;
use std::time::Instant;

use log::{Level, LevelFilter, Log, Metadata, Record, SetLoggerError};

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
        let _ = write_record(&mut out, elapsed, &thread_name(), record);
    }

    fn flush(&self) {
        let _ = std::io::stderr().flush();
    }
}

/// Identifier for the calling thread.
///
/// Prefers the thread's name, since worker threads are named on creation and a name is
/// far more useful in a log than an opaque id. Falls back to the numeric `ThreadId` for
/// unnamed threads.
fn thread_name() -> String {
    let current = std::thread::current();
    match current.name() {
        Some(name) => name.to_owned(),
        // The Debug form is `ThreadId(3)`; the digits alone are what matter here.
        None => format!("{:?}", current.id())
            .trim_start_matches("ThreadId(")
            .trim_end_matches(')')
            .to_owned(),
    }
}

/// Writes one record in the engine's layout.
///
/// Separated from [`ConsoleLogger::log`] so the layout can be tested against a buffer
/// without installing a process-global logger.
///
/// # Errors
///
/// Returns the writer's error. The console logger ignores it: a failed write to stderr is
/// not actionable and must never abort the engine.
pub fn write_record(
    out: &mut impl Write,
    elapsed_seconds: f64,
    thread: &str,
    record: &Record<'_>,
) -> std::io::Result<()> {
    writeln!(
        out,
        "[{elapsed_seconds:>9.3}] {:<5} [{thread}] {}: {}",
        record.level(),
        record.target(),
        record.args()
    )
}

/// Whether a record at `level` from `target` would be emitted right now.
///
/// Useful before building an expensive diagnostic message, so the work is skipped when
/// the level is filtered out.
#[must_use]
pub fn enabled(level: Level, target: &str) -> bool {
    log::logger().enabled(&Metadata::builder().level(level).target(target).build())
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
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    /// Renders one record to a string using the production layout.
    fn render(level: Level, target: &str, message: &str) -> String {
        let mut buffer = Vec::new();
        // `format_args!` borrows its temporaries, so the record must be built and consumed
        // inside one expression rather than bound to a `let`.
        let result = write_record(
            &mut buffer,
            1.5,
            "main",
            &Record::builder()
                .level(level)
                .target(target)
                .args(format_args!("{message}"))
                .build(),
        );
        result.expect("writing to a Vec cannot fail");
        String::from_utf8(buffer).expect("the layout is ASCII")
    }

    #[test]
    fn a_record_carries_time_level_thread_and_subsystem() {
        let line = render(
            Level::Info,
            "engine_renderer::device",
            "D3D12 device created",
        );
        assert_eq!(
            line,
            "[    1.500] INFO  [main] engine_renderer::device: D3D12 device created\n"
        );
    }

    #[test]
    fn every_level_renders_in_the_same_column_layout() {
        let levels = [
            Level::Trace,
            Level::Debug,
            Level::Info,
            Level::Warn,
            Level::Error,
        ];
        let prefixes: Vec<String> = levels
            .into_iter()
            .map(|level| {
                let line = render(level, "Memory", "x");
                // Everything up to the thread field must line up across levels, or the
                // log is unreadable when scanning a column of records.
                line.split_once("[main]")
                    .expect("the thread field is present")
                    .0
                    .to_owned()
            })
            .collect();
        let width = prefixes[0].len();
        for (level, prefix) in levels.into_iter().zip(&prefixes) {
            assert_eq!(prefix.len(), width, "{level} misaligns the layout");
        }
    }

    #[test]
    fn an_explicit_target_overrides_the_module_path() {
        let line = render(Level::Warn, "Memory", "frame allocator exceeded its budget");
        assert!(
            line.contains("WARN  [main] Memory: frame allocator exceeded its budget"),
            "{line}"
        );
    }

    #[test]
    fn the_thread_field_names_the_current_thread() {
        let name = thread_name();
        assert!(!name.is_empty());

        let named = std::thread::Builder::new()
            .name("worker-0".to_owned())
            .spawn(thread_name)
            .expect("spawning a thread must succeed")
            .join()
            .expect("the thread must not panic");
        assert_eq!(named, "worker-0");
    }

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
