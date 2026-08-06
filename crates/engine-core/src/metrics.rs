//! Frame metrics: CPU and GPU frame-time history, stage averages, and the summary the
//! debug overlay and the log reports read.
//!
//! The engine records one sample per frame. That path is a handful of stores into
//! pre-allocated buffers — no allocation, no sorting, no formatting.
//!
//! Percentiles need a sort, which is `O(n log n)` in the window size and far too expensive
//! to pay every frame at 5000 FPS. So [`FrameMetrics`] keeps a cached [`FrameSummary`] and
//! recomputes it only when the refresh interval has elapsed. Readers get the most recent
//! summary; they never trigger the sort themselves.

use std::time::{Duration, Instant};

use crate::application::StageTimings;
use crate::time::{FrameStats, FrameStatsWindow, duration_to_ms};

/// Frames kept for percentile reporting.
///
/// At 240 FPS this is roughly four seconds of history, and at 5000 FPS a fifth of a
/// second — enough for a stable p99 without letting old behaviour dominate.
pub const DEFAULT_HISTORY: usize = 1024;

/// How often the cached summary is recomputed.
///
/// Four times a second: fast enough that a reported number is never stale on a human
/// timescale, slow enough that the sort is irrelevant to the frame budget.
pub const DEFAULT_REFRESH_INTERVAL: Duration = Duration::from_millis(250);

/// Mean CPU time per stage over the frames since the last refresh, in milliseconds.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct StageAverages {
    /// Frames the averages were computed from.
    pub frames: u32,
    /// Mean event-processing time.
    pub events_ms: f64,
    /// Mean update time.
    pub update_ms: f64,
    /// Mean render-recording time.
    pub render_ms: f64,
    /// Mean present time.
    pub present_ms: f64,
}

impl StageAverages {
    /// Sum of the stage averages.
    ///
    /// Slightly below the mean frame time: engine bookkeeping is not attributed to a stage.
    #[must_use]
    pub fn total_ms(&self) -> f64 {
        self.events_ms + self.update_ms + self.render_ms + self.present_ms
    }
}

/// Running per-stage totals. Plain counters, so accumulating allocates nothing.
#[derive(Debug, Default, Clone, Copy)]
struct StageTotals {
    events_ms: f64,
    update_ms: f64,
    render_ms: f64,
    present_ms: f64,
    frames: u32,
}

impl StageTotals {
    fn add(&mut self, stages: StageTimings) {
        self.events_ms += duration_to_ms(stages.events);
        self.update_ms += duration_to_ms(stages.update);
        self.render_ms += duration_to_ms(stages.render);
        self.present_ms += duration_to_ms(stages.present);
        self.frames += 1;
    }

    fn averages(&self) -> StageAverages {
        if self.frames == 0 {
            return StageAverages::default();
        }
        let frames = f64::from(self.frames);
        StageAverages {
            frames: self.frames,
            events_ms: self.events_ms / frames,
            update_ms: self.update_ms / frames,
            render_ms: self.render_ms / frames,
            present_ms: self.present_ms / frames,
        }
    }
}

/// A snapshot of frame performance, recomputed on an interval.
///
/// Plain `Copy` data, so a reader takes a consistent snapshot without borrowing the
/// metrics or risking a half-updated view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameSummary {
    /// CPU frame-time statistics over the history window.
    pub cpu: FrameStats,
    /// GPU frame-time statistics, absent until GPU timestamps have been resolved.
    pub gpu: Option<FrameStats>,
    /// Mean CPU time per stage since the previous summary.
    pub stages: StageAverages,
    /// Frame index the summary was computed at.
    pub frame_index: u64,
}

/// CPU and GPU frame-time history with an interval-refreshed summary.
///
/// Allocation policy: both history windows are allocated in [`FrameMetrics::new`] and
/// never grow. Recording and reading allocate nothing.
#[derive(Debug)]
pub struct FrameMetrics {
    cpu: FrameStatsWindow,
    gpu: FrameStatsWindow,
    stages: StageTotals,
    summary: Option<FrameSummary>,
    refresh_interval: Duration,
    last_refresh: Instant,
    frame_index: u64,
}

impl Default for FrameMetrics {
    fn default() -> Self {
        Self::new(DEFAULT_HISTORY, DEFAULT_REFRESH_INTERVAL)
    }
}

