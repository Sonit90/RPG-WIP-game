//! The F5 developer overlay: frame rate and nothing else.
//!
//! ```text
//! FPS    412   CPU   2.43 MS   GPU   1.88 MS
//! CPU    P50   2.40   P95   3.00   P99   3.60 MS
//! SIM    0.031 MS   TICKS 1   60 HZ
//! MEM    12 / 1024 KIB   PEAK 40 KIB
//! ```
//!
//! A frame-rate readout, deliberately narrow. Position, target, damage, draw calls, cull
//! counts and the rest were once here and are not any more: the overlay is what a developer
//! glances at while playing, and a screenful of text covers the thing being measured.
//!
//! None of that instrumentation went away. Frame times still come from
//! [`engine_core::metrics::FrameMetrics`], simulation timings from
//! [`engine_game::sim::SimStats`], damage from [`engine_game::telemetry::CombatMeter`],
//! renderer statistics from the renderer, and the periodic log line and `engine-bench`
//! still report all of it. Only this overlay stopped drawing it.
//!
//! Every number comes from a measurement, and one that is not available yet prints as `N/A`
//! rather than as a zero that reads like a measurement.
//!
//! This is a developer readout, not a UI framework: it produces lines of text, and the
//! renderer's existing overlay draws them with a 5x7 bitmap font. That font has uppercase
//! letters, digits and a handful of punctuation and nothing else, so every line is written
//! uppercase and a test asserts that every character has a glyph.

use std::fmt::Write as _;

use engine_core::memory::MemoryStats;
use engine_core::metrics::FrameSummary;
use engine_game::sim::SimStats;

/// Bytes in a kibibyte, the unit the memory line is written in.
const KIB: f64 = 1024.0;

/// Everything the overlay reports, gathered in one place.
///
/// A struct rather than a handful of parameters, so it is obvious at the call site which
/// value came from where and so adding a line does not change a signature.
#[derive(Debug, Default, Clone, Copy)]
pub struct HudModel {
    /// Frame times from the engine loop, absent until the first metrics refresh.
    pub summary: Option<FrameSummary>,
    /// Where the last simulation advance that actually ticked spent its time.
    ///
    /// `Simulation::last_tick_stats`, not `stats`: at 300 FPS four frames in five run no
    /// ticks, and printing their zero would say the simulation is free rather than that it
    /// did not run this frame.
    pub sim: SimStats,
    /// Ticks the *current* frame ran, which is usually zero or one.
    pub ticks_this_frame: u32,
    /// The simulation's fixed tick rate, in hertz.
    pub tick_hz: f64,
    /// The frame allocator's usage, absent if the caller does not track one.
    pub memory: Option<MemoryStats>,
}

