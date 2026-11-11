//! The resolution matrix, measured in one process.
//!
//! # Why this exists
//!
//! Comparing two builds at four resolutions used to mean eight application launches: eight
//! scene loads, eight sets of pipeline creation and eight cold driver caches, with the
//! numbers from separate processes compared as if they were one experiment. That is slow
//! enough to discourage measuring, and it is the reason the milestone-3 question — "the
//! frame got 0.13 ms slower at 800x450 and no slower at 1080p" — had no answer.
//!
//! So the benchmark drives the resolutions itself. One launch, one scene, one set of
//! materials and meshes and descriptors, and the window is resized between stages. What a
//! resize recreates is exactly what depends on the size — the swap-chain buffers, the depth
//! buffer, the contact-shadow target — and nothing else: the meshes, the textures, their
//! descriptors and the pipelines are the same objects at 4K that they were at 800x450.
//!
//! # What is measured, and what is deliberately not
//!
//! Each stage is three phases:
//!
//! ```text
//! Resizing    the window has been asked to change; frames still render, nothing is recorded
//! WarmingUp   the renderer reports the new size; N frames pass and are discarded
//! Measuring   M frames are recorded
//! ```
//!
//! The resize itself, the swap-chain recreation, the depth-buffer allocation and the frames
//! where the GPU timestamps still describe the *old* resolution all land in the first two
//! phases and are thrown away. The warm-up has to be longer than the renderer's frames in
//! flight for the last reason: a GPU timestamp is read a few frames after the frame it
//! belongs to, so a measurement window that opened immediately would begin with two or three
//! samples from the previous resolution.
//!
//! CPU and GPU statistics come from the engine's own metrics window, cleared at the moment
//! measurement opens, so the percentiles are computed exactly the way the ordinary end-of-run
//! report computes them. The per-pass GPU split is this module's own, out of
//! [`engine_renderer::gpu_timer`]'s timestamps.

use std::fmt::Write as _;
use std::time::Instant;

use engine_core::time::{FrameStats, FrameStatsWindow};
use engine_renderer::gpu_timer::{MAX_TIMED_PASSES, TIMED_PASSES};

/// How many frames a stage discards after the renderer reports the new size.
///
/// Must exceed the renderer's frames in flight, which is three: a GPU timestamp is resolved
/// that many frames after the work it timed, so the first samples after a resize would
/// otherwise describe the previous resolution. Sixty is far above that and still under a
/// second at any frame rate this engine produces.
pub const DEFAULT_WARMUP_FRAMES: u64 = 60;

/// How many frames a stage measures once it is warm.
pub const DEFAULT_MEASURE_FRAMES: u64 = 300;

/// The resolutions a bare `--benchmark` runs.
pub const DEFAULT_RESOLUTIONS: [(u32, u32); 4] =
    [(800, 450), (1280, 720), (1920, 1080), (3840, 2160)];

/// What one stage of the matrix asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stage {
    /// The client size to render at.
    pub size: (u32, u32),
    /// Which repetition of the whole matrix this is, from zero.
    pub pass: u32,
}

/// What the benchmark wants the application to do after a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Ask the window for this client size. The application applies it through its ordinary
    /// resize path, which is the same one a player dragging the window edge takes.
    Resize(u32, u32),
    /// Keep rendering; the benchmark is warming up or measuring.
    Continue,
    /// Every stage is done. The application should report and exit.
    Finished,
}

/// One measured stage.
#[derive(Debug, Clone)]
pub struct StageResult {
    /// Which pass of the matrix this row belongs to.
    pub pass: u32,
    /// What was asked for.
    pub requested: (u32, u32),
    /// What the renderer's swap chain actually reported while measuring. A row where this
    /// differs from `requested` is a row that measured something other than what it says.
    pub actual: (u32, u32),
    /// CPU frame time over the measurement window.
    pub cpu: Option<FrameStats>,
    /// GPU frame time over the same window.
    pub gpu: Option<FrameStats>,
    /// GPU time per pass: shadow, scene, particles, interface.
    pub passes: [Option<FrameStats>; MAX_TIMED_PASSES],
    /// Frames measured, and how long they took in wall-clock seconds.
    pub frames: u64,
    /// Wall-clock seconds the measurement window spanned.
    pub seconds: f64,
    /// Draw calls in the last measured frame.
    pub draw_calls: u32,
    /// Textures the renderer holds, and how many distinct images they came from.
    pub textures: usize,
    /// Distinct uploaded images behind those textures — the deduplication ratio.
    pub unique_textures: usize,
    /// Persistent descriptors in use, and the region's capacity.
    pub descriptors: (u32, u32),
}

impl StageResult {
    /// Frames per second implied by the wall-clock span of the measurement window.
    #[must_use]
    pub fn fps(&self) -> f64 {
        if self.seconds > 0.0 {
            self.frames as f64 / self.seconds
        } else {
            0.0
        }
    }
}

