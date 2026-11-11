//! Startup timing: where the seconds before the first frame go.
//!
//! ```text
//!  main ──► begin() ──► stage("window") ──► stage("D3D12") ──► ... ──► report()
//! ```
//!
//! # What this is, and what it is not
//!
//! A measuring tape, not a profiler. One global stopwatch, a `&'static str` and a `Duration`
//! per stage, and a table printed once when the first frame is on screen. It is deliberately
//! small enough to delete in one commit: nothing outside this file holds any state, and every
//! call site is one line — [`stage`] or [`stage_from`].
//!
//! # Why a global
//!
//! The stages it measures are spread across `main`, `App::new`, the first frame and the
//! renderer's own device creation, and threading a stopwatch through all of them would be the
//! refactor this measurement exists to avoid. The mutex is taken about a dozen times in the
//! life of the process.
//!
//! Timings are wall clock and include everything the operating system did in that window —
//! page faults, driver work, disk. That is the point: the question is why a launch takes a
//! second, not how many cycles a function burns.
//!
//! # Not in a shipping build
//!
//! Behind the `startup-timing` feature. Without it this module is five empty inline functions
//! and nothing else — no mutex, no stage names, no `Instant`, no table. The call sites stay
//! where they are and cost nothing, which is what keeps the measurement removable without
//! putting `#[cfg]` on a dozen lines spread across the application.

#[cfg(feature = "startup-timing")]
pub use measured::*;

/// The stopwatch, compiled only when somebody asked to measure.
#[cfg(feature = "startup-timing")]
mod measured {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    /// One measured stage.
    struct Stage {
        name: &'static str,
        elapsed: Duration,
    }

    /// The stopwatch: when the process started, when the last stage ended, and what has been
    /// measured so far.
    struct Stopwatch {
        started: Instant,
        last: Instant,
        stages: Vec<Stage>,
        reported: bool,
    }

    static WATCH: Mutex<Option<Stopwatch>> = Mutex::new(None);

    /// Starts the clock. Called first thing in `main`.
    pub fn begin() {
        let now = Instant::now();
        if let Ok(mut watch) = WATCH.lock() {
            *watch = Some(Stopwatch {
                started: now,
                last: now,
                stages: Vec::with_capacity(16),
                reported: false,
            });
        }
    }

    /// Records everything since the previous stage as `name`.
    pub fn stage(name: &'static str) {
        let now = Instant::now();
        if let Ok(mut guard) = WATCH.lock()
            && let Some(watch) = guard.as_mut()
        {
            let elapsed = now.saturating_duration_since(watch.last);
            watch.last = now;
            watch.stages.push(Stage { name, elapsed });
        }
    }

    /// Records `start .. now` as `name`, without moving the stage boundary.
    ///
    /// For a stage that is measured inside something else — the D3D12 device inside the
    /// renderer's constructor — so that the nested timing appears in the table without the
    /// enclosing stage losing the time to it.
    pub fn stage_from(name: &'static str, start: Instant) {
        let elapsed = start.elapsed();
        if let Ok(mut guard) = WATCH.lock()
            && let Some(watch) = guard.as_mut()
        {
            watch.stages.push(Stage { name, elapsed });
        }
    }

    /// Records a duration somebody else measured, under `name`.
    ///
    /// For the two halves of a stage that alternate in one loop — building a model and uploading
    /// it — where a stage boundary cannot separate them.
    pub fn record(name: &'static str, elapsed: Duration) {
        if let Ok(mut guard) = WATCH.lock()
            && let Some(watch) = guard.as_mut()
        {
            watch.stages.push(Stage { name, elapsed });
        }
    }

    /// Prints the table. Called once, when the first frame has been presented.
    ///
    /// A no-op on every call after the first, so a caller can put it on the present path without
    /// having to know which frame this is.
    pub fn report() {
        let Ok(mut guard) = WATCH.lock() else {
            return;
        };
        let Some(watch) = guard.as_mut() else {
            return;
        };
        if watch.reported {
            return;
        }
        watch.reported = true;
        let total = watch.started.elapsed();
        for stage in &watch.stages {
            log::info!(
                "[startup] {:<22} {:>6.1} ms",
                stage.name,
                stage.elapsed.as_secs_f64() * 1e3
            );
        }
        log::info!(
            "[startup] {:<22} {:>6.1} ms",
            "TOTAL",
            total.as_secs_f64() * 1e3
        );
    }
}

/// The same interface, doing nothing. What a shipping build compiles.
#[cfg(not(feature = "startup-timing"))]
mod unmeasured {
    use std::time::{Duration, Instant};

    /// Does nothing without the `startup-timing` feature.
    #[inline]
    pub fn begin() {}

    /// Does nothing without the `startup-timing` feature.
    #[inline]
    pub fn stage(_name: &'static str) {}

    /// Does nothing without the `startup-timing` feature.
    #[inline]
    pub fn stage_from(_name: &'static str, _start: Instant) {}

    /// Does nothing without the `startup-timing` feature.
    #[inline]
    pub fn record(_name: &'static str, _elapsed: Duration) {}

    /// Does nothing without the `startup-timing` feature.
    #[inline]
    pub fn report() {}
}

#[cfg(not(feature = "startup-timing"))]
pub use unmeasured::*;