/// Rewrites `lines` in place from `model`.
///
/// In place because formatting allocates and the overlay is rebuilt several times a second:
/// after the first update this reuses the same `String` buffers rather than making new ones.
pub fn write_lines(lines: &mut Vec<String>, model: &HudModel) {
    let mut row = 0;
    let line = |lines: &mut Vec<String>, row: &mut usize| -> usize {
        while lines.len() <= *row {
            lines.push(String::new());
        }
        lines[*row].clear();
        let index = *row;
        *row += 1;
        index
    };

    let index = line(lines, &mut row);
    match model.summary {
        Some(summary) => {
            let _ = write!(
                lines[index],
                "FPS {:>6.0}   CPU {:>6.2} MS   GPU {}",
                summary.cpu.mean_fps(),
                summary.cpu.mean_ms,
                match summary.gpu {
                    Some(gpu) => format!("{:.2} MS", gpu.mean_ms),
                    None => "N/A".to_owned(),
                }
            );
        }
        None => {
            let _ = write!(lines[index], "FPS    N/A   CPU    N/A   GPU N/A");
        }
    }

    // The tail as well as the mean: a mean hides the spike that a p99 makes obvious, and
    // a frame-rate readout that only averages cannot show a stutter at all.
    let index = line(lines, &mut row);
    match model.summary {
        Some(summary) => {
            let _ = write!(
                lines[index],
                "CPU P50 {:>5.2}  P95 {:>5.2}  P99 {:>5.2} MS",
                summary.cpu.p50_ms, summary.cpu.p95_ms, summary.cpu.p99_ms
            );
        }
        None => {
            let _ = write!(lines[index], "CPU P50   N/A  P95   N/A  P99   N/A");
        }
    }

    let index = line(lines, &mut row);
    let _ = write!(
        lines[index],
        "SIM {:>6.3} MS   TICKS {}   {:.0} HZ",
        model.sim.total_ms(),
        model.ticks_this_frame,
        model.tick_hz
    );

    let index = line(lines, &mut row);
    match model.memory {
        Some(memory) => {
            let _ = write!(
                lines[index],
                "MEM {:.0} / {:.0} KIB   PEAK {:.0} KIB",
                memory.used as f64 / KIB,
                memory.capacity as f64 / KIB,
                memory.peak as f64 / KIB
            );
        }
        None => {
            let _ = write!(lines[index], "MEM N/A");
        }
    }

    lines.truncate(row);
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_core::metrics::StageAverages;
    use engine_core::time::FrameStats;
    use engine_game::arena::{self, ArenaConfig};

    fn summary() -> FrameSummary {
        FrameSummary {
            cpu: FrameStats {
                count: 512,
                mean_ms: 2.43,
                min_ms: 2.0,
                max_ms: 4.1,
                p50_ms: 2.4,
                p95_ms: 3.0,
                p99_ms: 3.6,
            },
            gpu: Some(FrameStats {
                count: 500,
                mean_ms: 1.88,
                min_ms: 1.5,
                max_ms: 2.5,
                p50_ms: 1.85,
                p95_ms: 2.1,
                p99_ms: 2.3,
            }),
            stages: StageAverages::default(),
            frame_index: 1_000,
        }
    }

    fn memory() -> MemoryStats {
        MemoryStats {
            capacity: 1024 * 1024,
            used: 12 * 1024,
            peak: 40 * 1024,
            allocations: 7,
            failures: 0,
        }
    }

    /// A model built the way the application builds it, from a simulation that has run.
    fn model(summary: Option<FrameSummary>) -> HudModel {
        let mut simulation = arena::training_arena(&ArenaConfig::default());
        simulation.step_many(120);
        HudModel {
            summary,
            sim: simulation.last_tick_stats(),
            ticks_this_frame: simulation.stats().ticks,
            tick_hz: simulation.timestep().hz(),
            memory: Some(memory()),
        }
    }

    fn render(model: &HudModel) -> Vec<String> {
        let mut lines = Vec::new();
        write_lines(&mut lines, model);
        lines
    }

    #[test]
    fn every_character_has_a_glyph_in_the_overlay_font() {
        // The renderer's 5x7 font covers exactly this set; anything else renders as a
        // hollow box, which reads as a missing word rather than as a bug.
        const ALLOWED: &str = " 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ.,:-+/()%=_";
        for line in render(&model(Some(summary()))) {
            for character in line.chars() {
                assert!(
                    ALLOWED.contains(character),
                    "{character:?} in {line:?} has no glyph"
                );
            }
        }
    }

    #[test]
    fn the_overlay_reports_frame_rate_cpu_and_gpu_time() {
        let text = render(&model(Some(summary()))).join("\n");
        assert!(text.contains("FPS"), "{text}");
        assert!(text.contains("CPU"), "{text}");
        assert!(text.contains("GPU"), "{text}");
        assert!(
            text.contains("2.43"),
            "the measured CPU mean is shown: {text}"
        );
        assert!(
            text.contains("1.88"),
            "the measured GPU mean is shown: {text}"
        );
        assert!(text.contains("P99"), "the tail is shown: {text}");
        assert!(text.contains("3.60"), "the measured p99 is shown: {text}");
    }

    #[test]
    fn the_overlay_reports_the_simulation_rate_and_memory() {
        let text = render(&model(Some(summary()))).join("\n");
        assert!(text.contains("SIM"), "{text}");
        assert!(text.contains("TICKS"), "{text}");
        assert!(text.contains("60 HZ"), "the tick rate is shown: {text}");
        assert!(text.contains("MEM"), "{text}");
        assert!(text.contains("PEAK"), "{text}");
        assert!(text.contains("1024 KIB"), "the capacity is shown: {text}");
    }

    #[test]
    fn nothing_but_frame_rate_is_reported() {
        // The overlay's whole job: everything below is still measured, and none of it is
        // drawn here any more.
        let text = render(&model(Some(summary()))).join("\n");
        for forbidden in [
            "POS",
            "TARGET",
            "ENERGY",
            "DAMAGE",
            "DPS",
            "FIREBALL",
            "ABILITIES",
            "VISIBLE",
            "CULLED",
            "DRAWS",
            "EXTRACT",
            "CULL",
            "WASD",
            "MOVE",
        ] {
            assert!(
                !text.contains(forbidden),
                "{forbidden} is still on the overlay: {text}"
            );
        }
    }

    #[test]
    fn an_unavailable_measurement_prints_as_not_available() {
        // Before the first metrics refresh there are no frame times. Printing zero would
        // read as "the frame took no time".
        let text = render(&model(None)).join("\n");
        assert!(text.contains("N/A"), "{text}");
        assert!(!text.contains("FPS      0"), "{text}");

        let mut without_memory = model(None);
        without_memory.memory = None;
        assert!(render(&without_memory).join("\n").contains("MEM N/A"));
    }

    #[test]
    fn rewriting_reuses_the_same_buffers() {
        let model = model(Some(summary()));
        let mut lines = Vec::new();

        write_lines(&mut lines, &model);
        let capacities: Vec<usize> = lines.iter().map(String::capacity).collect();
        let count = lines.len();

        for _ in 0..10 {
            write_lines(&mut lines, &model);
        }
        assert_eq!(lines.len(), count, "the line count drifted");
        assert_eq!(
            lines.iter().map(String::capacity).collect::<Vec<_>>(),
            capacities,
            "the overlay reallocated its lines"
        );
    }

    #[test]
    fn the_overlay_stays_small_enough_to_glance_at() {
        // It is drawn over the game at a fixed cell size. A frame-rate readout that filled
        // the screen would cover the thing whose frame rate it reports.
        let lines = render(&model(Some(summary())));
        assert!(lines.len() <= 6, "{} lines", lines.len());
        let widest = lines.iter().map(|line| line.chars().count()).max();
        assert!(widest.is_some_and(|width| width <= 48), "{widest:?}");
    }
}
