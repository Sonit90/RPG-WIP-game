//! The whole measured run as one distribution, and the frames that hitched in it.
//!
//! # Why the engine's own metrics are not enough
//!
//! `FrameMetrics` keeps the last 1024 frames, which is right for a live readout and wrong for
//! the closing numbers of a benchmark: a 20,000-frame run's "final p99" is the p99 of its
//! last four seconds, and a 0.1% low over 1024 samples is the single slowest frame. So a run
//! with `--frames` also records every measured frame here, in a window sized to the run and
//! allocated once at startup, and the closing report reads it.
//!
//! # The hitch log
//!
//! A percentile says *that* the tail exists; it does not say what was in it. Every frame
//! slower than [`HitchLog::threshold_ms`] is captured with its stage split, its update
//! sub-stages and what the renderer created during it — pipeline state objects, resources,
//! flushes that waited on the GPU. The worst [`WORST_KEPT`] are kept and printed at the end.
//!
//! The resource columns are only non-zero in a build with `startup-timing`, which is the
//! only build that counts them; the frame times are always real.
//!
//! Nothing here allocates per frame: both windows and the hitch table are sized in `new`.

use engine_core::application::StageTimings;
use engine_core::time::{FrameStats, FrameStatsWindow};
use engine_renderer::ResourceCounters;
use engine_renderer::gpu_timer::{MAX_TIMED_PASSES, TIMED_PASSES};

/// Largest run recorded in full. Beyond it the window keeps the most recent frames, exactly
/// as the engine's own window does. 200,000 frames is 3.2 MB across both windows.
const MAX_RUN_FRAMES: usize = 200_000;

/// How many of the worst frames the hitch log keeps.
pub const WORST_KEPT: usize = 24;

/// A frame is a hitch if it is this many times the running median...
const HITCH_MULTIPLE: f64 = 4.0;

/// ...and at least this long. Below it a "4x spike" on a 0.3 ms frame is 1.2 ms, which no
/// player can see; 2 ms is half a refresh at 240 Hz.
const HITCH_FLOOR_MS: f64 = 2.0;

/// Where a frame's update stage went, from the application's own timers.
#[derive(Debug, Clone, Copy, Default)]
pub struct UpdateSplit {
    /// Simulation ticks run this frame.
    pub sim_ms: f64,
    /// Extraction, excluding effects (includes character animation).
    pub extract_ms: f64,
    /// The character-animation share of extraction.
    pub animate_ms: f64,
    /// Effects.
    pub vfx_ms: f64,
    /// Camera follow, collision sweeps and fade.
    pub camera_ms: f64,
    /// The audio director.
    pub audio_ms: f64,
    /// Building the interface.
    pub ui_ms: f64,
}

/// Where a frame's render stage went: the renderer's own pass timers, and how long the CPU
/// blocked on the GPU before it could start recording.
///
/// The wait matters because it is *inside* the render stage: a GPU-bound frame's render stage
/// is mostly the fence wait at the top of `begin_recording`, and "recording" computed as the
/// render stage minus the passes would count that wait as CPU work.
#[derive(Debug, Clone, Copy, Default)]
pub struct RenderSplit {
    /// Blocked on this slot's fence before recording could begin.
    pub wait_ms: f64,
    /// Frustum culling.
    pub cull_ms: f64,
    /// Grouping the visible list into batches.
    pub group_ms: f64,
    /// The shadow stage's CPU preparation.
    pub shadow_prep_ms: f64,
}

/// What the shadow stage recorded in one frame, from the shadow renderer's own counters.
///
/// Counts are carried as `f64` because the profile only ever sums and averages them.
#[derive(Debug, Clone, Copy, Default)]
pub struct ShadowSplit {
    /// Recording the atlas views: cascades and local lights.
    pub atlas_ms: f64,
    /// Recording the camera's depth prepass.
    pub prepass_ms: f64,
    /// Draw calls into the cascades.
    pub cascade_draws: f64,
    /// Draw calls into the local views.
    pub local_draws: f64,
    /// Skinned draws into the atlas views (included in the two above).
    pub skinned_draws: f64,
    /// Casters gathered across every view; a caster in two views counts twice.
    pub casters: f64,
    /// Views holding cached static casters.
    pub static_views: f64,
    /// Views whose static casters were gathered again this frame.
    pub static_rebuilt: f64,
    /// Static instances drawn from the cache.
    pub cached_instances: f64,
    /// Moving instances drawn.
    pub moving_instances: f64,
}

