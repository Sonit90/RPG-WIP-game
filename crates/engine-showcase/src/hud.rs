//! The statistics overlay's text, and the frame-time history behind its graph.
//!
//! Every number here comes from the engine: frame times from
//! [`engine_core::metrics::FrameMetrics`], cull counts from
//! [`engine_renderer::Renderer::cull_stats`], draw calls and triangles from the command
//! context's own counters. Nothing is estimated, and a value the engine cannot supply — GPU
//! time before the first timestamp resolves — is printed as `N/A` rather than invented.
//!
//! The overlay font has uppercase letters, digits and a handful of punctuation and nothing
//! else, so every line is written uppercase.

use std::fmt::Write as _;

use engine_core::math::Vec3;
use engine_core::metrics::FrameSummary;
use engine_renderer::CullStats;

/// Frames of frame-time history the graph shows.
pub const GRAPH_SAMPLES: usize = 120;

/// The frame time the graph's full height represents, in milliseconds.
///
/// 4 ms is 250 FPS. A bar that reaches the top is at the reference line; one that would
/// exceed it is clamped and recoloured, so a hitch is visible as a red bar rather than as a
/// rescaled graph in which every other bar suddenly shrinks.
pub const GRAPH_BUDGET_MS: f32 = 4.0;

/// A fixed-length ring of recent frame times.
///
/// Its own history rather than the engine's: `FrameMetrics` keeps a window for percentiles
/// and exposes the summary, not the samples, and a graph needs the samples.
#[derive(Debug)]
pub struct FrameGraph {
    samples: Vec<f32>,
    /// Where the next sample goes.
    cursor: usize,
    filled: usize,
    /// The samples in chronological order, rebuilt when the graph is read.
    ordered: Vec<f32>,
}

impl Default for FrameGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameGraph {
    /// An empty history sized for [`GRAPH_SAMPLES`] frames.
    #[must_use]
    pub fn new() -> Self {
        Self {
            samples: vec![0.0; GRAPH_SAMPLES],
            cursor: 0,
            filled: 0,
            ordered: Vec::with_capacity(GRAPH_SAMPLES),
        }
    }

    /// Records one frame time, in milliseconds.
    pub fn record(&mut self, milliseconds: f32) {
        self.samples[self.cursor] = milliseconds;
        self.cursor = (self.cursor + 1) % self.samples.len();
        self.filled = (self.filled + 1).min(self.samples.len());
    }

    /// Forgets every sample. Used when the workload changes.
    pub fn clear(&mut self) {
        self.samples.fill(0.0);
        self.cursor = 0;
        self.filled = 0;
        self.ordered.clear();
    }

    /// The recorded frame times, oldest first.
    ///
    /// Rebuilt into a contiguous buffer that is reused between calls, so reading the graph
    /// allocates only on the first call.
    pub fn ordered(&mut self) -> &[f32] {
        self.ordered.clear();
        if self.filled < self.samples.len() {
            self.ordered.extend_from_slice(&self.samples[..self.filled]);
        } else {
            self.ordered.extend_from_slice(&self.samples[self.cursor..]);
            self.ordered.extend_from_slice(&self.samples[..self.cursor]);
        }
        &self.ordered
    }
}

/// Everything the overlay reports, gathered from the engine in one place.
///
/// A struct rather than a dozen parameters so it is obvious at the call site which value
/// came from where, and so adding a line to the overlay does not change a signature.
#[derive(Debug, Clone, Copy)]
pub struct HudStats<'a> {
    /// Frame times and stage averages from the engine loop.
    pub summary: FrameSummary,
    /// What the last cull pass did.
    pub cull: CullStats,
    /// Milliseconds the cull pass itself took.
    pub cull_ms: f64,
    /// Objects in the world, culled or not.
    pub objects: usize,
    /// Triangles the world holds before culling.
    pub scene_triangles: u64,
    /// Draw calls the last frame recorded.
    pub draw_calls: u32,
    /// Triangles the last frame submitted.
    pub triangles: u64,
    /// Where the camera is.
    pub camera_position: Vec3,
    /// Back-buffer size.
    pub resolution: (u32, u32),
    /// Name of the loaded preset.
    pub preset: &'a str,
    /// Whether frustum culling is on.
    pub culling: bool,
    /// Whether the cull frustum is frozen away from the camera.
    pub cull_frozen: bool,
    /// Whether the world is drawn as wireframe.
    pub wireframe: bool,
    /// Whether the mouse is captured for look control.
    pub mouse_captured: bool,
}

