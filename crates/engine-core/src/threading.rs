//! Worker threads: named, cooperatively stopped, joined on drop.
//!
//! This is the foundation a job system is built on, not the job system itself. It answers
//! three questions and nothing else: how a thread is started, how it is asked to stop, and
//! who waits for it.
//!
//! ## Cooperative shutdown
//!
//! There is no way to kill a thread safely, so shutdown is a flag the worker checks. A
//! worker that blocks must block on something that can be woken — a condition variable
//! signalled by the same code that sets the flag — or it will not notice until it wakes on
//! its own.
//!
//! ## Joined on drop
//!
//! [`Worker::drop`] requests shutdown and joins. A detached engine thread outliving the
//! object that owns its data is a use-after-free waiting for a slow machine, and the point
//! of the abstraction is that forgetting to join is not possible.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

use crate::error::EngineError;

/// A flag a worker polls to learn it should stop.
///
/// Cloned into the worker; setting it from the owning side is what ends the loop.
#[derive(Debug, Clone, Default)]
pub struct ShutdownSignal {
    flag: Arc<AtomicBool>,
}

impl ShutdownSignal {
    /// Creates an unset signal.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Asks the worker to stop.
    ///
    /// `Release` pairs with the worker's `Acquire` load, so everything written before this
    /// call is visible to the worker once it observes the flag.
    pub fn set(&self) {
        self.flag.store(true, Ordering::Release);
    }

    /// Whether shutdown has been requested.
    #[must_use]
    pub fn is_set(&self) -> bool {
        self.flag.load(Ordering::Acquire)
    }
}

/// A named OS thread with cooperative shutdown.
///
/// The closure receives the [`ShutdownSignal`] and is expected to return when it is set.
#[derive(Debug)]
pub struct Worker {
    name: String,
    shutdown: ShutdownSignal,
    /// `None` only after [`Worker::join`] has taken the handle.
    handle: Option<JoinHandle<()>>,
}

impl Worker {
    /// Spawns a worker running `body`.
    ///
    /// The name is given to the OS where supported, so it shows up in a debugger and in
    /// this engine's log lines — an unnamed thread in a crash dump is one more thing to
    /// work out under pressure.
    ///
    /// # Errors
    ///
    /// Returns an [`EngineError`] in the `Platform` category if the OS refuses to create
    /// the thread.
    pub fn spawn<F>(name: impl Into<String>, body: F) -> Result<Self, EngineError>
    where
        F: FnOnce(ShutdownSignal) + Send + 'static,
    {
        let name = name.into();
        let shutdown = ShutdownSignal::new();
        let worker_signal = shutdown.clone();

        let handle = thread::Builder::new()
            .name(name.clone())
            .spawn(move || body(worker_signal))
            .map_err(|error| {
                EngineError::platform(format!("spawn worker thread {name:?}")).with_source(error)
            })?;

        log::debug!(target: "Threading", "worker {name:?} started");

        Ok(Self {
            name,
            shutdown,
            handle: Some(handle),
        })
    }

    /// The worker's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// A clone of the shutdown signal, for waking a blocked worker.
    #[must_use]
    pub fn shutdown_signal(&self) -> ShutdownSignal {
        self.shutdown.clone()
    }

    /// Asks the worker to stop without waiting for it.
    ///
    /// A worker parked on a condition variable also needs to be notified; this only sets
    /// the flag.
    pub fn request_shutdown(&self) {
        self.shutdown.set();
    }

    /// Whether the worker's closure has returned.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.handle
            .as_ref()
            .is_none_or(std::thread::JoinHandle::is_finished)
    }

    /// Requests shutdown and waits for the worker to finish.
    ///
    /// # Errors
    ///
    /// Returns an [`EngineError`] in the `InvalidState` category if the worker panicked.
    /// The panic is not resumed here: one worker dying should not take the engine's
    /// shutdown path with it, and the caller decides what a dead worker means.
    pub fn join(mut self) -> Result<(), EngineError> {
        self.shutdown.set();
        self.join_handle()
    }

    /// Joins the handle if it is still present. Shared by [`Worker::join`] and `Drop`.
    fn join_handle(&mut self) -> Result<(), EngineError> {
        let Some(handle) = self.handle.take() else {
            return Ok(());
        };
        match handle.join() {
            Ok(()) => {
                log::debug!(target: "Threading", "worker {:?} stopped", self.name);
                Ok(())
            }
            Err(_) => Err(EngineError::invalid_state(format!(
                "worker thread {:?} panicked",
                self.name
            ))),
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.shutdown.set();
        if let Err(error) = self.join_handle() {
            // Dropping is not a place to fail from, but a panicked worker must not vanish
            // silently.
            log::error!(target: "Threading", "{}", error.chain());
        }
    }
}