/// Which phase of a stage the benchmark is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Waiting for the renderer to report the requested size.
    Resizing,
    /// The size is right; discarding frames while the driver settles.
    WarmingUp,
    /// Recording.
    Measuring,
    /// Every stage is done.
    Finished,
}

/// Drives the matrix and collects the rows.
pub struct ResolutionBenchmark {
    stages: Vec<Stage>,
    current: usize,
    phase: Phase,
    frames_in_phase: u64,
    warmup_frames: u64,
    measure_frames: u64,
    /// Per-pass GPU windows, this module's own — the engine's metrics window carries the
    /// frame's total and knows nothing about passes.
    passes: [FrameStatsWindow; MAX_TIMED_PASSES],
    measurement_started: Option<Instant>,
    /// How long the application has been waiting for a resize to take effect. A stage that
    /// waits forever is a renderer bug, and reporting it as one is better than a benchmark
    /// that silently hangs.
    resize_frames: u64,
    results: Vec<StageResult>,
}

/// Everything the benchmark reads about a frame, gathered by the application.
///
/// A struct rather than eight arguments, and borrowed from the application rather than the
/// renderer directly, so this module needs no opinion about how the application is built.
#[derive(Debug, Clone, Copy)]
pub struct FrameReport {
    /// What the renderer's swap chain says its size is, right now.
    pub actual_size: (u32, u32),
    /// This frame's per-pass GPU times, as the renderer last resolved them.
    pub pass_ms: [Option<f64>; MAX_TIMED_PASSES],
    /// Draw calls recorded for the frame.
    pub draw_calls: u32,
    /// Textures held, and distinct images behind them.
    pub textures: usize,
    /// Distinct uploaded images.
    pub unique_textures: usize,
    /// Persistent descriptors used and the capacity they came from.
    pub descriptors: (u32, u32),
}

/// What the application must do at the two boundaries the benchmark controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Boundary {
    /// Measurement is opening: clear the engine's metrics window so the percentiles cover
    /// this stage and nothing before it.
    OpenMeasurement,
    /// Measurement has closed: read the engine's metrics window back out.
    CloseMeasurement,
    /// Nothing to do.
    None,
}

impl ResolutionBenchmark {
    /// Builds a benchmark over `resolutions`, repeated `passes` times.
    ///
    /// # Panics
    ///
    /// Panics if `resolutions` is empty or `passes` is zero, both of which would leave a
    /// benchmark with nothing to measure.
    #[must_use]
    pub fn new(
        resolutions: &[(u32, u32)],
        passes: u32,
        warmup_frames: u64,
        measure_frames: u64,
    ) -> Self {
        assert!(
            !resolutions.is_empty(),
            "a benchmark needs at least one resolution"
        );
        assert!(passes > 0, "a benchmark needs at least one pass");
        let stages = (0..passes)
            .flat_map(|pass| {
                resolutions
                    .iter()
                    .map(move |size| Stage { size: *size, pass })
            })
            .collect();
        Self {
            stages,
            current: 0,
            phase: Phase::Resizing,
            frames_in_phase: 0,
            warmup_frames,
            measure_frames,
            passes: std::array::from_fn(|_| FrameStatsWindow::new(4096)),
            measurement_started: None,
            resize_frames: 0,
            results: Vec::new(),
        }
    }