/// Rewrites `lines` in place from `stats`.
///
/// In place because formatting allocates and the overlay is rebuilt several times a second:
/// after the first update this reuses the same `String` buffers rather than making new ones.
pub fn write_lines(lines: &mut Vec<String>, stats: &HudStats<'_>) {
    /// Number of lines the overlay writes.
    const LINE_COUNT: usize = 20;

    while lines.len() < LINE_COUNT {
        lines.push(String::new());
    }
    lines.truncate(LINE_COUNT);
    for line in lines.iter_mut() {
        line.clear();
    }

    let cpu = stats.summary.cpu;
    let stages = stats.summary.stages;

    let _ = write!(lines[0], "ENGINE SHOWCASE - {}", stats.preset);

    let _ = write!(lines[2], "FPS           {:>10.0}", cpu.mean_fps());
    let _ = write!(lines[3], "FRAME         {:>10.2} MS", cpu.mean_ms);
    // p50 next to p95 and p99, because a mean hides exactly the spikes the percentiles
    // exist to expose, and a p99 alone cannot say whether the typical frame moved with it.
    let _ = write!(
        lines[4],
        "P50/P95/P99   {:>5.2} {:>5.2} {:>5.2}",
        cpu.p50_ms, cpu.p95_ms, cpu.p99_ms
    );

    let _ = write!(lines[6], "CPU");
    let _ = write!(lines[7], "  RENDER      {:>10.2} MS", stages.render_ms);
    let _ = write!(lines[8], "  CULLING     {:>10.2} MS", stats.cull_ms);
    let _ = write!(lines[9], "  UPDATE      {:>10.2} MS", stages.update_ms);
    let _ = write!(lines[10], "  PRESENT     {:>10.2} MS", stages.present_ms);

    match stats.summary.gpu {
        // GPU timestamps lag the CPU by a few frames; reading them sooner would mean
        // blocking on the GPU, which would change what is being measured.
        Some(gpu) => {
            let _ = write!(lines[11], "GPU FRAME     {:>10.2} MS", gpu.mean_ms);
        }
        None => {
            let _ = write!(lines[11], "GPU FRAME     {:>10}", "N/A");
        }
    }

    let _ = write!(lines[13], "OBJECTS       {:>10}", stats.objects);
    let _ = write!(lines[14], "VISIBLE       {:>10}", stats.cull.visible);
    let _ = write!(lines[15], "CULLED        {:>10}", stats.cull.culled);
    let _ = write!(
        lines[16],
        "DRAWS         {:>10}   TRIS {}",
        stats.draw_calls, stats.triangles
    );
    let _ = write!(
        lines[17],
        "SCENE TRIS    {:>10}   {}X{}",
        stats.scene_triangles, stats.resolution.0, stats.resolution.1
    );
    let _ = write!(
        lines[18],
        "CAMERA  {:.0} / {:.0} / {:.0}",
        stats.camera_position.x, stats.camera_position.y, stats.camera_position.z
    );
    let _ = write!(
        lines[19],
        "CULLING {}   FROZEN {}   WIRE {}   MOUSE {}",
        on_off(stats.culling),
        on_off(stats.cull_frozen),
        on_off(stats.wireframe),
        if stats.mouse_captured { "LOOK" } else { "FREE" }
    );
}

