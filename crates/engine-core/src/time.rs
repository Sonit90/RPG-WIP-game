//! Frame timing: a monotonic clock and a fixed-capacity statistics window.
//!
//! Everything here is pure CPU-side timing built on [`Instant`], which is backed by
//! `QueryPerformanceCounter` on Windows (sub-microsecond resolution). GPU time cannot be
//! derived from these numbers — it needs GPU timestamps, added in Milestone 5.
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