impl ShadowSplit {
    /// The split of one frame's shadow statistics.
    #[must_use]
    pub fn of(stats: &engine_renderer::shadow::ShadowStats) -> Self {
        let cascade_draws: u32 = stats.cascades.iter().map(|pass| pass.draw_calls).sum();
        Self {
            atlas_ms: stats.atlas_ms,
            prepass_ms: stats.prepass_ms,
            cascade_draws: f64::from(cascade_draws),
            local_draws: f64::from(stats.local.draw_calls),
            skinned_draws: f64::from(stats.skinned_draws),
            casters: f64::from(stats.casters()),
            static_views: f64::from(stats.static_views),
            static_rebuilt: f64::from(stats.static_cache.rebuilt),
            cached_instances: f64::from(stats.static_cache.cached_instances),
            moving_instances: f64::from(stats.static_cache.moving_instances),
        }
    }

    fn add(&mut self, other: &Self) {
        self.atlas_ms += other.atlas_ms;
        self.prepass_ms += other.prepass_ms;
        self.cascade_draws += other.cascade_draws;
        self.local_draws += other.local_draws;
        self.skinned_draws += other.skinned_draws;
        self.casters += other.casters;
        self.static_views += other.static_views;
        self.static_rebuilt += other.static_rebuilt;
        self.cached_instances += other.cached_instances;
        self.moving_instances += other.moving_instances;
    }
}

/// Everything recorded about one frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameSample {
    /// Engine frame index.
    pub frame: u64,
    /// The four engine stages.
    pub stages: StageTimings,
    /// The whole frame, as the engine measured it.
    pub total_ms: f64,
    /// The renderer's latest GPU frame time, which lags the frame by a few frames.
    pub gpu_ms: Option<f64>,
    /// The engine's current median frame time, which sets the hitch threshold.
    pub running_p50_ms: f64,
    /// The update stage, broken down.
    pub update: UpdateSplit,
    /// The render stage, broken down.
    pub render: RenderSplit,
    /// The shadow stage's recording, broken down.
    pub shadow: ShadowSplit,
}

/// One captured slow frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct Hitch {
    /// Engine frame index.
    pub frame: u64,
    /// The whole frame, as the engine measured it.
    pub total_ms: f64,
    /// The four engine stages.
    pub stages: [f64; 4],
    /// The update stage, broken down.
    pub update: UpdateSplit,
    /// What the renderer created or waited on during the frame.
    pub created: ResourceCounters,
    /// What the frame's threshold was, for reading the entry.
    pub threshold_ms: f64,
}

/// Slow frames, the worst [`WORST_KEPT`] of them.
#[derive(Debug)]
pub struct HitchLog {
    worst: Vec<Hitch>,
    count: u64,
    over_refresh: u64,
    over_8ms: u64,
    over_16ms: u64,
    with_pipelines: u64,
    with_flushes: u64,
    last_counters: ResourceCounters,
}

impl HitchLog {
    fn new() -> Self {
        Self {
            worst: Vec::with_capacity(WORST_KEPT),
            count: 0,
            over_refresh: 0,
            over_8ms: 0,
            over_16ms: 0,
            with_pipelines: 0,
            with_flushes: 0,
            last_counters: engine_renderer::resource_counters(),
        }
    }

    /// The hitch threshold for a frame whose running median is `p50_ms`.
    #[must_use]
    pub fn threshold_ms(p50_ms: f64) -> f64 {
        (p50_ms * HITCH_MULTIPLE).max(HITCH_FLOOR_MS)
    }

