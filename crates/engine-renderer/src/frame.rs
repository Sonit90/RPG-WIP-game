//! Frames in flight: per-slot command allocators, fence bookkeeping, and the measurement
//! of how often the CPU actually waits for the GPU.
//!
//! ```text
//! slot 0        slot 1        slot 2
//! ├ allocator   ├ allocator   ├ allocator
//! └ fence value └ fence value └ fence value
//! ```
//!
//! ## Why slots exist at all
//!
//! A command allocator's memory cannot be reset while the GPU is still executing commands
//! recorded from it. With one allocator the CPU would have to wait for the GPU every
//! frame, which serializes the two processors and throws away most of the machine. With
//! `N` allocators the CPU can run `N - 1` frames ahead before it must wait.
//!
//! ## Why the wait is measured rather than assumed
//!
//! "The CPU does not block" is a claim, and claims about performance are worth exactly
//! what their measurements are worth. [`FrameRing`] counts how many frames actually had to
//! wait and for how long, so the claim becomes a number in the statistics.
//!
//! The check is free when it is not needed: the fence's completed value is read first, and
//! the clock is only sampled on the path that genuinely blocks.
//!
//! ## Frame slot is not back-buffer index
//!
//! Phase 0 used the swap chain's current back-buffer index as the frame slot. That works
//! only while the two counts are equal and the presentation model hands them out in the
//! same order. They are separate concepts — a slot is CPU-side recording state, a back
//! buffer is a texture — so this module owns its own monotonic frame counter and derives
//! the slot from it.

use std::time::{Duration, Instant};

use windows::Win32::Graphics::Direct3D12::{ID3D12CommandAllocator, ID3D12CommandQueue};

use crate::device::RenderDevice;
use crate::error::RendererError;
use crate::sync::GpuFence;

/// Default number of frames the CPU may run ahead of the GPU.
///
/// Three matches the swap chain's buffer count: two is enough to overlap CPU and GPU, and
/// the third absorbs a frame that runs long without forcing a stall. More would add
/// latency for no throughput, since presentation would become the limit.
pub const DEFAULT_FRAMES_IN_FLIGHT: usize = 3;

/// Largest supported value, bounded by the swap chain's buffer count.
///
/// Queueing more frames than there are back buffers cannot help: `Present` blocks once
/// every buffer is spoken for.
pub const MAX_FRAMES_IN_FLIGHT: usize = crate::swapchain::FRAME_COUNT;

/// How often, and for how long, the CPU waited for the GPU.
///
/// A healthy uncapped renderer stalls on nearly every frame — that is what it means to be
/// GPU-bound — while a CPU-bound one stalls rarely. The number is diagnostic, not a
/// target: it says which processor is the limit.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SyncStats {
    /// Frames whose pacing wait had to block.
    pub stalled_frames: u64,
    /// Frames begun in total.
    pub total_frames: u64,
    /// Time spent blocked, summed.
    pub total_stall: Duration,
    /// Time spent blocked in the most recent stalling frame.
    pub last_stall: Duration,
    /// Longest single stall observed.
    pub max_stall: Duration,
}

impl SyncStats {
    /// Fraction of frames that had to wait, in `0.0..=1.0`.
    #[must_use]
    pub fn stall_ratio(&self) -> f64 {
        if self.total_frames == 0 {
            0.0
        } else {
            self.stalled_frames as f64 / self.total_frames as f64
        }
    }

    /// Mean stall over all frames, in milliseconds — including frames that did not stall.
    #[must_use]
    pub fn mean_stall_ms(&self) -> f64 {
        if self.total_frames == 0 {
            0.0
        } else {
            self.total_stall.as_secs_f64() * 1e3 / self.total_frames as f64
        }
    }
}

/// One frame slot's CPU-side state.
struct Frame {
    allocator: ID3D12CommandAllocator,
    /// Fence value signalled after this slot's work was submitted. The slot cannot be
    /// reused until the GPU has passed it. Zero means "never submitted".
    fence_value: u64,
}

