//! Frame timing: a monotonic clock, real/simulation time, and a statistics window.
//!
//! Everything here is pure CPU-side timing built on [`Instant`], which is backed by
//! `QueryPerformanceCounter` on Windows (sub-microsecond resolution) and is monotonic:
//! it cannot jump backwards when the wall clock is adjusted or when NTP steps the system
//! time. Wall-clock time is never used for frame timing. GPU time cannot be derived from
//! these numbers — it needs GPU timestamps, which the renderer provides.
//!
//! Allocation policy: the sample window and its sort scratch are allocated once in
//! [`FrameStatsWindow::new`]. Recording a sample and computing statistics never allocate,
//! so the frame path stays allocation-free.

use std::time::{Duration, Instant};

/// Monotonic per-frame clock.
///
/// `tick` returns the time since the previous tick, which is the frame delta the
/// simulation should advance by.
#[derive(Debug)]
pub struct FrameClock {
    start: Instant,
    last: Instant,
    frame_index: u64,
}

impl Default for FrameClock {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameClock {
    /// Starts the clock. The first [`FrameClock::tick`] measures from this moment.
    #[must_use]
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            start: now,
            last: now,
            frame_index: 0,
        }
    }

    /// Ends the current frame and returns its duration.
    pub fn tick(&mut self) -> Duration {
        let now = Instant::now();
        let delta = now.duration_since(self.last);
        self.last = now;
        self.frame_index += 1;
        delta
    }

    /// Time since the clock was created.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        self.last.duration_since(self.start)
    }

    /// Number of completed ticks.
    #[must_use]
    pub fn frame_index(&self) -> u64 {
        self.frame_index
    }
}

/// Longest frame delta the simulation is allowed to see, by default.
///
/// A real frame never takes this long at the engine's frame-rate targets, so a larger
/// delta means the process was stalled: a breakpoint, a window drag, a driver reset, a
/// swapped-out page. Feeding that to a simulation makes bodies teleport through walls
/// and ability timers fire in a burst. Real time still records the true value; only the
/// simulation delta is clamped.
pub const DEFAULT_MAX_DELTA: Duration = Duration::from_millis(250);

/// The engine's clock: real time, simulation time and the frame counter.
///
/// Two timelines, deliberately separate:
///
/// * **Real time** is what the frame actually took. Frame-time statistics, profiling and
///   anything measuring performance must use it, unclamped and unscaled.
/// * **Simulation time** is what gameplay advances by. It is the real delta clamped to
///   [`Time::max_delta`] and multiplied by [`Time::scale`], and it stops while paused.
///
/// Keeping them apart is what allows pausing, slow motion and a stalled frame without
/// corrupting either performance measurement or simulation behaviour.
///
/// A fixed timestep is not implemented yet, but the shape supports it: a future fixed-step
/// loop accumulates [`Time::delta`] and consumes it in fixed increments, and because the
/// simulation only ever reads this type, nothing else has to change.
///
/// Allocation policy: plain scalars, no allocation anywhere.
#[derive(Debug)]
pub struct Time {
    clock: FrameClock,
    frame_index: u64,
    real_delta: Duration,
    real_elapsed: Duration,
    simulation_delta: Duration,
    simulation_elapsed: Duration,
    max_delta: Duration,
    scale: f64,
    paused: bool,
}

impl Default for Time {
    fn default() -> Self {
        Self::new()
    }
}

impl Time {
    /// Starts both timelines at zero.
    #[must_use]
    pub fn new() -> Self {
        Self {
            clock: FrameClock::new(),
            frame_index: 0,
            real_delta: Duration::ZERO,
            real_elapsed: Duration::ZERO,
            simulation_delta: Duration::ZERO,
            simulation_elapsed: Duration::ZERO,
            max_delta: DEFAULT_MAX_DELTA,
            scale: 1.0,
            paused: false,
        }
    }