    fn observe(&mut self, mut hitch: Hitch) {
        let now = engine_renderer::resource_counters();
        hitch.created = now.since(self.last_counters);
        self.last_counters = now;

        if hitch.total_ms > 1000.0 / 240.0 {
            self.over_refresh += 1;
        }
        if hitch.total_ms > 8.333 {
            self.over_8ms += 1;
        }
        if hitch.total_ms > 16.667 {
            self.over_16ms += 1;
        }
        if hitch.total_ms <= hitch.threshold_ms {
            return;
        }
        self.count += 1;
        if hitch.created.pipelines > 0 {
            self.with_pipelines += 1;
        }
        if hitch.created.flushes > 0 {
            self.with_flushes += 1;
        }
        if self.worst.len() < WORST_KEPT {
            self.worst.push(hitch);
            return;
        }
        // Replace the mildest kept entry if this one is worse. A linear scan over 24 entries,
        // only on a frame that already hitched.
        if let Some((index, mildest)) = self
            .worst
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.total_ms.total_cmp(&b.1.total_ms))
            && hitch.total_ms > mildest.total_ms
        {
            self.worst[index] = hitch;
        }
    }

    fn clear(&mut self) {
        self.worst.clear();
        self.count = 0;
        self.over_refresh = 0;
        self.over_8ms = 0;
        self.over_16ms = 0;
        self.with_pipelines = 0;
        self.with_flushes = 0;
        self.last_counters = engine_renderer::resource_counters();
    }
}

/// Every measured frame of a `--frames` run.
#[derive(Debug)]
pub struct RunProfile {
    cpu: FrameStatsWindow,
    gpu: FrameStatsWindow,
    /// GPU time per timed pass, indexed as `gpu_timer::TIMED_PASSES` names them.
    passes: [FrameStatsWindow; MAX_TIMED_PASSES],
    /// Mesh-pass pixel-shader invocations per screen pixel, from `--pipeline-stats`. A ratio,
    /// not a time; the window is only used for its percentiles.
    overdraw: FrameStatsWindow,
    /// The same, per section of the pass (`pipeline_stats::SECTION_NAMES`), summed for means.
    overdraw_sections: [f64; engine_renderer::pipeline_stats::SECTIONS],
    overdraw_frames: u64,
    hitches: HitchLog,
    /// The allocation counter when measurement began. Zero without `alloc-count`.
    allocs_at_start: crate::alloc_count::Allocations,
    /// Sums of every measured frame's stages and update split, for run-wide means. The
    /// periodic report's split is one frame's sample; this is the whole run's.
    stage_sums: [f64; 4],
    split_sums: UpdateSplit,
    render_sums: RenderSplit,
    shadow_sums: ShadowSplit,
    frames: u64,
    /// The renderer's counters when measurement began, so the report can say what the run
    /// itself created: a pipeline created mid-run is a stall whether or not it crossed the
    /// hitch threshold. Zero without `startup-timing`.
    counters_at_start: ResourceCounters,
}

impl RunProfile {
    /// A profile for a run of `frames` frames, or `None` for an open-ended run.
    #[must_use]
    pub fn for_run(frames: Option<u64>) -> Option<Self> {
        let frames = frames?;
        let capacity = usize::try_from(frames)
            .unwrap_or(MAX_RUN_FRAMES)
            .clamp(1, MAX_RUN_FRAMES);
        Some(Self {
            cpu: FrameStatsWindow::new(capacity),
            gpu: FrameStatsWindow::new(capacity),
            passes: std::array::from_fn(|_| FrameStatsWindow::new(capacity)),
            overdraw: FrameStatsWindow::new(capacity),
            overdraw_sections: [0.0; engine_renderer::pipeline_stats::SECTIONS],
            overdraw_frames: 0,
            hitches: HitchLog::new(),
            allocs_at_start: crate::alloc_count::allocations(),
            stage_sums: [0.0; 4],
            split_sums: UpdateSplit::default(),
            render_sums: RenderSplit::default(),
            shadow_sums: ShadowSplit::default(),
            frames: 0,
            counters_at_start: engine_renderer::resource_counters(),
        })
    }

    /// Records the renderer's latest per-pass GPU times. They lag the frame by the same few
    /// frames the frame's own GPU time does, which a steady-state distribution does not mind.
    pub fn record_passes(&mut self, passes: [Option<f64>; MAX_TIMED_PASSES]) {
        for (window, pass) in self.passes.iter_mut().zip(passes) {
            if let Some(milliseconds) = pass {
                window.record_ms(milliseconds);
            }
        }
    }