/// The ring of frame slots and the fence that paces them.
pub struct FrameRing {
    frames: Vec<Frame>,
    fence: GpuFence,
    /// Frames begun since creation. The slot is this modulo the slot count.
    frame_index: u64,
    stats: SyncStats,
}

impl FrameRing {
    /// Creates `frames_in_flight` slots, each with its own command allocator.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] if `frames_in_flight` is zero or above
    /// [`MAX_FRAMES_IN_FLIGHT`], or [`RendererError::Api`] if a D3D12 object cannot be
    /// created.
    pub fn new(device: &RenderDevice, frames_in_flight: usize) -> Result<Self, RendererError> {
        if frames_in_flight == 0 || frames_in_flight > MAX_FRAMES_IN_FLIGHT {
            return Err(RendererError::InvalidConfig {
                reason: format!(
                    "frames in flight must be 1..={MAX_FRAMES_IN_FLIGHT}, got {frames_in_flight}"
                ),
            });
        }

        let mut frames = Vec::with_capacity(frames_in_flight);
        for _ in 0..frames_in_flight {
            frames.push(Frame {
                allocator: device.create_command_allocator()?,
                fence_value: 0,
            });
        }

        Ok(Self {
            frames,
            fence: GpuFence::new(device)?,
            frame_index: 0,
            stats: SyncStats::default(),
        })
    }

    /// Number of slots.
    #[must_use]
    pub fn frames_in_flight(&self) -> usize {
        self.frames.len()
    }

    /// Slot the next [`FrameRing::begin_frame`] will use.
    #[must_use]
    pub fn current_slot(&self) -> usize {
        (self.frame_index % self.frames.len() as u64) as usize
    }

    /// Frames begun since creation.
    #[must_use]
    pub fn frame_index(&self) -> u64 {
        self.frame_index
    }

    /// Synchronization measurements.
    #[must_use]
    pub fn stats(&self) -> SyncStats {
        self.stats
    }

    /// Clears the synchronization measurements.
    pub fn reset_stats(&mut self) {
        self.stats = SyncStats::default();
    }

    /// Waits for the current slot to be free, resets its allocator, and returns the slot.
    ///
    /// The wait only blocks when the CPU is a whole lap ahead. The fence's completed value
    /// is checked first, so a frame that does not need to wait pays one atomic read and
    /// never samples the clock.
    ///
    /// After this returns, the slot's allocator holds no in-flight work — which is exactly
    /// the precondition for resetting it, and the point at which the slot's GPU timestamps
    /// from its previous use are known to have resolved.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if the fence wait or the allocator reset fails.
    pub fn begin_frame(&mut self) -> Result<usize, RendererError> {
        let slot = self.current_slot();
        let target = self.frames[slot].fence_value;

        // Reading the completed value is a load from shared memory; the event wait below
        // is a kernel transition. Checking first keeps the fast path out of the kernel and
        // makes "did this frame stall?" an honest measurement rather than a guess.
        if target > 0 && self.fence.completed_value() < target {
            let started = Instant::now();
            self.fence.wait(target)?;
            let stall = started.elapsed();

            self.stats.stalled_frames += 1;
            self.stats.total_stall += stall;
            self.stats.last_stall = stall;
            self.stats.max_stall = self.stats.max_stall.max(stall);
        }

        self.stats.total_frames += 1;

        // SAFETY: FFI call. The GPU has finished every command recorded from this
        // allocator, proven by the fence wait above, which is exactly what Reset requires.
        unsafe { self.frames[slot].allocator.Reset() }
            .map_err(|error| RendererError::api("ID3D12CommandAllocator::Reset", error))?;

        Ok(slot)
    }

    /// The allocator for `slot`.
    #[must_use]
    pub fn allocator(&self, slot: usize) -> &ID3D12CommandAllocator {
        &self.frames[slot].allocator
    }