impl FrameMetrics {
    /// Creates metrics holding `history` frames and refreshing every `refresh_interval`.
    ///
    /// # Panics
    ///
    /// Panics if `history` is zero, which would make every statistic undefined.
    #[must_use]
    pub fn new(history: usize, refresh_interval: Duration) -> Self {
        Self {
            cpu: FrameStatsWindow::new(history),
            gpu: FrameStatsWindow::new(history),
            stages: StageTotals::default(),
            summary: None,
            refresh_interval,
            last_refresh: Instant::now(),
            frame_index: 0,
        }
    }

    /// Records one frame. Never allocates and never sorts.
    ///
    /// `cpu_frame_time` must be the *real* frame delta: a paused or slowed simulation must
    /// not make the engine look faster. `gpu_frame_ms` is `None` while GPU timestamps are
    /// still in flight, which is normal for the first frames after startup or a resize.
    pub fn record(
        &mut self,
        frame_index: u64,
        cpu_frame_time: Duration,
        stages: StageTimings,
        gpu_frame_ms: Option<f64>,
    ) {
        self.frame_index = frame_index;
        self.cpu.record(cpu_frame_time);
        if let Some(gpu_ms) = gpu_frame_ms {
            self.gpu.record_ms(gpu_ms);
        }
        self.stages.add(stages);

        if self.last_refresh.elapsed() >= self.refresh_interval {
            self.refresh();
        }
    }

    /// Recomputes the summary now, regardless of the interval.
    ///
    /// This is the expensive call: it sorts both history windows. The engine calls it on
    /// the refresh interval; call it directly only when a fresh summary is needed at a
    /// known-safe moment, such as at shutdown.
    pub fn refresh(&mut self) {
        self.last_refresh = Instant::now();
        let Some(cpu) = self.cpu.stats() else {
            return;
        };
        self.summary = Some(FrameSummary {
            cpu,
            gpu: self.gpu.stats(),
            stages: self.stages.averages(),
            frame_index: self.frame_index,
        });
        self.stages = StageTotals::default();
    }

    /// The most recent summary, or `None` before the first refresh.
    ///
    /// Cheap: returns the cached value and never sorts.
    #[must_use]
    pub fn summary(&self) -> Option<FrameSummary> {
        self.summary
    }

    /// How often the summary is recomputed.
    #[must_use]
    pub fn refresh_interval(&self) -> Duration {
        self.refresh_interval
    }

    /// Sets the refresh interval. A zero interval refreshes every frame, which is only
    /// sensible in a test.
    pub fn set_refresh_interval(&mut self, interval: Duration) {
        self.refresh_interval = interval;
    }

    /// Number of CPU samples currently held.
    #[must_use]
    pub fn cpu_sample_count(&self) -> usize {
        self.cpu.len()
    }

    /// Number of GPU samples currently held.
    #[must_use]
    pub fn gpu_sample_count(&self) -> usize {
        self.gpu.len()
    }

