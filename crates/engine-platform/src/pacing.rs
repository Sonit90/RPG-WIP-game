//! Frame pacing: holding the loop to a frame rate without burning a core to do it.
//!
//! ```text
//! frame work .......|---- sleep (timer) ----|- spin -|  next frame
//!                                            ^ deadline - SPIN_MARGIN
//! ```
//!
//! A frame limiter has two ways to be bad. Sleeping the whole remainder misses the deadline
//! by whatever the scheduler feels like — a millisecond or more, which at 240 FPS is a quarter
//! of the frame. Spinning the whole remainder hits it exactly and costs a full core, which is
//! the opposite of what a player capping their frame rate wants. So the wait sleeps on a
//! high-resolution waitable timer until [`SPIN_MARGIN`] before the deadline, then yields in a
//! loop for the rest.
//!
//! ## The schedule
//!
//! Deadlines are spaced from the previous *deadline*, not from when the wait happened to
//! return: an oversleep of 0.2 ms then shortens the next wait by 0.2 ms instead of drifting
//! the rate. A frame that is already late starts at once and re-anchors the schedule there,
//! so a hitch is never followed by a burst of unthrottled catch-up frames.
//! [`next_release`] is that rule, and it is pure so it is tested without a clock.

use std::time::{Duration, Instant};

#[cfg(windows)]
use windows::Win32::Foundation::{CloseHandle, HANDLE};
#[cfg(windows)]
use windows::Win32::System::Threading::{
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW, INFINITE, SetWaitableTimer,
    TIMER_ALL_ACCESS, WaitForSingleObject,
};
#[cfg(windows)]
use windows::core::PCWSTR;

/// How long before a deadline the wait stops sleeping and starts yielding.
///
/// A high-resolution timer wakes within a few hundred microseconds of when it was asked to;
/// one millisecond covers that with room left for a thread that is not scheduled straight
/// away.
pub const SPIN_MARGIN: Duration = Duration::from_millis(1);

/// The margin used when no high-resolution timer could be created.
///
/// `std::thread::sleep` may then be rounded up to the system timer's tick, which on an
/// untouched machine is about 15.6 ms. Two milliseconds does not cover that — nothing short
/// of spinning would — but it keeps the common 1 ms-resolution case on time.
#[cfg(windows)]
const FALLBACK_SPIN_MARGIN: Duration = Duration::from_millis(2);

/// The length of one frame at `fps`, or `None` for no limit.
///
/// Zero frames a second is treated as no limit rather than as an infinite wait.
#[must_use]
pub fn frame_period(fps: Option<u32>) -> Option<Duration> {
    match fps {
        Some(fps) if fps > 0 => Some(Duration::from_secs(1) / fps),
        _ => None,
    }
}

/// When the next frame may start, given when the previous one was released.
///
/// `None` for `previous` means there is no schedule yet — the first frame under a new limit —
/// and the frame starts now. A frame whose slot has already passed also starts now, and the
/// schedule restarts from there.
#[must_use]
pub fn next_release(previous: Option<Instant>, now: Instant, period: Duration) -> Instant {
    match previous {
        Some(previous) => {
            let target = previous + period;
            if target > now { target } else { now }
        }
        None => now,
    }
}

/// Holds the frame loop to a frame rate.
///
/// Owns one high-resolution waitable timer for the life of the loop, so a wait creates no
/// kernel object. Where that timer is not available — Windows before 10 version 1803 — it
/// falls back to `std::thread::sleep` and a wider spin.
#[cfg(windows)]
#[derive(Debug)]
pub struct FramePacer {
    /// The waitable timer, or `None` if one could not be created.
    timer: Option<HANDLE>,
    /// When the previous frame was released. `None` while no limit is in force.
    last_release: Option<Instant>,
}

#[cfg(windows)]
impl FramePacer {
    /// Creates a pacer. Never fails: without a timer it still paces, less economically.
    #[must_use]
    pub fn new() -> Self {
        // SAFETY: FFI call with no pointers but a null name and no security attributes; the
        // returned handle is owned by this pacer and closed in `drop`.
        let timer = unsafe {
            CreateWaitableTimerExW(
                None,
                PCWSTR::null(),
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS.0,
            )
        };
        let timer = match timer {
            Ok(handle) => Some(handle),
            Err(error) => {
                log::warn!(
                    "pacing: no high-resolution timer ({error}); frame limits use sleep and spin"
                );
                None
            }
        };
        Self {
            timer,
            last_release: None,
        }
    }

    /// Whether waits sleep on a high-resolution timer.
    #[must_use]
    pub fn has_high_resolution_timer(&self) -> bool {
        self.timer.is_some()
    }

    /// Blocks until the next frame may start under `fps` frames a second.
    ///
    /// Call once per frame, after the frame's work. `None` returns at once and forgets the
    /// schedule, so lifting a limit and setting it again does not wait for a slot computed
    /// from a frame long gone.
    pub fn wait(&mut self, fps: Option<u32>) {
        let Some(period) = frame_period(fps) else {
            self.last_release = None;
            return;
        };
        let release = next_release(self.last_release, Instant::now(), period);
        self.wait_until(release);
        self.last_release = Some(release);
    }