    /// Signals the fence for `slot` and advances to the next frame.
    ///
    /// Called after the frame's work has been submitted *and* presented, so the recorded
    /// value covers the whole submission.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if the fence signal fails.
    pub fn end_frame(
        &mut self,
        queue: &ID3D12CommandQueue,
        slot: usize,
    ) -> Result<u64, RendererError> {
        let value = self.fence.signal(queue)?;
        self.frames[slot].fence_value = value;
        self.frame_index += 1;
        Ok(value)
    }

    /// Blocks until the GPU has finished everything submitted so far.
    ///
    /// Fully serializing: for shutdown, resize and resource destruction only.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if the signal or wait fails.
    pub fn wait_for_idle(&mut self, queue: &ID3D12CommandQueue) -> Result<(), RendererError> {
        self.fence.wait_for_idle(queue)
    }

    /// Marks every slot as free without waiting.
    ///
    /// Only valid immediately after [`FrameRing::wait_for_idle`], when the GPU provably
    /// holds no reference to any slot's work. Used when the swap chain is recreated: the
    /// old fence values refer to submissions that touched destroyed back buffers, and
    /// waiting on them again would be waiting on the past.
    pub fn release_all_slots(&mut self) {
        let completed = self.fence.completed_value();
        for frame in &mut self.frames {
            frame.fence_value = completed;
        }
    }

    /// The fence value the GPU has passed.
    #[must_use]
    pub fn completed_value(&self) -> u64 {
        self.fence.completed_value()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The slot sequence a ring of `slots` produces over `frames` frames.
    ///
    /// Pure arithmetic, so the cycling rule is tested without a GPU. `FrameRing` uses the
    /// same expression in `current_slot`.
    fn slot_sequence(slots: usize, frames: u64) -> Vec<usize> {
        (0..frames)
            .map(|index| (index % slots as u64) as usize)
            .collect()
    }

    #[test]
    fn slots_cycle_and_repeat_only_after_a_full_lap() {
        assert_eq!(slot_sequence(3, 7), [0, 1, 2, 0, 1, 2, 0]);
        assert_eq!(slot_sequence(1, 4), [0, 0, 0, 0]);
        assert_eq!(slot_sequence(2, 5), [0, 1, 0, 1, 0]);
    }

    #[test]
    fn the_default_fits_within_the_maximum() {
        const { assert!(DEFAULT_FRAMES_IN_FLIGHT >= 1) };
        const { assert!(DEFAULT_FRAMES_IN_FLIGHT <= MAX_FRAMES_IN_FLIGHT) };
        assert_eq!(
            MAX_FRAMES_IN_FLIGHT,
            crate::swapchain::FRAME_COUNT,
            "queueing more frames than there are back buffers cannot help"
        );
    }

    #[test]
    fn stall_statistics_start_empty() {
        let stats = SyncStats::default();
        assert_eq!(stats.stall_ratio(), 0.0);
        assert_eq!(stats.mean_stall_ms(), 0.0);
        assert_eq!(stats.stalled_frames, 0);
    }

    #[test]
    fn stall_ratio_and_mean_describe_the_frames_counted() {
        let stats = SyncStats {
            stalled_frames: 3,
            total_frames: 12,
            total_stall: Duration::from_micros(600),
            last_stall: Duration::from_micros(150),
            max_stall: Duration::from_micros(300),
        };
        assert!((stats.stall_ratio() - 0.25).abs() < 1e-12);
        // 600 us over 12 frames, including the nine that did not stall.
        assert!((stats.mean_stall_ms() - 0.05).abs() < 1e-12);
    }

    #[test]
    fn a_ring_that_never_stalls_reports_a_zero_ratio() {
        let stats = SyncStats {
            stalled_frames: 0,
            total_frames: 1000,
            ..SyncStats::default()
        };
        assert_eq!(stats.stall_ratio(), 0.0);
        assert_eq!(stats.mean_stall_ms(), 0.0);
    }
}
