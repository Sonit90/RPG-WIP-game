//! The developer overlay: performance, combat meter and ability bar.
//!
//! ```text
//! MYGAME - TRAINING ARENA
//!
//! FPS            412   CPU 2.43 MS   GPU 1.88 MS
//! SIM          0.031 MS   TICKS 1   60 HZ
//!   MOVE 0.004  ABIL 0.001  STAT 0.002  TELE 0.001
//! EXTRACT      0.012 MS   CULL 0.021 MS
//!
//! DAMAGE DONE      12431
//! DPS               8201
//!   FIREBALL        5400  (12)
//!   IGNITE          2100  (3)
//! ...
//! 1 FIREBALL  READY     2 IGNITE    4.2
//! ```
//!
//! Every number comes from a measurement: frame times from
//! [`engine_core::metrics::FrameSummary`], simulation timings from
//! [`engine_game::sim::SimStats`], damage from the [`engine_game::telemetry::CombatMeter`].
//! Nothing is estimated, and a value that is not available yet prints as `N/A` rather than
//! as a zero that reads like a measurement.
//!
//! This is a developer readout, not a UI framework: it produces lines of text, and the
//! renderer's existing overlay draws them with a 5x7 bitmap font. That font has uppercase
//! letters, digits and a handful of punctuation and nothing else, so every line is written
//! uppercase and a test asserts that every character has a glyph.

use std::fmt::Write as _;

use engine_core::metrics::FrameSummary;
use engine_game::ability::{ABILITY_SLOTS, AbilityId};
use engine_game::sim::{SimStats, Simulation};
use engine_game::telemetry::{AbilityTotals, TargetTotals};

/// Rows of the meter's per-ability breakdown.
const ABILITY_ROWS: usize = 4;

/// Rows of the meter's per-target breakdown.
const TARGET_ROWS: usize = 3;

/// Abilities shown per row of the ability bar.
const BAR_COLUMNS: usize = 2;

/// Everything the overlay reports, gathered in one place.
///
/// A struct rather than a dozen parameters, so it is obvious at the call site which value
/// came from where and so adding a line does not change a signature.
#[derive(Debug, Clone, Copy)]
pub struct HudModel<'a> {
    /// The simulation being played.
    pub simulation: &'a Simulation,
    /// Frame times from the engine loop, absent until the first refresh.
    pub summary: Option<FrameSummary>,
    /// Where the last simulation advance that actually ticked spent its time.
    ///
    /// `Simulation::last_tick_stats`, not `stats`: at 300 FPS four frames in five run no
    /// ticks, and printing their zero would say the simulation is free rather than that it
    /// did not run this frame.
    pub sim: SimStats,
    /// Ticks the *current* frame ran, which is usually zero or one.
    pub ticks_this_frame: u32,
    /// Milliseconds the last render extraction took.
    pub extract_ms: f64,
    /// Milliseconds the last cull pass took.
    pub cull_ms: f64,
    /// Entities the last cull pass kept.
    pub visible: u32,
    /// Entities the last cull pass rejected.
    pub culled: u32,
    /// Draw calls the last frame recorded.
    pub draw_calls: u32,
    /// Back-buffer size.
    pub resolution: (u32, u32),
}