    /// Sleeps until shortly before `deadline`, then yields until it has passed.
    fn wait_until(&self, deadline: Instant) {
        let margin = if self.timer.is_some() {
            SPIN_MARGIN
        } else {
            FALLBACK_SPIN_MARGIN
        };
        let now = Instant::now();
        if let Some(remaining) = deadline.checked_duration_since(now)
            && remaining > margin
        {
            self.sleep(remaining - margin);
        }
        // Yield rather than a bare spin hint: another thread of this process — the renderer's
        // or a job worker — may be what needs the core for the last millisecond.
        while Instant::now() < deadline {
            std::thread::yield_now();
        }
    }

    /// Sleeps for about `duration`, on the timer if there is one.
    fn sleep(&self, duration: Duration) {
        let Some(timer) = self.timer else {
            std::thread::sleep(duration);
            return;
        };
        // Negative means relative, in 100-nanosecond units.
        let hundreds_of_nanoseconds = i64::try_from(duration.as_nanos() / 100).unwrap_or(i64::MAX);
        let due = -hundreds_of_nanoseconds.max(1);
        // SAFETY: `timer` is a live waitable timer owned by `self`; `due` outlives the call,
        // and no completion routine is registered.
        let armed = unsafe { SetWaitableTimer(timer, &due, 0, None, None, false) };
        match armed {
            // SAFETY: waits on the handle armed above, which stays open for the call.
            Ok(()) => unsafe {
                WaitForSingleObject(timer, INFINITE);
            },
            Err(error) => {
                log::trace!("pacing: could not arm the timer ({error}); sleeping instead");
                std::thread::sleep(duration);
            }
        }
    }
}

#[cfg(windows)]
impl Default for FramePacer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(windows)]
impl Drop for FramePacer {
    fn drop(&mut self) {
        if let Some(timer) = self.timer.take() {
            // SAFETY: the handle was created by this pacer and is closed exactly once.
            if let Err(error) = unsafe { CloseHandle(timer) } {
                log::warn!("pacing: could not close the frame timer: {error}");
            }
        }
    }
}

// macOS has no waitable timer to own; its pacer is a sleep-and-spin one in its own file, and
// shares the pure schedule above.
#[cfg(target_os = "macos")]
#[path = "macos/pacing.rs"]
mod native;
#[cfg(target_os = "macos")]
pub use native::FramePacer;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_limit_is_a_period_and_none_or_zero_is_no_limit() {
        assert_eq!(frame_period(None), None);
        assert_eq!(frame_period(Some(0)), None);
        assert_eq!(frame_period(Some(1)), Some(Duration::from_secs(1)));
        let at_60 = frame_period(Some(60)).unwrap_or_default();
        assert!(at_60 > Duration::from_micros(16_666) && at_60 < Duration::from_micros(16_668));
    }

    #[test]
    fn the_first_frame_under_a_limit_starts_at_once() {
        let now = Instant::now();
        assert_eq!(next_release(None, now, Duration::from_millis(10)), now);
    }

    #[test]
    fn an_early_frame_waits_for_its_slot() {
        let start = Instant::now();
        let period = Duration::from_millis(10);
        let now = start + Duration::from_millis(3);
        assert_eq!(next_release(Some(start), now, period), start + period);
    }

    #[test]
    fn slots_are_spaced_from_the_last_slot_so_an_oversleep_does_not_drift_the_rate() {
        let start = Instant::now();
        let period = Duration::from_millis(10);
        // The previous wait woke 0.4 ms late; the next slot is still exactly one period on.
        let first = next_release(Some(start), start + Duration::from_millis(2), period);
        let woke = first + Duration::from_micros(400);
        let second = next_release(Some(first), woke + Duration::from_millis(2), period);
        assert_eq!(second, start + period * 2);
    }

    #[test]
    fn a_late_frame_starts_at_once_and_restarts_the_schedule() {
        let start = Instant::now();
        let period = Duration::from_millis(10);
        let late = start + Duration::from_millis(35);
        let release = next_release(Some(start), late, period);
        assert_eq!(release, late, "a hitch must not be followed by a burst");
        assert_eq!(
            next_release(Some(release), late, period),
            late + period,
            "the next slot is one period after the late frame"
        );
    }

    #[test]
    fn no_limit_never_waits() {
        let mut pacer = FramePacer::new();
        let started = Instant::now();
        for _ in 0..1_000 {
            pacer.wait(None);
        }
        assert!(started.elapsed() < Duration::from_millis(50));
    }

    #[test]
    fn a_limit_holds_the_loop_to_its_rate() {
        // 200 FPS for 20 frames is 95 ms of slots after the first, which starts at once.
        // Generous above, strict below: a busy test machine can be late, never early.
        let mut pacer = FramePacer::new();
        let started = Instant::now();
        for _ in 0..20 {
            pacer.wait(Some(200));
        }
        let elapsed = started.elapsed();
        assert!(
            elapsed >= Duration::from_millis(95),
            "{elapsed:?}: too fast"
        );
        assert!(
            elapsed < Duration::from_millis(400),
            "{elapsed:?}: far too slow"
        );
    }
}
