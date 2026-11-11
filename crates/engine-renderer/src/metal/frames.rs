//! Frames in flight: which frame slot the CPU may record into, and when the GPU has finished
//! with it.
//!
//! D3D12 paces the CPU with a fence. Metal has none to wait on, so each committed command
//! buffer carries a completion handler that publishes its serial number here, and the CPU
//! waits on a condition variable until the slot's previous serial has been published.
//!
//! ```text
//! serial 1 -> slot 0     serial 2 -> slot 1     serial 3 -> slot 2     serial 4 -> slot 0
//!                                                                       ^ waits for serial 1
//! ```
//!
//! Command buffers on one queue complete in order, so "the highest serial published" is
//! "everything up to it is done" and the handlers need not run in order themselves.

use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use super::api_error;
use crate::error::RendererError;
use crate::frame::{MAX_FRAMES_IN_FLIGHT, SyncStats};

/// Longest the CPU waits for the GPU before reporting a hang instead of blocking forever.
const WAIT_LIMIT: Duration = Duration::from_secs(10);

/// What completion handlers write and the CPU reads.
#[derive(Debug, Default)]
struct Completion {
    /// Highest serial whose command buffer has finished.
    completed: u64,
    /// GPU time of the most recently finished frame, in milliseconds, not yet read.
    gpu_ms: Option<f64>,
}

/// The state shared with completion handlers, which run on a Metal thread.
#[derive(Debug, Default)]
pub(crate) struct FrameShared {
    state: Mutex<Completion>,
    signal: Condvar,
}

impl FrameShared {
    fn lock(&self) -> MutexGuard<'_, Completion> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Called from a command buffer's completion handler.
    pub(crate) fn complete(&self, serial: u64, gpu_ms: Option<f64>) {
        let mut state = self.lock();
        state.completed = state.completed.max(serial);
        if gpu_ms.is_some() {
            state.gpu_ms = gpu_ms;
        }
        drop(state);
        self.signal.notify_all();
    }

    /// Highest serial the GPU has finished.
    pub(crate) fn completed(&self) -> u64 {
        self.lock().completed
    }

    /// The GPU time of the latest finished frame, once.
    fn take_gpu_ms(&self) -> Option<f64> {
        self.lock().gpu_ms.take()
    }

    /// Blocks until `serial` has completed, returning how long that took (zero when it had).
    fn wait_for(&self, serial: u64) -> Result<Duration, RendererError> {
        let mut state = self.lock();
        if state.completed >= serial {
            return Ok(Duration::ZERO);
        }
        let started = Instant::now();
        while state.completed < serial {
            let remaining = WAIT_LIMIT.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return Err(api_error(
                    "MTLCommandBuffer completion",
                    format!("the GPU did not finish frame {serial} within {WAIT_LIMIT:?}"),
                ));
            }
            state = self
                .signal
                .wait_timeout(state, remaining)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        Ok(started.elapsed())
    }
}

/// The ring of frame slots and the serials that pace them.
pub(crate) struct FrameRing {
    shared: Arc<FrameShared>,
    /// Serial of the last command buffer committed from each slot; zero means never.
    slot_serials: Vec<u64>,
    /// Serial given to the most recently committed frame.
    last_serial: u64,
    /// Frames finished with `end_frame`. The slot is this modulo the slot count.
    frame_index: u64,
    stats: SyncStats,
}

impl FrameRing {
    /// A ring of `frames_in_flight` slots, clamped to `1..=MAX_FRAMES_IN_FLIGHT`.
    pub(crate) fn new(frames_in_flight: usize) -> Self {
        let slots = frames_in_flight.clamp(1, MAX_FRAMES_IN_FLIGHT);
        if slots != frames_in_flight {
            log::warn!(
                "frames in flight {frames_in_flight} is outside 1..={MAX_FRAMES_IN_FLIGHT}; \
                 using {slots}"
            );
        }
        Self {
            shared: Arc::new(FrameShared::default()),
            slot_serials: vec![0; slots],
            last_serial: 0,
            frame_index: 0,
            stats: SyncStats::default(),
        }
    }