    /// Ends the current frame: samples the monotonic clock and advances both timelines.
    ///
    /// Called once per frame by the engine loop. Nothing else should call it — two callers
    /// would split one frame's duration across two deltas.
    pub fn advance(&mut self) {
        let real_delta = self.clock.tick();
        self.advance_by(real_delta);
    }

    /// Advances both timelines by a supplied real delta instead of sampling the clock.
    ///
    /// The clock is left untouched, so mixing this with [`Time::advance`] in one run makes
    /// real time disagree with wall time. That is the point: it is how tests exercise
    /// clamping and scaling without sleeping, and it is the hook a future deterministic
    /// replay needs to drive the simulation from recorded deltas rather than from this
    /// machine's clock.
    pub fn advance_by(&mut self, real_delta: Duration) {
        self.frame_index += 1;
        self.real_delta = real_delta;
        self.real_elapsed += real_delta;
        self.simulation_delta = self.scaled_delta(real_delta);
        self.simulation_elapsed += self.simulation_delta;
    }

    /// Applies the pause, clamp and scale rules to a real delta.
    fn scaled_delta(&self, real: Duration) -> Duration {
        if self.paused {
            return Duration::ZERO;
        }
        let clamped = real.min(self.max_delta);
        if self.scale == 1.0 {
            clamped
        } else {
            clamped.mul_f64(self.scale)
        }
    }

    /// Duration of the previous frame, as it really happened.
    ///
    /// Never clamped, never scaled, unaffected by pause. This is the number performance
    /// measurement must use.
    #[must_use]
    pub fn real_delta(&self) -> Duration {
        self.real_delta
    }

    /// Time since the clock started, as it really happened.
    ///
    /// The sum of every real delta, which for clock-driven frames is exactly the time the
    /// clock has been running.
    #[must_use]
    pub fn real_elapsed(&self) -> Duration {
        self.real_elapsed
    }

    /// Duration the simulation should advance by this frame.
    ///
    /// Zero on the first frame and while paused, so simulation code must tolerate a zero
    /// delta rather than dividing by it.
    #[must_use]
    pub fn delta(&self) -> Duration {
        self.simulation_delta
    }

    /// Simulation time accumulated so far.
    ///
    /// The sum of every [`Time::delta`], so it is behind [`Time::real_elapsed`] by every
    /// paused, slowed or clamped frame.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        self.simulation_elapsed
    }

    /// Simulation delta in seconds, the form gameplay integration usually wants.
    #[must_use]
    pub fn delta_seconds(&self) -> f32 {
        self.simulation_delta.as_secs_f32()
    }

    /// Number of completed frames.
    #[must_use]
    pub fn frame_index(&self) -> u64 {
        self.frame_index
    }

    /// Rate simulation time passes at relative to real time. `1.0` is normal speed.
    #[must_use]
    pub fn scale(&self) -> f64 {
        self.scale
    }

    /// Sets the simulation rate. Negative values are clamped to zero: time does not
    /// run backwards, and a rewind is a replay feature, not a clock feature.
    pub fn set_scale(&mut self, scale: f64) {
        self.scale = if scale.is_finite() {
            scale.max(0.0)
        } else {
            1.0
        };
    }

    /// Whether simulation time is stopped.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Stops or resumes simulation time. Real time keeps running either way.
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    /// Longest delta the simulation may see in one frame.
    #[must_use]
    pub fn max_delta(&self) -> Duration {
        self.max_delta
    }

    /// Sets the clamp. A zero clamp freezes the simulation, so it is rejected in favour
    /// of the default.
    pub fn set_max_delta(&mut self, max_delta: Duration) {
        self.max_delta = if max_delta.is_zero() {
            DEFAULT_MAX_DELTA
        } else {
            max_delta
        };
    }

    /// Restarts both timelines and the frame counter, keeping scale, pause and clamp.
    ///
    /// Used when the workload changes: old frame times describe a different scene, and
    /// blending the two produces statistics that describe neither.
    pub fn reset(&mut self) {
        self.clock = FrameClock::new();
        self.frame_index = 0;
        self.real_delta = Duration::ZERO;
        self.real_elapsed = Duration::ZERO;
        self.simulation_delta = Duration::ZERO;
        self.simulation_elapsed = Duration::ZERO;
    }
}