    /// Records the latest mesh pass's pixel-shader invocations per screen pixel, whole and
    /// per section.
    pub fn record_overdraw(&mut self, sections: [f64; engine_renderer::pipeline_stats::SECTIONS]) {
        self.overdraw.record_ms(sections.iter().sum());
        for (sum, value) in self.overdraw_sections.iter_mut().zip(sections) {
            *sum += value;
        }
        self.overdraw_frames += 1;
    }

    /// Records one frame.
    pub fn record(&mut self, sample: FrameSample) {
        let FrameSample {
            frame,
            stages,
            total_ms,
            gpu_ms,
            running_p50_ms,
            update,
            render,
            shadow,
        } = sample;
        self.cpu.record_ms(total_ms);
        if let Some(gpu_ms) = gpu_ms {
            self.gpu.record_ms(gpu_ms);
        }
        let stage_ms = [
            stages.events.as_secs_f64() * 1e3,
            stages.update.as_secs_f64() * 1e3,
            stages.render.as_secs_f64() * 1e3,
            stages.present.as_secs_f64() * 1e3,
        ];
        for (sum, value) in self.stage_sums.iter_mut().zip(stage_ms) {
            *sum += value;
        }
        let sums = &mut self.split_sums;
        sums.sim_ms += update.sim_ms;
        sums.extract_ms += update.extract_ms;
        sums.animate_ms += update.animate_ms;
        sums.vfx_ms += update.vfx_ms;
        sums.camera_ms += update.camera_ms;
        sums.audio_ms += update.audio_ms;
        sums.ui_ms += update.ui_ms;
        let render_sums = &mut self.render_sums;
        render_sums.wait_ms += render.wait_ms;
        render_sums.cull_ms += render.cull_ms;
        render_sums.group_ms += render.group_ms;
        render_sums.shadow_prep_ms += render.shadow_prep_ms;
        self.shadow_sums.add(&shadow);
        self.frames += 1;
        self.hitches.observe(Hitch {
            frame,
            total_ms,
            stages: stage_ms,
            update,
            created: ResourceCounters::default(),
            threshold_ms: HitchLog::threshold_ms(running_p50_ms),
        });
    }

    /// Drops everything recorded so far: the warm-up boundary.
    pub fn clear(&mut self) {
        self.cpu.clear();
        self.gpu.clear();
        for window in &mut self.passes {
            window.clear();
        }
        self.overdraw.clear();
        self.overdraw_sections = [0.0; engine_renderer::pipeline_stats::SECTIONS];
        self.overdraw_frames = 0;
        self.stage_sums = [0.0; 4];
        self.split_sums = UpdateSplit::default();
        self.render_sums = RenderSplit::default();
        self.shadow_sums = ShadowSplit::default();
        self.frames = 0;
        self.counters_at_start = engine_renderer::resource_counters();
        self.hitches.clear();
        self.allocs_at_start = crate::alloc_count::allocations();
    }

    /// The run's CPU and GPU distributions.
    pub fn stats(&mut self) -> (Option<FrameStats>, Option<FrameStats>) {
        (self.cpu.stats(), self.gpu.stats())
    }

