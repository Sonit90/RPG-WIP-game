//! The macOS frame pacer: sleep for the bulk of the wait, then yield up to the deadline.
//!
//! Windows needs a high-resolution waitable timer because its default timer tick is 15.6 ms.
//! macOS has no such tick: `std::thread::sleep` is `nanosleep`/`mach_wait_until` and wakes
//! within about a hundred microseconds on an idle core, so plain sleep plus the same short
//! spin the Windows pacer ends with is enough. The schedule itself ([`next_release`]) is the
//! shared pure rule, identical to Windows.

use std::time::Instant;

use super::{SPIN_MARGIN, frame_period, next_release};

/// Holds the frame loop to a frame rate.
///
/// Stateless apart from the schedule: there is no kernel object to own, unlike the Windows
/// pacer's timer.
#[derive(Debug)]
pub struct FramePacer {
    /// When the previous frame was released. `None` while no limit is in force.
    last_release: Option<Instant>,
}

impl FramePacer {
    /// Creates a pacer. Never fails.
    #[must_use]
    pub fn new() -> Self {
        Self { last_release: None }
    }

    /// Whether waits sleep on a high-resolution timer.
    ///
    /// Always true on macOS: the system sleep is already precise enough, so it is the
    /// equivalent of the Windows high-resolution timer for the engine's purposes.
    #[must_use]
    pub fn has_high_resolution_timer(&self) -> bool {
        true
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
        wait_until(release);
        self.last_release = Some(release);
    }
}

/// Sleeps until [`SPIN_MARGIN`] before `deadline`, then yields until it has passed.
fn wait_until(deadline: Instant) {
    if let Some(remaining) = deadline.checked_duration_since(Instant::now())
        && remaining > SPIN_MARGIN
    {
        std::thread::sleep(remaining - SPIN_MARGIN);
    }
    // Yield rather than a bare spin hint: another thread of this process — the renderer's or
    // a job worker — may be what needs the core for the last millisecond.
    while Instant::now() < deadline {
        std::thread::yield_now();
    }
}

impl Default for FramePacer {
    fn default() -> Self {
        Self::new()
    }
}
