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
//!
//! ## Cost when enabled
//!
//! The write, not the formatting, is what costs: stderr is unbuffered, so every record is
//! its own `WriteFile`, and against a Windows console that call is blocking
//! inter-process communication. A burst of twenty records on the main thread is twenty
//! blocking calls between two frames, which is a visible hitch.
//!
//! So the console write does not happen on the calling thread. A record is rendered into
//! a pooled buffer and handed to a writer thread over a bounded queue; the writer takes
//! everything queued at once and emits it as a single write. A full queue drops the
//! record and counts it rather than block — a log line must never stall a frame — and the
//! count is reported on the next record so the gap is visible. Call [`drain`] before a
//! process exit or a panic, or the queue dies with the process.

use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use log::{Level, LevelFilter, Log, Metadata, Record, SetLoggerError};

/// Environment variable used to override the log level at startup.
pub const LOG_LEVEL_ENV: &str = "MYGAME_LOG";

/// Process start time, used to render a monotonic timestamp per record.
static START: OnceLock<Instant> = OnceLock::new();

static LOGGER: ConsoleLogger = ConsoleLogger;

/// Depth of the queue between the logging threads and the writer.
///
/// Deep enough to swallow a startup burst or a whole periodic report at once; shallow
/// enough that a wedged console cannot grow it without bound.
const QUEUE_DEPTH: usize = 4096;

/// How many record buffers are kept for reuse.
///
/// The writer hands every buffer back as soon as it has been copied into the batch, so a
/// steady stream of records reuses a handful and allocates nothing.
const POOL_DEPTH: usize = 64;

/// How long [`drain`] waits for the writer thread to acknowledge.
///
/// A shutdown must not hang on a console that has stopped accepting writes, and the
/// records lost by giving up are the ones nobody could have read anyway.
const DRAIN_TIMEOUT: Duration = Duration::from_millis(500);

/// Work item for the writer thread.
enum Message {
    /// One rendered record, newline included.
    Record(Vec<u8>),
    /// Write everything queued, flush stderr, then acknowledge on the channel.
    Flush(SyncSender<()>),
}

/// Handle to the writer thread, published once the thread is running.
struct Sink {
    /// Queue of pending work.
    records: SyncSender<Message>,
    /// Records dropped because the queue was full, reported on the next record.
    dropped: AtomicU64,
    /// Buffers handed back by the writer, so a steady stream of records allocates
    /// nothing after the first few.
    pool: Mutex<Vec<Vec<u8>>>,
}

impl Sink {
    /// Takes a buffer from the pool, or allocates one if the pool is empty.
    fn take_buffer(&self) -> Vec<u8> {
        self.pool
            .lock()
            .ok()
            .and_then(|mut pool| pool.pop())
            .unwrap_or_default()
    }

    /// Returns a buffer to the pool, dropping it if the pool is already full.
    fn recycle(&self, mut buffer: Vec<u8>) {
        buffer.clear();
        if let Ok(mut pool) = self.pool.lock() {
            if pool.len() < POOL_DEPTH {
                pool.push(buffer);
            }
        }
    }
}

/// The writer thread's queue, or `None` while records must be written inline.
static SINK: OnceLock<Sink> = OnceLock::new();

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
        let Some(sink) = SINK.get() else {
            // No writer thread: either the logger was installed without one, or spawning
            // it failed. Writing inline is slow but never loses a record.
            // Locking stderr keeps a single record on one line under concurrency.
            let mut out = std::io::stderr().lock();
            // A failed write to stderr is not actionable and must never abort the engine.
            let _ = write_record(&mut out, elapsed, cached_thread_name(), record);
            return;
        };
        let mut buffer = sink.take_buffer();
        // Silent loss would be worse than the loss itself: whoever reads the log needs to
        // know that a gap is there.
        let dropped = sink.dropped.swap(0, Ordering::Relaxed);
        if dropped > 0 {
            let _ = writeln!(
                buffer,
                "[{elapsed:>9.3}] WARN  [log] Logging: {dropped} record(s) dropped, the log queue was full"
            );
        }
        // Writing to a `Vec` cannot fail; the result is ignored for the same reason the
        // inline path ignores its own.
        let _ = write_record(&mut buffer, elapsed, cached_thread_name(), record);
        if let Err(error) = sink.records.try_send(Message::Record(buffer)) {
            // Dropping beats blocking. A full queue means the console is behind, and no
            // log line is worth stalling the frame that produced it.
            sink.dropped.fetch_add(dropped + 1, Ordering::Relaxed);
            if let std::sync::mpsc::TrySendError::Full(Message::Record(buffer)) = error {
                sink.recycle(buffer);
            }
        }
    }

    fn flush(&self) {
        drain();
    }
}