/// How many worker threads to create by default.
///
/// One less than the number of logical cores, so the main thread — which drives the frame
/// loop and submits GPU work — keeps a core to itself. At least one worker either way, so
/// code paths that assume a worker exists stay exercised on small machines.
#[must_use]
pub fn recommended_worker_count() -> usize {
    thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(2)
        .saturating_sub(1)
        .max(1)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;
    use std::time::{Duration, Instant};

    /// Spins until `condition` holds or the timeout expires.
    ///
    /// Returns whether it held. Used instead of a fixed sleep so the tests are neither
    /// flaky on a loaded machine nor slow on an idle one.
    fn wait_for(timeout: Duration, condition: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if condition() {
                return true;
            }
            thread::yield_now();
        }
        condition()
    }

    #[test]
    fn a_worker_runs_its_body_and_knows_its_name() {
        let observed = Arc::new(std::sync::Mutex::new(String::new()));
        let sink = Arc::clone(&observed);

        let worker = Worker::spawn("test-worker", move |_shutdown| {
            let name = thread::current().name().unwrap_or_default().to_owned();
            *sink.lock().expect("the mutex is not poisoned") = name;
        })
        .expect("spawning must succeed");

        assert_eq!(worker.name(), "test-worker");
        worker.join().expect("the worker must not panic");
        assert_eq!(
            *observed.lock().expect("the mutex is not poisoned"),
            "test-worker",
            "the OS thread carries the name, so a debugger and the log agree"
        );
    }

    #[test]
    fn a_worker_loop_stops_when_shutdown_is_requested() {
        let iterations = Arc::new(AtomicU32::new(0));
        let counter = Arc::clone(&iterations);

        let worker = Worker::spawn("looping-worker", move |shutdown| {
            while !shutdown.is_set() {
                counter.fetch_add(1, Ordering::Relaxed);
                thread::yield_now();
            }
        })
        .expect("spawning must succeed");

        assert!(
            wait_for(Duration::from_secs(5), || iterations
                .load(Ordering::Relaxed)
                > 0),
            "the worker must actually run"
        );

        worker.join().expect("the worker must not panic");
    }

    #[test]
    fn dropping_a_worker_stops_and_joins_it() {
        let finished = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&finished);

        {
            let _worker = Worker::spawn("dropped-worker", move |shutdown| {
                while !shutdown.is_set() {
                    thread::yield_now();
                }
                flag.store(true, Ordering::Release);
            })
            .expect("spawning must succeed");
        }

        // Drop has returned, so the worker has already finished — no waiting needed. That
        // is the property under test: a detached engine thread cannot outlive its owner.
        assert!(
            finished.load(Ordering::Acquire),
            "drop must request shutdown and join"
        );
    }

    #[test]
    fn a_panicking_worker_is_reported_rather_than_resumed() {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let worker = Worker::spawn("panicking-worker", |_shutdown| {
            panic!("deliberate test panic");
        })
        .expect("spawning must succeed");

        let result = worker.join();
        std::panic::set_hook(previous);

        let error = result.expect_err("a panicked worker must be reported");
        assert_eq!(
            error.category(),
            crate::error::ErrorCategory::InvalidState,
            "{}",
            error.chain()
        );
        assert!(error.chain().contains("panicking-worker"));
    }

    #[test]
    fn a_finished_worker_reports_itself_as_finished() {
        let worker = Worker::spawn("brief-worker", |_shutdown| {}).expect("spawning must succeed");
        assert!(
            wait_for(Duration::from_secs(5), || worker.is_finished()),
            "the body returns immediately, so the thread must finish"
        );
        worker.join().expect("the worker must not panic");
    }

    #[test]
    fn shutdown_is_visible_across_threads_and_idempotent() {
        let signal = ShutdownSignal::new();
        assert!(!signal.is_set());

        let observer = signal.clone();
        let handle = thread::spawn(move || {
            while !observer.is_set() {
                thread::yield_now();
            }
            true
        });

        signal.set();
        signal.set();
        assert!(handle.join().expect("the observer must not panic"));
        assert!(signal.is_set());
    }

    #[test]
    fn several_workers_run_concurrently_and_all_shut_down() {
        let started = Arc::new(AtomicU32::new(0));
        let workers: Vec<Worker> = (0..4)
            .map(|index| {
                let counter = Arc::clone(&started);
                Worker::spawn(format!("worker-{index}"), move |shutdown| {
                    counter.fetch_add(1, Ordering::Relaxed);
                    while !shutdown.is_set() {
                        thread::yield_now();
                    }
                })
                .expect("spawning must succeed")
            })
            .collect();

        assert!(
            wait_for(Duration::from_secs(5), || started.load(Ordering::Relaxed)
                == 4),
            "all four workers must start"
        );

        for worker in workers {
            worker.join().expect("no worker must panic");
        }
    }

    #[test]
    fn the_recommended_worker_count_leaves_a_core_for_the_main_thread() {
        let count = recommended_worker_count();
        assert!(count >= 1, "there must always be at least one worker");
        if let Ok(cores) = thread::available_parallelism() {
            assert!(count < cores.get() || cores.get() == 1);
        }
    }
}