/// Simulation ticks per second the engine runs gameplay at.
///
/// 60 Hz is 16.67 ms of gameplay resolution. The renderer runs anywhere from 240 to 1500
/// FPS, and a simulation tied to that rate would give a player at 1400 FPS a different
/// jump arc than one at 240 FPS, make cooldowns frame-rate dependent, and make a recorded
/// replay unreproducible.
pub const DEFAULT_TICK_HZ: u32 = 60;

/// Longest run of catch-up ticks a single [`FixedTimestep::accumulate`] may return.
///
/// Without a bound, a frame that took a second asks for sixty ticks, each of which takes
/// time, which makes the next frame longer still — the spiral of death. Past the bound the
/// simulation runs slow for a moment, which is visible and recoverable, rather than
/// stopping.
pub const DEFAULT_MAX_CATCH_UP_TICKS: u32 = 8;

/// Turns a variable frame delta into a whole number of fixed simulation ticks.
///
/// ```text
/// render delta 3.7 ms ->  accumulate -> 0 ticks (accumulator 3.7 ms)
/// render delta 3.7 ms ->  accumulate -> 0 ticks (accumulator 7.4 ms)
/// ...
/// render delta 3.7 ms ->  accumulate -> 1 tick  (accumulator 1.1 ms)
/// ```
///
/// The leftover is kept, so ticks happen at a fixed rate on average no matter how the
/// frames fall. [`FixedTimestep::alpha`] exposes the leftover as a fraction of a step, for
/// a renderer that wants to interpolate between the last two simulation states.
///
/// Allocation policy: plain scalars, no allocation anywhere.
#[derive(Debug, Clone)]
pub struct FixedTimestep {
    step: Duration,
    accumulator: Duration,
    max_catch_up_ticks: u32,
    ticks: u64,
    dropped: u64,
}

impl Default for FixedTimestep {
    fn default() -> Self {
        Self::new(DEFAULT_TICK_HZ)
    }
}

impl FixedTimestep {
    /// A timestep running `hz` times a second.
    ///
    /// A zero rate falls back to [`DEFAULT_TICK_HZ`]: a zero-length step would make every
    /// frame ask for an infinite number of ticks.
    #[must_use]
    pub fn new(hz: u32) -> Self {
        let hz = if hz == 0 { DEFAULT_TICK_HZ } else { hz };
        Self::with_step(Duration::from_secs_f64(1.0 / f64::from(hz)))
    }

    /// A timestep with an explicit step duration.
    #[must_use]
    pub fn with_step(step: Duration) -> Self {
        Self {
            step: if step.is_zero() {
                Duration::from_secs_f64(1.0 / f64::from(DEFAULT_TICK_HZ))
            } else {
                step
            },
            accumulator: Duration::ZERO,
            max_catch_up_ticks: DEFAULT_MAX_CATCH_UP_TICKS,
            ticks: 0,
            dropped: 0,
        }
    }

    /// Sets how many catch-up ticks one call may return.
    ///
    /// Zero is rejected in favour of one: a timestep that can never tick is a stopped
    /// simulation, which is what pausing is for.
    pub fn set_max_catch_up_ticks(&mut self, ticks: u32) {
        self.max_catch_up_ticks = ticks.max(1);
    }

    /// Adds a frame delta and returns how many ticks to run now.
    ///
    /// The remainder stays in the accumulator. Beyond [`FixedTimestep::max_catch_up_ticks`]
    /// the surplus is discarded and counted in [`FixedTimestep::dropped_ticks`], because
    /// running it would make the next frame longer still.
    pub fn accumulate(&mut self, delta: Duration) -> u32 {
        self.accumulator += delta;

        let mut ticks = 0;
        while self.accumulator >= self.step && ticks < self.max_catch_up_ticks {
            self.accumulator -= self.step;
            ticks += 1;
        }
        self.ticks += u64::from(ticks);

        if self.accumulator >= self.step {
            // Whole steps that will never run. Counted rather than kept, so a stall cannot
            // leave the simulation permanently behind the clock.
            let surplus = self.accumulator.as_nanos() / self.step.as_nanos();
            self.dropped += surplus as u64;
            self.accumulator = Duration::ZERO;
        }
        ticks
    }