/// Writes everything queued and waits for it to reach stderr.
///
/// Call before a process exit or a panic: records sit in the queue until the writer
/// thread gets to them, and a dying process takes them with it. Gives up after
/// [`DRAIN_TIMEOUT`] rather than hang on a console that is not draining.
pub fn drain() {
    let Some(sink) = SINK.get() else {
        let _ = std::io::stderr().flush();
        return;
    };
    let (acknowledge, drained) = sync_channel(1);
    // A blocking send is correct here: draining is never on the frame path, and the
    // caller is asking to wait.
    if sink.records.send(Message::Flush(acknowledge)).is_err() {
        return;
    }
    let _ = drained.recv_timeout(DRAIN_TIMEOUT);
}

/// Starts the writer thread and publishes its queue.
///
/// The console write is the expensive part of logging, not the formatting: on Windows a
/// write to a console host is blocking inter-process communication, and a burst of
/// records is a burst of them on whichever thread logged. Moving the writes here costs
/// the caller one formatted line and a channel send, and lets a burst be coalesced into
/// a single write.
fn start_writer() {
    if SINK.get().is_some() {
        return;
    }
    let (records, queue) = sync_channel(QUEUE_DEPTH);
    if std::thread::Builder::new()
        .name("log".to_owned())
        .spawn(move || writer_loop(&queue))
        .is_err()
    {
        // Leaving `SINK` unset is the fallback: every record is then written inline.
        return;
    }
    let _ = SINK.set(Sink {
        records,
        dropped: AtomicU64::new(0),
        pool: Mutex::new(Vec::new()),
    });
}

/// Drains the queue onto stderr until every sender is gone.
fn writer_loop(queue: &Receiver<Message>) {
    let mut writer = Writer::default();
    while writer.pump(queue, &mut std::io::stderr()) {}
}

/// The writer thread's scratch space, kept across batches so it stops allocating.
#[derive(Default)]
struct Writer {
    /// The current batch, ready to go out as one write.
    batch: Vec<u8>,
    /// Record buffers already copied into `batch`, waiting to go back to the pool.
    spent: Vec<Vec<u8>>,
    /// Who is waiting for this batch to reach the console.
    acknowledgements: Vec<SyncSender<()>>,
}

impl Writer {
    /// Takes everything already queued and emits it as a single write.
    ///
    /// One write per batch rather than one per record is the whole point: the periodic
    /// report's twenty-odd lines cost one syscall instead of twenty.
    ///
    /// Returns `false` once every sender is gone and there is nothing left to write.
    fn pump(&mut self, queue: &Receiver<Message>, out: &mut impl Write) -> bool {
        let Ok(first) = queue.recv() else {
            return false;
        };
        self.batch.clear();
        let mut message = Some(first);
        while let Some(next) = message {
            match next {
                Message::Record(record) => {
                    self.batch.extend_from_slice(&record);
                    self.spent.push(record);
                }
                Message::Flush(acknowledgement) => self.acknowledgements.push(acknowledgement),
            }
            // An empty queue and a disconnected one both end the batch.
            message = queue.try_recv().ok();
        }
        // A failed write to stderr is not actionable and must never abort the engine.
        let _ = out.write_all(&self.batch);
        if !self.acknowledgements.is_empty() {
            let _ = out.flush();
        }
        match SINK.get() {
            Some(sink) => {
                for buffer in self.spent.drain(..) {
                    sink.recycle(buffer);
                }
            }
            None => self.spent.clear(),
        }
        // After the write, so a caller waiting on the drain knows the bytes are out.
        for acknowledgement in self.acknowledgements.drain(..) {
            let _ = acknowledgement.send(());
        }
        true
    }
}

/// This thread's identifier, computed once per thread.
///
/// A thread's name never changes and every record needs it, so computing one per record
/// is an allocation per record for a string that is always the same. The name is leaked
/// instead, which bounds the waste by the number of threads the process ever creates —
/// a handful — and lets every record borrow it for free.
fn cached_thread_name() -> &'static str {
    thread_local! {
        static NAME: &'static str = Box::leak(thread_name().into_boxed_str());
    }
    // `try_with` fails only while thread-local storage is being torn down. A record
    // logged from a destructor is still worth printing, just without a name.
    NAME.try_with(|name| *name).unwrap_or("?")
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
    start_writer();
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
    fn a_burst_of_records_reaches_the_writer_in_order_as_one_write() {
        let (sender, queue) = sync_channel(32);
        for index in 0..8 {
            let mut record = Vec::new();
            writeln!(record, "record {index}").expect("writing to a Vec cannot fail");
            sender
                .send(Message::Record(record))
                .expect("the queue has room");
        }
        let (acknowledge, drained) = sync_channel(1);
        sender
            .send(Message::Flush(acknowledge))
            .expect("the queue has room");

        // One pass: everything queued goes out together, which is what makes the report
        // one syscall instead of twenty.
        let mut out = Vec::new();
        assert!(Writer::default().pump(&queue, &mut out));
        drained
            .recv_timeout(DRAIN_TIMEOUT)
            .expect("the flush is acknowledged once the batch is written");

        let expected: String = (0..8).map(|index| format!("record {index}\n")).collect();
        assert_eq!(
            String::from_utf8(out).expect("the records are ASCII"),
            expected
        );
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