/// `ON` or `OFF`, for a toggle line.
fn on_off(value: bool) -> &'static str {
    if value { "ON" } else { "OFF" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::metrics::StageAverages;
    use engine_core::time::FrameStats;

    fn stats() -> HudStats<'static> {
        HudStats {
            summary: FrameSummary {
                cpu: FrameStats {
                    count: 120,
                    mean_ms: 2.12,
                    min_ms: 1.90,
                    max_ms: 3.40,
                    p50_ms: 2.10,
                    p95_ms: 2.60,
                    p99_ms: 3.10,
                },
                gpu: None,
                stages: StageAverages::default(),
                frame_index: 0,
            },
            cull: CullStats {
                considered: 10_000,
                visible: 1_842,
                culled: 8_158,
            },
            cull_ms: 0.08,
            objects: 10_000,
            scene_triangles: 420_000,
            draw_calls: 1_842,
            triangles: 42_310,
            camera_position: Vec3::new(12.4, 4.1, -8.2),
            resolution: (1920, 1080),
            preset: "LARGE",
            culling: true,
            cull_frozen: false,
            wireframe: false,
            mouse_captured: true,
        }
    }

    #[test]
    fn a_full_ring_reads_oldest_first() {
        let mut graph = FrameGraph::new();
        for index in 0..GRAPH_SAMPLES + 5 {
            graph.record(index as f32);
        }
        let ordered = graph.ordered().to_vec();
        assert_eq!(ordered.len(), GRAPH_SAMPLES);
        assert_eq!(ordered[0], 5.0, "the five oldest were overwritten");
        assert_eq!(ordered[GRAPH_SAMPLES - 1], (GRAPH_SAMPLES + 4) as f32);
    }

    #[test]
    fn a_partly_filled_ring_reports_only_what_it_has() {
        let mut graph = FrameGraph::new();
        graph.record(1.0);
        graph.record(2.0);
        assert_eq!(graph.ordered(), &[1.0, 2.0]);
    }

    #[test]
    fn clearing_forgets_the_previous_workload() {
        let mut graph = FrameGraph::new();
        for _ in 0..50 {
            graph.record(3.0);
        }
        graph.clear();
        assert!(graph.ordered().is_empty());
    }

    #[test]
    fn reading_the_graph_does_not_reallocate() {
        let mut graph = FrameGraph::new();
        for index in 0..GRAPH_SAMPLES * 2 {
            graph.record(index as f32);
        }
        let _ = graph.ordered();
        let capacity = graph.ordered.capacity();
        for _ in 0..10 {
            let _ = graph.ordered();
        }
        assert_eq!(graph.ordered.capacity(), capacity);
    }

    #[test]
    fn every_line_is_writable_by_the_overlay_font() {
        // The font has uppercase letters, digits and a little punctuation. A lowercase
        // letter is silently dropped, which reads as a missing word rather than as a bug.
        const ALLOWED: &str = " 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ.,:-+/()%=_";
        let mut lines = Vec::new();
        write_lines(&mut lines, &stats());
        for line in &lines {
            for character in line.chars() {
                assert!(
                    ALLOWED.contains(character),
                    "{character:?} in {line:?} has no glyph"
                );
            }
        }
    }

    #[test]
    fn missing_gpu_timings_are_reported_rather_than_invented() {
        let mut lines = Vec::new();
        write_lines(&mut lines, &stats());
        assert!(
            lines.iter().any(|line| line.contains("N/A")),
            "an unresolved GPU timestamp must print N/A"
        );
    }

    #[test]
    fn the_cull_counts_come_from_the_renderer() {
        let mut lines = Vec::new();
        write_lines(&mut lines, &stats());
        let joined = lines.join("\n");
        assert!(joined.contains("10000"), "{joined}");
        assert!(joined.contains("1842"), "{joined}");
        assert!(joined.contains("8158"), "{joined}");
    }

    #[test]
    fn rewriting_reuses_the_same_buffers() {
        let mut lines = Vec::new();
        write_lines(&mut lines, &stats());
        let capacities: Vec<usize> = lines.iter().map(String::capacity).collect();
        write_lines(&mut lines, &stats());
        let after: Vec<usize> = lines.iter().map(String::capacity).collect();
        assert_eq!(capacities, after);
    }
}