    /// The state completion handlers publish into.
    pub(crate) fn shared(&self) -> Arc<FrameShared> {
        Arc::clone(&self.shared)
    }

    /// Number of slots.
    pub(crate) fn frames_in_flight(&self) -> usize {
        self.slot_serials.len()
    }

    /// Frames finished since creation; the dynamic meshes rotate their copies by it.
    pub(crate) fn frame_index(&self) -> u64 {
        self.frame_index
    }

    /// Synchronization measurements.
    pub(crate) fn stats(&self) -> SyncStats {
        self.stats
    }

    /// Highest serial the GPU has finished.
    pub(crate) fn completed(&self) -> u64 {
        self.shared.completed()
    }

    /// Serial of the last frame committed, which a resource used by every frame so far can
    /// be released after.
    pub(crate) fn last_committed(&self) -> u64 {
        self.last_serial
    }

    /// The GPU time of the latest finished frame in milliseconds, once.
    pub(crate) fn take_gpu_ms(&self) -> Option<f64> {
        self.shared.take_gpu_ms()
    }

    /// Waits until the current slot's previous frame has finished and returns the slot.
    ///
    /// Only blocks when the CPU is a whole lap ahead; the stall is measured, and a frame that
    /// did not wait never samples the clock.
    ///
    /// # Errors
    ///
    /// [`RendererError::Api`] if the GPU does not finish within [`WAIT_LIMIT`].
    pub(crate) fn begin_frame(&mut self) -> Result<usize, RendererError> {
        let slot = (self.frame_index % self.slot_serials.len() as u64) as usize;
        let waited = self.shared.wait_for(self.slot_serials[slot])?;
        self.stats.total_frames += 1;
        if !waited.is_zero() {
            self.stats.stalled_frames += 1;
            self.stats.total_stall += waited;
            self.stats.last_stall = waited;
            self.stats.max_stall = self.stats.max_stall.max(waited);
        }
        Ok(slot)
    }

    /// Records that `slot`'s frame was committed, and returns the serial its completion
    /// handler must publish.
    pub(crate) fn next_serial(&mut self) -> u64 {
        self.last_serial += 1;
        self.last_serial
    }

    /// Ties `serial` to `slot` and moves the ring to the next slot.
    pub(crate) fn end_frame(&mut self, slot: usize, serial: u64) {
        self.slot_serials[slot] = serial;
        self.frame_index += 1;
    }

    /// Blocks until everything committed so far has finished.
    ///
    /// # Errors
    ///
    /// [`RendererError::Api`] if the GPU does not finish within [`WAIT_LIMIT`].
    pub(crate) fn wait_for_idle(&self) -> Result<(), RendererError> {
        self.shared.wait_for(self.last_serial).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slot_waits_only_for_its_own_previous_frame() {
        let mut ring = FrameRing::new(2);
        let shared = ring.shared();
        let slot = ring.begin_frame().unwrap_or(usize::MAX);
        assert_eq!(slot, 0);
        let first = ring.next_serial();
        ring.end_frame(slot, first);
        // Slot 1 has never been used, so it does not wait for slot 0's frame.
        assert_eq!(ring.begin_frame().unwrap_or(usize::MAX), 1);
        let second = ring.next_serial();
        ring.end_frame(1, second);
        shared.complete(first, Some(1.5));
        // Slot 0 again: its previous serial has completed.
        assert_eq!(ring.begin_frame().unwrap_or(usize::MAX), 0);
        assert_eq!(ring.take_gpu_ms(), Some(1.5));
        assert_eq!(ring.stats().stalled_frames, 0);
    }

    #[test]
    fn the_slot_count_is_clamped() {
        assert_eq!(FrameRing::new(0).frames_in_flight(), 1);
        assert_eq!(FrameRing::new(99).frames_in_flight(), MAX_FRAMES_IN_FLIGHT);
    }
}