    /// The fixed step.
    #[must_use]
    pub fn step(&self) -> Duration {
        self.step
    }

    /// The fixed step in seconds, the form gameplay integration wants.
    #[must_use]
    pub fn step_seconds(&self) -> f32 {
        self.step.as_secs_f32()
    }

    /// Ticks per second.
    #[must_use]
    pub fn hz(&self) -> f64 {
        1.0 / self.step.as_secs_f64()
    }

    /// How far the next tick is into the future, as a fraction of a step.
    ///
    /// What a renderer interpolating between the previous and current simulation states
    /// blends by. Always in [0, 1).
    #[must_use]
    pub fn alpha(&self) -> f32 {
        (self.accumulator.as_secs_f64() / self.step.as_secs_f64()) as f32
    }

    /// Ticks returned since the timestep was created.
    #[must_use]
    pub fn ticks(&self) -> u64 {
        self.ticks
    }

    /// Whole ticks discarded because the catch-up bound was reached.
    #[must_use]
    pub fn dropped_ticks(&self) -> u64 {
        self.dropped
    }

    /// Most catch-up ticks one [`FixedTimestep::accumulate`] may return.
    #[must_use]
    pub fn max_catch_up_ticks(&self) -> u32 {
        self.max_catch_up_ticks
    }

    /// Empties the accumulator and the counters, keeping the step.
    pub fn reset(&mut self) {
        self.accumulator = Duration::ZERO;
        self.ticks = 0;
        self.dropped = 0;
    }
}

/// Summary of a window of frame times, all in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameStats {
    /// Number of samples the summary was computed from.
    pub count: usize,
    /// Arithmetic mean frame time.
    pub mean_ms: f64,
    /// Fastest frame in the window.
    pub min_ms: f64,
    /// Slowest frame in the window.
    pub max_ms: f64,
    /// Median frame time.
    pub p50_ms: f64,
    /// 95th percentile: one frame in twenty is at least this slow.
    pub p95_ms: f64,
    /// 99th percentile: the stutter tail.
    pub p99_ms: f64,
}

impl FrameStats {
    /// Frames per second implied by the mean frame time.
    ///
    /// Reported separately from the percentiles because an average FPS number hides
    /// exactly the spikes the percentiles exist to expose.
    #[must_use]
    pub fn mean_fps(&self) -> f64 {
        if self.mean_ms > 0.0 {
            1000.0 / self.mean_ms
        } else {
            0.0
        }
    }

    /// Frames per second implied by the 99th-percentile frame time.
    #[must_use]
    pub fn p99_fps(&self) -> f64 {
        if self.p99_ms > 0.0 {
            1000.0 / self.p99_ms
        } else {
            0.0
        }
    }
}

/// Rolling window of frame times with percentile queries.
///
/// Holds the most recent `capacity` samples; older ones are overwritten.
#[derive(Debug)]
pub struct FrameStatsWindow {
    samples: Vec<f64>,
    scratch: Vec<f64>,
    next: usize,
    filled: bool,
}