    /// Logs the closing numbers and the hitch table.
    pub fn report(&mut self) {
        let (cpu, gpu) = self.stats();
        if let Some(cpu) = cpu {
            log::info!(
                "run CPU: mean {:.3} | p50 {:.3} | p99 {:.3} | p99.9 {:.3} | max {:.3} ms | \
                 avg {:.0} FPS | 1% low {:.0} FPS | 0.1% low {:.0} FPS | n={}",
                cpu.mean_ms,
                cpu.p50_ms,
                cpu.p99_ms,
                cpu.p999_ms,
                cpu.max_ms,
                cpu.mean_fps(),
                cpu.p99_fps(),
                cpu.p999_fps(),
                cpu.count,
            );
        }
        if let Some(gpu) = gpu {
            log::info!(
                "run GPU: mean {:.3} | p50 {:.3} | p99 {:.3} | p99.9 {:.3} | max {:.3} ms | n={}",
                gpu.mean_ms,
                gpu.p50_ms,
                gpu.p99_ms,
                gpu.p999_ms,
                gpu.max_ms,
                gpu.count,
            );
        }
        if self.frames > 0 {
            let n = self.frames as f64;
            let s = &self.split_sums;
            log::info!(
                "run stages (mean ms): events {:.4} | update {:.4} (sim {:.4} | extract {:.4}, anim {:.4} | vfx {:.4} | camera {:.4} | audio {:.4} | ui {:.4}) | render {:.4} | present {:.4}",
                self.stage_sums[0] / n,
                self.stage_sums[1] / n,
                s.sim_ms / n,
                s.extract_ms / n,
                s.animate_ms / n,
                s.vfx_ms / n,
                s.camera_ms / n,
                s.audio_ms / n,
                s.ui_ms / n,
                self.stage_sums[2] / n,
                self.stage_sums[3] / n,
            );
            let r = &self.render_sums;
            let render = self.stage_sums[2] / n;
            let passes = (r.wait_ms + r.cull_ms + r.group_ms + r.shadow_prep_ms) / n;
            log::info!(
                "run render (mean ms): {:.4} = GPU wait {:.4} + cull {:.4} + group {:.4} + shadow prep {:.4} + recording {:.4}",
                render,
                r.wait_ms / n,
                r.cull_ms / n,
                r.group_ms / n,
                r.shadow_prep_ms / n,
                (render - passes).max(0.0),
            );
            let sh = &self.shadow_sums;
            log::info!(
                "run shadow (mean): record atlas {:.4} ms + prepass {:.4} ms | draws: cascades {:.1}, local {:.1} (skinned {:.1}) | casters {:.1} | static cache: {:.0} views cached, rebuilds {:.4}/frame, instances {:.1} cached + {:.1} moving",
                sh.atlas_ms / n,
                sh.prepass_ms / n,
                sh.cascade_draws / n,
                sh.local_draws / n,
                sh.skinned_draws / n,
                sh.casters / n,
                sh.static_views / n,
                sh.static_rebuilt / n,
                sh.cached_instances / n,
                sh.moving_instances / n,
            );
        }
        // Per pass on one line, p50 and mean: the shape of the GPU frame. The last three are
        // parts of the shadow stage, not additions to it.
        let mut line = String::from("run GPU passes (p50/mean ms):");
        for (pass, name) in TIMED_PASSES {
            if let Some(stats) = self.passes[pass].stats() {
                line.push_str(&format!(
                    " {name} {:.3}/{:.3} |",
                    stats.p50_ms, stats.mean_ms
                ));
            }
        }
        log::info!("{}", line.trim_end_matches(" |"));
        if let Some(overdraw) = self.overdraw.stats() {
            log::info!(
                "run overdraw (mesh pass): p50 {:.3} | mean {:.3} pixel-shader invocations per pixel | n={}",
                overdraw.p50_ms,
                overdraw.mean_ms,
                overdraw.count,
            );
            let frames = self.overdraw_frames.max(1) as f64;
            let mut line = String::from("run overdraw by section (mean):");
            for (name, sum) in engine_renderer::pipeline_stats::SECTION_NAMES
                .iter()
                .zip(self.overdraw_sections)
            {
                line.push_str(&format!(" {name} {:.3} |", sum / frames));
            }
            log::info!("{}", line.trim_end_matches(" |"));
        }
        if crate::alloc_count::COUNTING
            && let Some(cpu) = cpu
        {
            let allocations = crate::alloc_count::allocations().since(self.allocs_at_start);
            let frames = cpu.count.max(1) as f64;
            log::info!(
                "run allocations: {} ({:.3}/frame), {:.1} KiB ({:.0} B/frame) over {} frames",
                allocations.count,
                allocations.count as f64 / frames,
                allocations.bytes as f64 / 1024.0,
                allocations.bytes as f64 / frames,
                cpu.count,
            );
        }
        let created = engine_renderer::resource_counters().since(self.counters_at_start);
        log::info!(
            "run created: {} pipeline(s) in {:.2} ms, {} resource(s), {} GPU-waiting flush(es) in {:.2} ms",
            created.pipelines,
            created.pipeline_ns as f64 / 1e6,
            created.resources,
            created.flushes,
            created.flush_ns as f64 / 1e6,
        );
        let log = &mut self.hitches;
        log::info!(
            "run hitches: {} over threshold ({} created a pipeline, {} waited on an upload) | \
             frames over 4.17 ms {} | over 8.33 ms {} | over 16.7 ms {}",
            log.count,
            log.with_pipelines,
            log.with_flushes,
            log.over_refresh,
            log.over_8ms,
            log.over_16ms,
        );
        log.worst.sort_by(|a, b| b.total_ms.total_cmp(&a.total_ms));
        for hitch in &log.worst {
            log::info!(
                "  hitch frame {:>6}: {:7.3} ms (thr {:.2}) | ev {:.3} up {:.3} rd {:.3} pr {:.3} | \
                 sim {:.3} ext {:.3} (anim {:.3}) vfx {:.3} cam {:.3} aud {:.3} ui {:.3} | \
                 +{} pso ({:.1} ms) +{} res +{} flush ({:.1} ms)",
                hitch.frame,
                hitch.total_ms,
                hitch.threshold_ms,
                hitch.stages[0],
                hitch.stages[1],
                hitch.stages[2],
                hitch.stages[3],
                hitch.update.sim_ms,
                hitch.update.extract_ms,
                hitch.update.animate_ms,
                hitch.update.vfx_ms,
                hitch.update.camera_ms,
                hitch.update.audio_ms,
                hitch.update.ui_ms,
                hitch.created.pipelines,
                hitch.created.pipeline_ns as f64 / 1e6,
                hitch.created.resources,
                hitch.created.flushes,
                hitch.created.flush_ns as f64 / 1e6,
            );
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn stages(total_ms: f64) -> StageTimings {
        StageTimings {
            update: Duration::from_secs_f64(total_ms / 1e3),
            ..StageTimings::default()
        }
    }

    #[test]
    fn an_open_ended_run_records_nothing() {
        assert!(RunProfile::for_run(None).is_none());
    }

    #[test]
    fn the_whole_run_is_one_distribution() {
        let mut profile = RunProfile::for_run(Some(5000)).unwrap();
        for frame in 0..5000 {
            let ms = if frame % 1000 == 999 { 10.0 } else { 0.5 };
            profile.record(FrameSample {
                frame,
                stages: stages(ms),
                total_ms: ms,
                gpu_ms: None,
                running_p50_ms: 0.5,
                ..FrameSample::default()
            });
        }
        let (cpu, gpu) = profile.stats();
        let cpu = cpu.unwrap();
        assert!(gpu.is_none());
        assert_eq!(cpu.count, 5000);
        // Five slow frames in 5000 is 0.1%: the p99.9 is exactly the boundary of them.
        assert!((cpu.p99_ms - 0.5).abs() < 1e-9);
        assert!((cpu.max_ms - 10.0).abs() < 1e-9);
    }

    #[test]
    fn only_the_worst_hitches_are_kept_and_the_mild_ones_are_not_hitches() {
        let mut profile = RunProfile::for_run(Some(1000)).unwrap();
        for frame in 0..1000u64 {
            // 1.5 ms is 3x a 0.5 ms median and under the floor: not a hitch.
            let ms = match frame % 10 {
                0 => 1.5,
                1 => 3.0 + frame as f64 / 1000.0,
                _ => 0.5,
            };
            profile.record(FrameSample {
                frame,
                stages: stages(ms),
                total_ms: ms,
                gpu_ms: Some(0.2),
                running_p50_ms: 0.5,
                ..FrameSample::default()
            });
        }
        assert_eq!(profile.hitches.count, 100);
        assert_eq!(profile.hitches.worst.len(), WORST_KEPT);
        let mildest = profile
            .hitches
            .worst
            .iter()
            .map(|hitch| hitch.total_ms)
            .fold(f64::INFINITY, f64::min);
        assert!(
            mildest > 3.7,
            "the 24 worst of 100 are the last 24: {mildest}"
        );
    }

    #[test]
    fn the_warm_up_boundary_forgets_everything() {
        let mut profile = RunProfile::for_run(Some(100)).unwrap();
        profile.record(FrameSample {
            frame: 0,
            stages: stages(50.0),
            total_ms: 50.0,
            gpu_ms: Some(1.0),
            running_p50_ms: 0.5,
            ..FrameSample::default()
        });
        profile.clear();
        assert_eq!(profile.stats().0, None);
        assert_eq!(profile.hitches.count, 0);
    }
}