    /// Discards all history and the cached summary.
    ///
    /// Used when the workload changes: samples from the old scene describe a different
    /// workload, and blending the two produces statistics that describe neither.
    pub fn clear(&mut self) {
        self.cpu.clear();
        self.gpu.clear();
        self.stages = StageTotals::default();
        self.summary = None;
        self.last_refresh = Instant::now();
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    fn stages_ms(events: u64, update: u64, render: u64, present: u64) -> StageTimings {
        StageTimings {
            events: Duration::from_millis(events),
            update: Duration::from_millis(update),
            render: Duration::from_millis(render),
            present: Duration::from_millis(present),
        }
    }

    /// Metrics that refresh on every recorded frame, so tests never sleep.
    fn eager_metrics() -> FrameMetrics {
        FrameMetrics::new(64, Duration::ZERO)
    }

    #[test]
    fn no_summary_before_the_first_frame() {
        let metrics = eager_metrics();
        assert!(metrics.summary().is_none());
        assert_eq!(metrics.cpu_sample_count(), 0);
    }

    #[test]
    fn a_recorded_frame_reaches_the_summary() {
        let mut metrics = eager_metrics();
        metrics.record(1, Duration::from_millis(4), stages_ms(1, 1, 1, 1), None);

        let summary = metrics.summary().expect("one frame was recorded");
        assert_eq!(summary.frame_index, 1);
        assert_eq!(summary.cpu.count, 1);
        assert!((summary.cpu.mean_ms - 4.0).abs() < 1e-9);
        assert!(summary.gpu.is_none(), "no GPU sample was supplied");
        assert_eq!(summary.stages.frames, 1);
        assert!((summary.stages.total_ms() - 4.0).abs() < 1e-9);
    }

    #[test]
    fn gpu_samples_are_tracked_separately_and_may_start_late() {
        let mut metrics = eager_metrics();
        metrics.record(1, Duration::from_millis(4), StageTimings::default(), None);
        metrics.record(2, Duration::from_millis(4), StageTimings::default(), None);
        metrics.record(
            3,
            Duration::from_millis(4),
            StageTimings::default(),
            Some(2.0),
        );

        let summary = metrics.summary().expect("frames were recorded");
        assert_eq!(summary.cpu.count, 3);
        let gpu = summary.gpu.expect("one GPU sample arrived");
        assert_eq!(gpu.count, 1, "missing GPU samples are not counted as zero");
        assert!((gpu.mean_ms - 2.0).abs() < 1e-9);
        assert_eq!(metrics.gpu_sample_count(), 1);
    }

    #[test]
    fn stage_averages_cover_only_the_frames_since_the_last_refresh() {
        let mut metrics = FrameMetrics::new(64, Duration::ZERO);
        metrics.record(1, Duration::from_millis(4), stages_ms(2, 4, 6, 8), None);
        let first = metrics.summary().expect("a frame was recorded").stages;
        assert!((first.update_ms - 4.0).abs() < 1e-9);

        metrics.record(2, Duration::from_millis(4), stages_ms(0, 0, 0, 0), None);
        let second = metrics.summary().expect("a frame was recorded").stages;
        assert_eq!(
            second.frames, 1,
            "the previous frame's totals were consumed"
        );
        assert!((second.update_ms - 0.0).abs() < 1e-9);
    }

    #[test]
    fn the_summary_is_not_recomputed_between_refreshes() {
        // A long interval means the only refresh is the explicit one.
        let mut metrics = FrameMetrics::new(64, Duration::from_secs(3600));
        for frame in 1..=10 {
            metrics.record(
                frame,
                Duration::from_millis(4),
                StageTimings::default(),
                None,
            );
        }
        assert!(
            metrics.summary().is_none(),
            "the interval has not elapsed, so no sort has run"
        );
        assert_eq!(metrics.cpu_sample_count(), 10, "history is still recorded");

        metrics.refresh();
        assert_eq!(
            metrics.summary().expect("refreshed explicitly").cpu.count,
            10
        );
    }

    #[test]
    fn percentiles_come_from_the_history_window() {
        let mut metrics = FrameMetrics::new(100, Duration::from_secs(3600));
        for frame in 1..=100_u64 {
            metrics.record(
                frame,
                Duration::from_millis(frame),
                StageTimings::default(),
                None,
            );
        }
        metrics.refresh();

        let cpu = metrics.summary().expect("refreshed explicitly").cpu;
        assert!((cpu.p50_ms - 50.0).abs() < 1e-9);
        assert!((cpu.p95_ms - 95.0).abs() < 1e-9);
        assert!((cpu.p99_ms - 99.0).abs() < 1e-9);
        assert!((cpu.max_ms - 100.0).abs() < 1e-9);
    }

    #[test]
    fn history_is_bounded_and_drops_the_oldest_frames() {
        let mut metrics = FrameMetrics::new(4, Duration::from_secs(3600));
        for frame in 1..=10_u64 {
            metrics.record(
                frame,
                Duration::from_millis(frame),
                StageTimings::default(),
                None,
            );
        }
        metrics.refresh();

        let cpu = metrics.summary().expect("refreshed explicitly").cpu;
        assert_eq!(cpu.count, 4);
        assert!((cpu.min_ms - 7.0).abs() < 1e-9, "frames 1-6 were dropped");
        assert!((cpu.max_ms - 10.0).abs() < 1e-9);
        assert_eq!(metrics.cpu_sample_count(), 4);
    }

    #[test]
    fn clearing_drops_history_and_the_cached_summary() {
        let mut metrics = eager_metrics();
        metrics.record(1, Duration::from_millis(4), stages_ms(1, 1, 1, 1), None);
        metrics.clear();

        assert!(metrics.summary().is_none());
        assert_eq!(metrics.cpu_sample_count(), 0);
        assert_eq!(metrics.gpu_sample_count(), 0);
    }

    #[test]
    fn stage_averages_of_no_frames_are_zero() {
        assert_eq!(StageTotals::default().averages(), StageAverages::default());
        assert!((StageAverages::default().total_ms() - 0.0).abs() < 1e-9);
    }
}