impl FrameStatsWindow {
    /// Creates a window holding up to `capacity` samples.
    ///
    /// Both buffers are allocated here and never grow afterwards.
    ///
    /// # Panics
    ///
    /// Panics if `capacity` is zero, which would make every statistic undefined.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "frame stats window needs a non-zero capacity");
        Self {
            samples: vec![0.0; capacity],
            scratch: vec![0.0; capacity],
            next: 0,
            filled: false,
        }
    }

    /// Records one frame duration. Never allocates.
    pub fn record(&mut self, frame_time: Duration) {
        self.record_ms(duration_to_ms(frame_time));
    }

    /// Records one frame time in milliseconds. Never allocates.
    pub fn record_ms(&mut self, milliseconds: f64) {
        self.samples[self.next] = milliseconds;
        self.next += 1;
        if self.next == self.samples.len() {
            self.next = 0;
            self.filled = true;
        }
    }

    /// Number of samples currently held.
    #[must_use]
    pub fn len(&self) -> usize {
        if self.filled {
            self.samples.len()
        } else {
            self.next
        }
    }

    /// Whether no sample has been recorded yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Maximum number of samples held before the oldest are dropped.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.samples.len()
    }

    /// Drops every recorded sample.
    pub fn clear(&mut self) {
        self.next = 0;
        self.filled = false;
    }

    /// Computes the statistics over the current window.
    ///
    /// Sorts into the pre-allocated scratch buffer, so this does not allocate. Cost is
    /// `O(n log n)` in the window size; call it on a reporting interval, not per frame.
    ///
    /// Returns `None` if no samples have been recorded.
    #[must_use]
    pub fn stats(&mut self) -> Option<FrameStats> {
        let count = self.len();
        if count == 0 {
            return None;
        }

        self.scratch[..count].copy_from_slice(&self.samples[..count]);
        let sorted = &mut self.scratch[..count];
        sorted.sort_by(f64::total_cmp);

        let sum: f64 = sorted.iter().sum();
        Some(FrameStats {
            count,
            mean_ms: sum / count as f64,
            min_ms: sorted[0],
            max_ms: sorted[count - 1],
            p50_ms: percentile_of_sorted(sorted, 0.50),
            p95_ms: percentile_of_sorted(sorted, 0.95),
            p99_ms: percentile_of_sorted(sorted, 0.99),
        })
    }
}