    /// Whether every stage has been measured.
    ///
    /// Test-only: the application learns the matrix is over from [`Action::Finished`], which
    /// arrives on the frame it happens rather than a frame later.
    #[cfg(test)]
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.phase == Phase::Finished
    }

    /// Advances one frame and says what the application should do next.
    ///
    /// `boundary` is written rather than returned beside the action because the two are
    /// different questions with different answers: the action is about the window, the
    /// boundary is about the engine's metrics window, and a frame can need both.
    pub fn frame(&mut self, report: &FrameReport, boundary: &mut Boundary) -> Action {
        *boundary = Boundary::None;
        let Some(stage) = self.stages.get(self.current).copied() else {
            self.phase = Phase::Finished;
            return Action::Finished;
        };

        match self.phase {
            Phase::Resizing => {
                if report.actual_size == stage.size {
                    // The renderer has acknowledged the new size. Only now does the warm-up
                    // clock start: counting from the request would include the frames where
                    // the swap chain was still the old one.
                    self.phase = Phase::WarmingUp;
                    self.frames_in_phase = 0;
                    self.resize_frames = 0;
                    log::info!(
                        "benchmark: {}x{} is live, warming up for {} frame(s)",
                        stage.size.0,
                        stage.size.1,
                        self.warmup_frames
                    );
                    return Action::Continue;
                }
                self.resize_frames += 1;
                if self.resize_frames % 240 == 0 {
                    log::warn!(
                        "benchmark: still waiting for {}x{}; the renderer reports {}x{} after \
                         {} frame(s)",
                        stage.size.0,
                        stage.size.1,
                        report.actual_size.0,
                        report.actual_size.1,
                        self.resize_frames
                    );
                }
                // Re-requested every frame until it takes: a request that raced a minimize,
                // or one Windows coalesced away, would otherwise stall the whole matrix.
                Action::Resize(stage.size.0, stage.size.1)
            }
            Phase::WarmingUp => {
                self.frames_in_phase += 1;
                if self.frames_in_phase >= self.warmup_frames {
                    self.phase = Phase::Measuring;
                    self.frames_in_phase = 0;
                    for window in &mut self.passes {
                        window.clear();
                    }
                    self.measurement_started = Some(Instant::now());
                    *boundary = Boundary::OpenMeasurement;
                    log::info!(
                        "benchmark: measuring {}x{} for {} frame(s)",
                        stage.size.0,
                        stage.size.1,
                        self.measure_frames
                    );
                }
                Action::Continue
            }
            Phase::Measuring => {
                for (pass, sample) in report.pass_ms.iter().enumerate() {
                    if let Some(milliseconds) = sample {
                        self.passes[pass].record_ms(*milliseconds);
                    }
                }
                self.frames_in_phase += 1;
                if self.frames_in_phase < self.measure_frames {
                    return Action::Continue;
                }

                let seconds = self
                    .measurement_started
                    .map(|start| start.elapsed().as_secs_f64())
                    .unwrap_or_default();
                self.results.push(StageResult {
                    pass: stage.pass,
                    requested: stage.size,
                    actual: report.actual_size,
                    cpu: None,
                    gpu: None,
                    passes: std::array::from_fn(|pass| self.passes[pass].stats()),
                    frames: self.frames_in_phase,
                    seconds,
                    draw_calls: report.draw_calls,
                    textures: report.textures,
                    unique_textures: report.unique_textures,
                    descriptors: report.descriptors,
                });
                *boundary = Boundary::CloseMeasurement;

                self.current += 1;
                self.frames_in_phase = 0;
                if self.current >= self.stages.len() {
                    self.phase = Phase::Finished;
                    return Action::Finished;
                }
                self.phase = Phase::Resizing;
                let next = self.stages[self.current];
                Action::Resize(next.size.0, next.size.1)
            }
            Phase::Finished => Action::Finished,
        }
    }

    /// Fills in the CPU and GPU statistics of the row just closed.
    ///
    /// Called by the application when it sees [`Boundary::CloseMeasurement`], with the
    /// engine's own metrics summary — the same numbers the ordinary end-of-run report uses,
    /// computed the same way.
    pub fn record_engine_stats(&mut self, cpu: Option<FrameStats>, gpu: Option<FrameStats>) {
        if let Some(result) = self.results.last_mut() {
            result.cpu = cpu;
            result.gpu = gpu;
        }
    }

    /// Every measured row, in the order they were measured.
    ///
    /// Test-only for now: the application prints [`ResolutionBenchmark::report`] and does not
    /// otherwise inspect the rows.
    #[cfg(test)]
    #[must_use]
    pub fn results(&self) -> &[StageResult] {
        &self.results
    }

    /// The whole matrix as a report, one block per row.
    #[must_use]
    pub fn report(&self) -> String {
        let mut out = String::new();
        for result in &self.results {
            let _ = writeln!(
                out,
                "  {}x{} (pass {})",
                result.requested.0,
                result.requested.1,
                result.pass + 1
            );
            let _ = writeln!(
                out,
                "    render target : {}x{}{}",
                result.actual.0,
                result.actual.1,
                if result.actual == result.requested {
                    ""
                } else {
                    "   MISMATCH - this row measured something else"
                }
            );
            match result.gpu {
                Some(gpu) => {
                    let _ = writeln!(
                        out,
                        "    GPU frame     : p50 {:.3} | p95 {:.3} | p99 {:.3} | mean {:.3} ms | n={}",
                        gpu.p50_ms, gpu.p95_ms, gpu.p99_ms, gpu.mean_ms, gpu.count
                    );
                }
                None => {
                    let _ = writeln!(out, "    GPU frame     : not available");
                }
            }
            if let Some(cpu) = result.cpu {
                let _ = writeln!(
                    out,
                    "    CPU frame     : p50 {:.3} | p95 {:.3} | p99 {:.3} | mean {:.3} ms | n={} | 1% low {:.0} FPS",
                    cpu.p50_ms,
                    cpu.p95_ms,
                    cpu.p99_ms,
                    cpu.mean_ms,
                    cpu.count,
                    cpu.p99_fps()
                );
            }
            for (pass, name) in TIMED_PASSES {
                if let Some(stats) = result.passes[pass] {
                    let _ = writeln!(
                        out,
                        "    GPU {name:<10}: p50 {:.3} | p95 {:.3} ms | n={}",
                        stats.p50_ms, stats.p95_ms, stats.count
                    );
                }
            }
            let _ = writeln!(
                out,
                "    FPS           : {:.0} over {} frame(s) in {:.3} s",
                result.fps(),
                result.frames,
                result.seconds
            );
            let _ = writeln!(
                out,
                "    draws         : {} | textures {} ({} unique) | descriptors {}/{}",
                result.draw_calls,
                result.textures,
                result.unique_textures,
                result.descriptors.0,
                result.descriptors.1
            );
        }
        out
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    fn report(size: (u32, u32)) -> FrameReport {
        FrameReport {
            actual_size: size,
            pass_ms: [
                Some(0.5),
                Some(1.0),
                Some(0.1),
                Some(0.05),
                Some(0.3),
                Some(0.1),
                Some(0.1),
            ],
            draw_calls: 42,
            textures: 19,
            unique_textures: 12,
            descriptors: (111, 4096),
        }
    }

    /// Runs a benchmark to completion against a renderer that resizes instantly.
    fn drive(benchmark: &mut ResolutionBenchmark, mut size: (u32, u32)) -> usize {
        let mut boundary = Boundary::None;
        let mut frames = 0;
        while !benchmark.is_finished() {
            frames += 1;
            assert!(frames < 1_000_000, "the benchmark must terminate");
            match benchmark.frame(&report(size), &mut boundary) {
                Action::Resize(width, height) => size = (width, height),
                Action::Continue => {}
                Action::Finished => break,
            }
            if boundary == Boundary::CloseMeasurement {
                benchmark.record_engine_stats(None, None);
            }
        }
        frames
    }

    #[test]
    fn every_resolution_is_measured_once_per_pass() {
        let mut benchmark = ResolutionBenchmark::new(&[(800, 450), (1920, 1080)], 2, 4, 8);
        drive(&mut benchmark, (800, 450));

        let rows = benchmark.results();
        assert_eq!(rows.len(), 4, "two resolutions, two passes");
        assert_eq!(rows[0].requested, (800, 450));
        assert_eq!(rows[1].requested, (1920, 1080));
        assert_eq!(rows[2].pass, 1, "the second pass repeats the matrix");
        for row in rows {
            assert_eq!(row.frames, 8, "each row measures the requested frame count");
            assert_eq!(
                row.actual, row.requested,
                "and measures the size it asked for"
            );
        }
    }

    #[test]
    fn nothing_is_recorded_until_the_renderer_reports_the_new_size() {
        // The property the whole design rests on: a stage that measured while the swap chain
        // was still the old size would report the old resolution's cost under the new
        // resolution's name, and nothing downstream could tell.
        let mut benchmark = ResolutionBenchmark::new(&[(1920, 1080)], 1, 2, 3);
        let mut boundary = Boundary::None;

        // Ten frames at the wrong size: every one asks for the resize again and records
        // nothing.
        for _ in 0..10 {
            let action = benchmark.frame(&report((800, 450)), &mut boundary);
            assert_eq!(action, Action::Resize(1920, 1080));
            assert_eq!(boundary, Boundary::None);
        }
        assert!(benchmark.results().is_empty());

        // The size arrives; the warm-up runs; only then does measurement open.
        assert_eq!(
            benchmark.frame(&report((1920, 1080)), &mut boundary),
            Action::Continue
        );
        assert_eq!(
            boundary,
            Boundary::None,
            "the acknowledging frame is not warm-up"
        );
        benchmark.frame(&report((1920, 1080)), &mut boundary);
        assert_eq!(boundary, Boundary::None);
        benchmark.frame(&report((1920, 1080)), &mut boundary);
        assert_eq!(
            boundary,
            Boundary::OpenMeasurement,
            "measurement opens after the warm-up frames, not before"
        );
    }

    #[test]
    fn the_per_pass_split_is_collected_and_reported() {
        let mut benchmark = ResolutionBenchmark::new(&[(800, 450)], 1, 1, 4);
        drive(&mut benchmark, (800, 450));

        let row = &benchmark.results()[0];
        let scene = row.passes[engine_renderer::gpu_timer::SCENE_PASS]
            .expect("the scene pass was sampled every frame");
        assert!((scene.p50_ms - 1.0).abs() < 1e-9);
        let shadow = row.passes[engine_renderer::gpu_timer::SHADOW_PASS]
            .expect("the shadow pass was sampled every frame");
        assert!((shadow.p50_ms - 0.5).abs() < 1e-9);

        let text = benchmark.report();
        assert!(text.contains("GPU scene"), "{text}");
        assert!(text.contains("800x450"), "{text}");
    }
}