/// Rewrites `lines` in place from `model`.
///
/// In place because formatting allocates and the overlay is rebuilt several times a second:
/// after the first update this reuses the same `String` buffers rather than making new
/// ones. Scratch buffers for the meter's rankings are the caller's, for the same reason.
pub fn write_lines(
    lines: &mut Vec<String>,
    model: &HudModel<'_>,
    abilities: &mut Vec<(AbilityId, AbilityTotals)>,
    targets: &mut Vec<TargetTotals>,
) {
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

    let simulation = model.simulation;
    let player = simulation.player();

    let index = line(lines, &mut row);
    let _ = write!(lines[index], "MYGAME - PHASE 3 TRAINING ARENA");

    // Performance.
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

    let index = line(lines, &mut row);
    let _ = write!(
        lines[index],
        "SIM {:>6.3} MS   TICKS {}   {:.0} HZ",
        model.sim.total_ms(),
        model.ticks_this_frame,
        simulation.timestep().hz()
    );

    let breakdown = model.sim.breakdown_ms();
    let index = line(lines, &mut row);
    let _ = write!(
        lines[index],
        "  MOVE {:.3}  ABIL {:.3}  STAT {:.3}  TELE {:.3}",
        breakdown[0], breakdown[1], breakdown[2], breakdown[3]
    );

    let index = line(lines, &mut row);
    let _ = write!(
        lines[index],
        "EXTRACT {:>6.3} MS   CULL {:>6.3} MS",
        model.extract_ms, model.cull_ms
    );

    let index = line(lines, &mut row);
    let _ = write!(
        lines[index],
        "VISIBLE {:>5}  CULLED {:>5}  DRAWS {:>4}  {}X{}",
        model.visible, model.culled, model.draw_calls, model.resolution.0, model.resolution.1
    );

    // Character.
    let index = line(lines, &mut row);
    let position = simulation.player_position();
    let state = simulation
        .player_body()
        .map_or("N/A", |body| body.state.as_str());
    let _ = write!(
        lines[index],
        "POS {:.1} / {:.1} / {:.1}   {}",
        position.x, position.y, position.z, state
    );

    let index = line(lines, &mut row);
    let target_text = match player
        .target
        .and_then(|id| simulation.world().health(id).map(|health| (id, *health)))
    {
        Some((id, health)) => format!(
            "DUMMY {} {:.0}/{:.0}",
            id.index(),
            health.current(),
            health.max()
        ),
        None => "NONE".to_owned(),
    };
    let _ = write!(
        lines[index],
        "ENERGY {:.0}/{:.0}   TARGET {target_text}",
        player.resource.current(),
        player.resource.max()
    );

    if let Some(error) = player.last_error {
        let index = line(lines, &mut row);
        let _ = write!(lines[index], "CANNOT CAST: {}", error.as_str());
    }

    // The meter.
    let meter = simulation.meter();
    let index = line(lines, &mut row);
    let _ = write!(lines[index], "DAMAGE DONE {:>12.0}", meter.total_damage());
    let index = line(lines, &mut row);
    let _ = write!(
        lines[index],
        "DPS {:>20.0}   {:.1} S",
        meter.dps(),
        meter.combat_seconds()
    );

    meter.ranked_abilities(abilities);
    for (id, totals) in abilities.iter().take(ABILITY_ROWS) {
        let name = player
            .abilities
            .get(*id)
            .map_or("UNKNOWN", |definition| definition.name);
        let index = line(lines, &mut row);
        let _ = write!(
            lines[index],
            "  {name:<10} {:>10.0}  ({} HITS)",
            totals.damage, totals.hits
        );
    }

    meter.ranked_targets(targets);
    if !targets.is_empty() {
        let index = line(lines, &mut row);
        let _ = write!(lines[index], "BY TARGET:");
        for totals in targets.iter().take(TARGET_ROWS) {
            let index = line(lines, &mut row);
            let _ = write!(
                lines[index],
                "  DUMMY {:<4} {:>10.0}  ({} HITS)",
                totals.entity.index(),
                totals.damage,
                totals.hits
            );
        }
    }

    // The ability bar, two to a row.
    let index = line(lines, &mut row);
    let _ = write!(lines[index], "ABILITIES:");
    for chunk in (0..ABILITY_SLOTS).collect::<Vec<_>>().chunks(BAR_COLUMNS) {
        let index = line(lines, &mut row);
        for slot in chunk {
            let Some(id) = AbilityId::from_slot(*slot) else {
                continue;
            };
            let Some(definition) = player.abilities.get(id) else {
                continue;
            };
            let remaining = player.abilities.cooldown_remaining(id);
            let state = if remaining <= 0.0 {
                "READY".to_owned()
            } else {
                format!("{remaining:.1}")
            };
            let _ = write!(
                lines[index],
                "{} {:<9} {:<7}",
                id.hotkey_digit(),
                definition.name,
                state
            );
        }
    }

    let index = line(lines, &mut row);
    let _ = write!(
        lines[index],
        "WASD MOVE  SPACE JUMP  1-0 CAST  TAB TARGET  R RESET"
    );

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
    use engine_game::command::PlayerCommand;

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

    fn model<'a>(simulation: &'a Simulation, summary: Option<FrameSummary>) -> HudModel<'a> {
        HudModel {
            simulation,
            summary,
            sim: simulation.last_tick_stats(),
            ticks_this_frame: simulation.stats().ticks,
            extract_ms: 0.012,
            cull_ms: 0.021,
            visible: 6,
            culled: 0,
            draw_calls: 4,
            resolution: (1920, 1080),
        }
    }

    /// A simulation that has fought for a while.
    fn fought() -> Simulation {
        let mut simulation = arena::training_arena(&ArenaConfig::default());
        simulation.step_once();
        for slot in [0, 1, 2] {
            simulation.push_command(PlayerCommand::UseAbility(
                AbilityId::from_slot(slot).expect("the slot exists"),
            ));
            simulation.step_many(4);
        }
        simulation.step_many(180);
        simulation
    }

    fn render(simulation: &Simulation, summary: Option<FrameSummary>) -> Vec<String> {
        let mut lines = Vec::new();
        let mut abilities = Vec::new();
        let mut targets = Vec::new();
        write_lines(
            &mut lines,
            &model(simulation, summary),
            &mut abilities,
            &mut targets,
        );
        lines
    }

    #[test]
    fn every_character_has_a_glyph_in_the_overlay_font() {
        // The renderer's 5x7 font covers exactly this set; anything else renders as a
        // hollow box, which reads as a missing word rather than as a bug.
        const ALLOWED: &str = " 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ.,:-+/()%=_";
        for line in render(&fought(), Some(summary())) {
            for character in line.chars() {
                assert!(
                    ALLOWED.contains(character),
                    "{character:?} in {line:?} has no glyph"
                );
            }
        }
    }

    #[test]
    fn the_meter_reports_total_damage_dps_and_a_breakdown() {
        let simulation = fought();
        let text = render(&simulation, Some(summary())).join("\n");

        assert!(text.contains("DAMAGE DONE"), "{text}");
        assert!(text.contains("DPS"), "{text}");
        assert!(text.contains("FIREBALL"), "{text}");
        assert!(text.contains("IGNITE"), "{text}");
        assert!(text.contains("BY TARGET"), "{text}");

        // The printed total must be the meter's total, not a recomputation.
        let expected = format!("{:.0}", simulation.meter().total_damage());
        assert!(text.contains(&expected), "{expected} missing from {text}");
    }

    #[test]
    fn the_ability_bar_shows_every_slot_with_its_key_and_cooldown() {
        let simulation = fought();
        let text = render(&simulation, Some(summary())).join("\n");

        for definition in simulation.player().abilities.definitions() {
            assert!(
                text.contains(definition.name),
                "{} is missing from the bar",
                definition.name
            );
        }
        assert!(text.contains("READY"), "no slot reads as ready: {text}");

        // Detonate has an eight-second cooldown and was cast three seconds ago.
        let detonate = AbilityId::from_slot(2).expect("slot 2 exists");
        let remaining = simulation.player().abilities.cooldown_remaining(detonate);
        assert!(remaining > 0.0, "the fixture did not cast detonate");
        assert!(
            text.contains(&format!("{remaining:.1}")),
            "the remaining cooldown {remaining:.1} is not shown: {text}"
        );
    }

    #[test]
    fn simulation_timing_is_reported_separately_from_frame_timing() {
        let simulation = fought();
        let text = render(&simulation, Some(summary())).join("\n");
        assert!(text.contains("SIM"), "{text}");
        assert!(text.contains("60 HZ"), "the tick rate is shown: {text}");
        assert!(text.contains("MOVE"), "the breakdown is shown: {text}");
        assert!(text.contains("EXTRACT"), "{text}");
        assert!(text.contains("CULL"), "{text}");
        assert!(text.contains("FPS"), "{text}");
    }

    #[test]
    fn an_unavailable_measurement_prints_as_not_available() {
        // Before the first metrics refresh there are no frame times. Printing zero would
        // read as "the frame took no time".
        let text = render(&fought(), None).join("\n");
        assert!(text.contains("N/A"), "{text}");
        assert!(!text.contains("FPS      0"), "{text}");
    }

    #[test]
    fn a_refused_cast_is_explained() {
        let mut simulation = arena::training_arena(&ArenaConfig::default());
        simulation.step_once();
        let fireball = AbilityId::from_slot(0).expect("slot 0 exists");
        simulation.use_ability(fireball).expect("in range");
        let _ = simulation.use_ability(fireball);

        let text = render(&simulation, Some(summary())).join("\n");
        assert!(text.contains("ON COOLDOWN"), "{text}");
    }

    #[test]
    fn the_target_and_its_health_are_shown() {
        let simulation = fought();
        let text = render(&simulation, Some(summary())).join("\n");
        assert!(text.contains("TARGET DUMMY"), "{text}");
        assert!(text.contains("ENERGY"), "{text}");
    }

    #[test]
    fn a_fresh_session_reads_as_zero_rather_than_as_nothing() {
        let mut simulation = arena::training_arena(&ArenaConfig::default());
        simulation.step_once();
        let text = render(&simulation, Some(summary())).join("\n");
        assert!(text.contains("DAMAGE DONE"), "{text}");
        assert!(text.contains("DPS"), "{text}");
    }

    #[test]
    fn rewriting_reuses_the_same_buffers() {
        let simulation = fought();
        let mut lines = Vec::new();
        let mut abilities = Vec::new();
        let mut targets = Vec::new();
        let model = model(&simulation, Some(summary()));

        write_lines(&mut lines, &model, &mut abilities, &mut targets);
        let capacities: Vec<usize> = lines.iter().map(String::capacity).collect();
        let count = lines.len();

        for _ in 0..10 {
            write_lines(&mut lines, &model, &mut abilities, &mut targets);
        }
        assert_eq!(lines.len(), count, "the line count drifted");
        assert_eq!(
            lines.iter().map(String::capacity).collect::<Vec<_>>(),
            capacities,
            "the overlay reallocated its lines"
        );
    }

    #[test]
    fn the_overlay_fits_a_reasonable_number_of_lines_and_columns() {
        // It is drawn over the game at a fixed cell size; an overlay that grows without
        // bound would cover the arena it exists to describe.
        let lines = render(&fought(), Some(summary()));
        assert!(lines.len() <= 28, "{} lines", lines.len());
        let widest = lines.iter().map(|line| line.chars().count()).max();
        assert!(widest.is_some_and(|width| width <= 56), "{widest:?}");
    }
}