/// Converts a duration to milliseconds without losing sub-microsecond resolution.
#[must_use]
pub fn duration_to_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// Nearest-rank percentile of an ascending-sorted slice.
///
/// Nearest rank is used rather than interpolation because a p99 frame time should be an
/// actual frame that happened, not a value between two frames.
///
/// # Panics
///
/// Panics if `sorted` is empty.
#[must_use]
pub fn percentile_of_sorted(sorted: &[f64], quantile: f64) -> f64 {
    assert!(!sorted.is_empty(), "percentile of an empty sample set");
    let quantile = quantile.clamp(0.0, 1.0);
    let rank = (quantile * sorted.len() as f64).ceil() as usize;
    let index = rank.saturating_sub(1).min(sorted.len() - 1);
    sorted[index]
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    fn window_of(values: &[f64]) -> FrameStatsWindow {
        let mut window = FrameStatsWindow::new(values.len().max(1));
        for &value in values {
            window.record_ms(value);
        }
        window
    }

    #[test]
    fn percentiles_use_nearest_rank() {
        let samples: Vec<f64> = (1..=100).map(f64::from).collect();
        assert_eq!(percentile_of_sorted(&samples, 0.50), 50.0);
        assert_eq!(percentile_of_sorted(&samples, 0.95), 95.0);
        assert_eq!(percentile_of_sorted(&samples, 0.99), 99.0);
        assert_eq!(percentile_of_sorted(&samples, 1.0), 100.0);
        assert_eq!(percentile_of_sorted(&samples, 0.0), 1.0);
    }

    #[test]
    fn percentile_of_a_single_sample_is_that_sample() {
        assert_eq!(percentile_of_sorted(&[4.2], 0.99), 4.2);
    }

    #[test]
    fn stats_over_a_known_set() {
        let mut window = window_of(&[1.0, 2.0, 3.0, 4.0]);
        let stats = window.stats().expect("four samples were recorded");
        assert_eq!(stats.count, 4);
        assert!((stats.mean_ms - 2.5).abs() < 1e-12);
        assert_eq!(stats.min_ms, 1.0);
        assert_eq!(stats.max_ms, 4.0);
        assert_eq!(stats.p50_ms, 2.0);
        assert_eq!(stats.p95_ms, 4.0);
    }

    #[test]
    fn stats_are_order_independent() {
        let mut ascending = window_of(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        let mut shuffled = window_of(&[4.0, 1.0, 5.0, 2.0, 3.0]);
        assert_eq!(ascending.stats(), shuffled.stats());
    }

    #[test]
    fn empty_window_has_no_stats() {
        let mut window = FrameStatsWindow::new(8);
        assert!(window.is_empty());
        assert!(window.stats().is_none());
    }

    #[test]
    fn window_keeps_only_the_most_recent_samples() {
        let mut window = FrameStatsWindow::new(3);
        for value in [10.0, 20.0, 30.0, 40.0] {
            window.record_ms(value);
        }
        let stats = window.stats().expect("window holds three samples");
        assert_eq!(stats.count, 3);
        assert_eq!(
            stats.min_ms, 20.0,
            "the oldest sample must have been dropped"
        );
        assert_eq!(stats.max_ms, 40.0);
        assert_eq!(window.capacity(), 3);
    }

    #[test]
    fn clear_empties_the_window() {
        let mut window = window_of(&[1.0, 2.0]);
        window.clear();
        assert!(window.stats().is_none());
    }

    #[test]
    fn fps_is_derived_from_frame_time() {
        let mut window = window_of(&[4.0; 4]);
        let stats = window.stats().expect("samples were recorded");
        assert!((stats.mean_fps() - 250.0).abs() < 1e-9);
        assert!((stats.p99_fps() - 250.0).abs() < 1e-9);
    }

    #[test]
    fn zero_frame_time_reports_zero_fps_instead_of_infinity() {
        let mut window = window_of(&[0.0]);
        let stats = window.stats().expect("one sample was recorded");
        assert_eq!(stats.mean_fps(), 0.0);
        assert_eq!(stats.p99_fps(), 0.0);
    }

    #[test]
    fn duration_conversion_keeps_sub_microsecond_resolution() {
        assert!((duration_to_ms(Duration::from_nanos(1_500)) - 0.0015).abs() < 1e-12);
        assert!((duration_to_ms(Duration::from_millis(4)) - 4.0).abs() < 1e-12);
    }

    #[test]
    fn a_fresh_time_is_at_zero_on_both_timelines() {
        let time = Time::new();
        assert_eq!(time.frame_index(), 0);
        assert_eq!(time.real_delta(), Duration::ZERO);
        assert_eq!(time.real_elapsed(), Duration::ZERO);
        assert_eq!(time.delta(), Duration::ZERO);
        assert_eq!(time.elapsed(), Duration::ZERO);
        assert_eq!(time.scale(), 1.0);
        assert!(!time.is_paused());
        assert_eq!(time.max_delta(), DEFAULT_MAX_DELTA);
    }

    #[test]
    fn timelines_agree_at_normal_speed() {
        let mut time = Time::new();
        for _ in 0..3 {
            time.advance_by(Duration::from_millis(4));
        }
        assert_eq!(time.frame_index(), 3);
        assert_eq!(time.real_elapsed(), Duration::from_millis(12));
        assert_eq!(time.elapsed(), Duration::from_millis(12));
        assert_eq!(time.delta(), time.real_delta());
    }

    #[test]
    fn a_stalled_frame_is_clamped_for_the_simulation_only() {
        let mut time = Time::new();
        let stall = Duration::from_secs(3);
        time.advance_by(stall);

        assert_eq!(time.real_delta(), stall, "real time records what happened");
        assert_eq!(time.real_elapsed(), stall);
        assert_eq!(time.delta(), DEFAULT_MAX_DELTA);
        assert_eq!(time.elapsed(), DEFAULT_MAX_DELTA);
    }

    #[test]
    fn the_clamp_is_configurable_and_rejects_zero() {
        let mut time = Time::new();
        time.set_max_delta(Duration::from_millis(10));
        time.advance_by(Duration::from_millis(500));
        assert_eq!(time.delta(), Duration::from_millis(10));

        time.set_max_delta(Duration::ZERO);
        assert_eq!(
            time.max_delta(),
            DEFAULT_MAX_DELTA,
            "a zero clamp would freeze the simulation"
        );
    }

    #[test]
    fn scale_slows_and_speeds_simulation_time_but_not_real_time() {
        let mut time = Time::new();
        time.set_scale(0.5);
        time.advance_by(Duration::from_millis(10));
        assert_eq!(time.real_delta(), Duration::from_millis(10));
        assert_eq!(time.delta(), Duration::from_millis(5));

        time.set_scale(2.0);
        time.advance_by(Duration::from_millis(10));
        assert_eq!(time.delta(), Duration::from_millis(20));
        assert_eq!(time.real_elapsed(), Duration::from_millis(20));
        assert_eq!(time.elapsed(), Duration::from_millis(25));
    }

    #[test]
    fn scale_rejects_negative_and_non_finite_values() {
        let mut time = Time::new();
        time.set_scale(-1.0);
        assert_eq!(time.scale(), 0.0, "time does not run backwards");
        time.set_scale(f64::NAN);
        assert_eq!(time.scale(), 1.0);
        time.set_scale(f64::INFINITY);
        assert_eq!(time.scale(), 1.0);
    }

    #[test]
    fn pausing_stops_simulation_time_and_leaves_real_time_running() {
        let mut time = Time::new();
        time.advance_by(Duration::from_millis(4));
        time.set_paused(true);
        time.advance_by(Duration::from_millis(4));

        assert_eq!(time.delta(), Duration::ZERO);
        assert_eq!(time.elapsed(), Duration::from_millis(4));
        assert_eq!(time.real_elapsed(), Duration::from_millis(8));
        assert_eq!(time.frame_index(), 2, "frames still pass while paused");

        time.set_paused(false);
        time.advance_by(Duration::from_millis(4));
        assert_eq!(time.elapsed(), Duration::from_millis(8));
    }

    #[test]
    fn elapsed_is_monotonic_across_pause_scale_and_clamp() {
        let mut time = Time::new();
        let mut previous_real = Duration::ZERO;
        let mut previous_sim = Duration::ZERO;

        for (index, delta) in [1_u64, 4, 3000, 8, 2, 900]
            .into_iter()
            .map(Duration::from_millis)
            .enumerate()
        {
            time.set_paused(index == 2);
            time.set_scale(if index == 4 { 0.25 } else { 1.0 });
            time.advance_by(delta);

            assert!(time.real_elapsed() >= previous_real);
            assert!(time.elapsed() >= previous_sim);
            previous_real = time.real_elapsed();
            previous_sim = time.elapsed();
        }
        assert_eq!(time.frame_index(), 6);
    }

    #[test]
    fn advancing_from_the_clock_produces_a_real_delta_and_counts_the_frame() {
        let mut time = Time::new();
        time.advance();
        assert_eq!(time.frame_index(), 1);
        assert!(time.real_elapsed() >= time.real_delta());
        assert_eq!(time.delta(), time.real_delta(), "no scaling by default");
    }

    #[test]
    fn reset_restarts_both_timelines_and_keeps_the_settings() {
        let mut time = Time::new();
        time.set_scale(0.5);
        time.set_paused(true);
        time.set_max_delta(Duration::from_millis(10));
        time.advance_by(Duration::from_millis(4));
        time.reset();

        assert_eq!(time.frame_index(), 0);
        assert_eq!(time.real_elapsed(), Duration::ZERO);
        assert_eq!(time.elapsed(), Duration::ZERO);
        assert_eq!(time.real_delta(), Duration::ZERO);
        assert_eq!(time.scale(), 0.5);
        assert!(time.is_paused());
        assert_eq!(time.max_delta(), Duration::from_millis(10));
    }

    #[test]
    fn delta_seconds_matches_the_simulation_delta() {
        let mut time = Time::new();
        time.advance_by(Duration::from_millis(16));
        assert!((time.delta_seconds() - 0.016).abs() < 1e-6);
    }

    #[test]
    fn a_frame_shorter_than_a_step_runs_no_ticks_but_keeps_the_time() {
        let mut timestep = FixedTimestep::new(60);
        assert_eq!(timestep.accumulate(Duration::from_micros(4_000)), 0);
        assert_eq!(timestep.accumulate(Duration::from_micros(4_000)), 0);
        assert_eq!(timestep.accumulate(Duration::from_micros(4_000)), 0);
        assert_eq!(timestep.accumulate(Duration::from_micros(4_000)), 0);
        // 20 ms is past one 16.67 ms step: the four short frames were not thrown away.
        assert_eq!(timestep.accumulate(Duration::from_micros(4_000)), 1);
        assert_eq!(timestep.ticks(), 1);
    }

    #[test]
    fn tick_rate_does_not_depend_on_how_the_frames_fall() {
        // The property the whole type exists for: one second of real time is 60 ticks,
        // whether it arrives as 300 fast frames or 30 slow ones.
        let mut fast = FixedTimestep::new(60);
        let mut ticks_fast = 0;
        for _ in 0..300 {
            ticks_fast += fast.accumulate(Duration::from_micros(3_333));
        }

        let mut slow = FixedTimestep::new(60);
        let mut ticks_slow = 0;
        for _ in 0..30 {
            ticks_slow += slow.accumulate(Duration::from_micros(33_333));
        }

        assert_eq!(ticks_fast, 59, "299.97 ms short of a full second");
        assert_eq!(ticks_slow, 59);
        assert_eq!(fast.dropped_ticks(), 0);
        assert_eq!(slow.dropped_ticks(), 0);
    }

    #[test]
    fn a_stall_is_bounded_rather_than_spiralling() {
        let mut timestep = FixedTimestep::new(60);
        // Two seconds of stall would be 120 ticks, each of which costs time.
        let ticks = timestep.accumulate(Duration::from_secs(2));
        assert_eq!(ticks, DEFAULT_MAX_CATCH_UP_TICKS);
        assert!(timestep.dropped_ticks() > 100, "the surplus is counted");
        assert_eq!(
            timestep.accumulate(Duration::ZERO),
            0,
            "the accumulator was emptied, not left behind the clock"
        );
    }

    #[test]
    fn alpha_is_the_leftover_as_a_fraction_of_a_step() {
        let mut timestep = FixedTimestep::with_step(Duration::from_millis(10));
        timestep.accumulate(Duration::from_millis(15));
        assert!(
            (timestep.alpha() - 0.5).abs() < 1e-5,
            "{}",
            timestep.alpha()
        );
        assert!((0.0..1.0).contains(&timestep.alpha()));
    }

    #[test]
    fn a_degenerate_rate_falls_back_instead_of_ticking_forever() {
        assert_eq!(
            FixedTimestep::new(0).hz().round(),
            f64::from(DEFAULT_TICK_HZ)
        );
        assert!(!FixedTimestep::with_step(Duration::ZERO).step().is_zero());

        let mut timestep = FixedTimestep::new(60);
        timestep.set_max_catch_up_ticks(0);
        assert_eq!(
            timestep.max_catch_up_ticks(),
            1,
            "a timestep must be able to tick"
        );
    }

    #[test]
    fn resetting_empties_the_accumulator_and_the_counters() {
        let mut timestep = FixedTimestep::new(60);
        timestep.accumulate(Duration::from_millis(100));
        timestep.reset();
        assert_eq!(timestep.ticks(), 0);
        assert_eq!(timestep.dropped_ticks(), 0);
        assert_eq!(timestep.alpha(), 0.0);
        assert!((timestep.step_seconds() - 1.0 / 60.0).abs() < 1e-6);
    }

    #[test]
    fn clock_counts_frames_and_returns_positive_deltas() {
        let mut clock = FrameClock::new();
        assert_eq!(clock.frame_index(), 0);
        let first = clock.tick();
        let second = clock.tick();
        assert_eq!(clock.frame_index(), 2);
        assert!(first >= Duration::ZERO);
        assert!(second >= Duration::ZERO);
        assert!(clock.elapsed() >= first);
    }
}
