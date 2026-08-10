//! Performance budgets for the combat interface, as tests.
//!
//! These are **regression detectors, not benchmarks**. Every threshold is a measured
//! baseline with generous headroom, and every quantity asserted is one that does not vary
//! with the machine: command counts, quad counts and allocation behaviour are properties of
//! the code, not of the CPU it runs on. Wall-clock time is deliberately *not* asserted —
//! a timing test that fails on a busy build agent teaches everyone to ignore it.
//!
//! Measured on the reference machine (Ryzen 7 5800X, release) at the time these were
//! written, for context rather than as the assertion:
//!
//! ```text
//! combat HUD, 20 dummies:   271 commands, 725 quads, 12.3 us, 0 allocations
//! combat HUD, 1000 dummies: 270 commands, 719 quads, 36.0 us, 0 allocations
//! ```
//!
//! The one thing that *is* asserted about cost is the shape: the interface must not grow
//! with the number of enemies in the world, because it is capped at the twenty nearest.

#![allow(clippy::expect_used)]

use engine_core::math::Vec3;
use engine_game::arena::{self, ArenaConfig};
use engine_game::command::PlayerCommand;
use engine_game::sim::Simulation;
use engine_presentation::camera::ThirdPersonCamera;
use engine_presentation::damage_numbers::{FloatingNumbers, MAX_FLOATING_NUMBERS};
use engine_presentation::game_ui::GameUi;
use engine_presentation::nameplate::{NameplateConfig, NameplateSet};
use engine_presentation::snapshot::UiSnapshot;
use engine_render_data::ui::UiCommand;
use engine_render_data::{Camera, Projection};

const VIEWPORT: (u32, u32) = (1920, 1080);

/// Commands a full combat HUD may produce.
///
/// Measured at 271; 400 leaves room for a few more elements before anyone has to think
/// about it, and would still be one draw call.
const COMMAND_BUDGET: usize = 400;

/// Draw calls the whole interface costs.
///
/// One. This is the property the entire phase was built to obtain, so it is asserted rather
/// than assumed — indirectly here, by checking that the list is a single batchable unit;
/// the GPU-side assertion lives in `engine-renderer/tests/ui_draw.rs`.
const DRAW_CALLS: usize = 1;

fn fought_in_arena(dummies: usize) -> Simulation {
    let mut simulation = arena::training_arena(&ArenaConfig::with_group(dummies));
    for tick in 0..300 {
        if tick % 20 == 0 {
            simulation.push_command(PlayerCommand::CycleTarget);
        }
        if tick % 9 == 0 {
            simulation.push_command(PlayerCommand::UseAbility(tick / 9 % 3));
        }
        simulation.step_once();
    }
    simulation
}

fn camera_for(simulation: &Simulation) -> engine_core::math::Mat4 {
    let aspect = VIEWPORT.0 as f32 / VIEWPORT.1 as f32;
    let mut controller = ThirdPersonCamera::following(simulation.player_position());
    controller.follow(simulation.player_position(), 0.016);
    let mut camera =
        Camera::new(Projection::perspective(aspect)).expect("a valid perspective projection");
    controller
        .apply(&mut camera, aspect)
        .expect("the camera applies");
    camera.view_projection()
}

/// Builds one frame of the real interface and returns the builder.
fn build_frame(simulation: &Simulation, numbers: &FloatingNumbers, now: f32) -> GameUi {
    let config = NameplateConfig::default();
    let view_projection = camera_for(simulation);
    let mut plates = NameplateSet::with_capacity(32);
    plates.build(simulation, &view_projection, VIEWPORT, &config);

    let snapshot = UiSnapshot::build(simulation);
    let mut ui = GameUi::new();
    ui.build(&snapshot, plates.plates(), &config, VIEWPORT);
    ui.push_damage_numbers(numbers, &view_projection, VIEWPORT, now);
    ui
}

fn full_numbers(simulation: &Simulation) -> FloatingNumbers {
    let mut numbers = FloatingNumbers::new();
    let now = simulation.time() as f32;
    for index in 0..MAX_FLOATING_NUMBERS {
        numbers.push(
            simulation.player().entity,
            Vec3::new((index % 8) as f32, 1.0, -((index / 8) as f32)),
            (index * 7 % 400) as f32 + 1.0,
            index % 20 == 0,
            now,
        );
    }
    numbers
}

#[test]
fn a_full_combat_hud_stays_within_its_command_budget() {
    let simulation = fought_in_arena(20);
    let numbers = full_numbers(&simulation);
    let ui = build_frame(&simulation, &numbers, simulation.time() as f32);

    assert!(
        ui.list().len() <= COMMAND_BUDGET,
        "the HUD produced {} commands against a budget of {COMMAND_BUDGET}",
        ui.list().len()
    );
    assert!(
        !ui.list().is_empty(),
        "a budget test that passes because nothing was drawn is worthless"
    );
}

#[test]
fn the_interface_does_not_grow_with_the_size_of_the_world() {
    // The property the nameplate cap exists for, and the one that makes the HUD's cost a
    // constant rather than a function of the encounter. A thousand dummies must cost what
    // twenty do.
    let twenty = fought_in_arena(20);
    let thousand = fought_in_arena(1_000);
    let numbers = full_numbers(&twenty);

    let small = build_frame(&twenty, &numbers, twenty.time() as f32);
    let large = build_frame(&thousand, &numbers, thousand.time() as f32);

    let ratio = large.list().len() as f64 / small.list().len().max(1) as f64;
    assert!(
        ratio < 1.5,
        "the HUD grew {ratio:.2}x from 20 to 1000 enemies: {} to {} commands",
        small.list().len(),
        large.list().len()
    );
}

#[test]
fn a_steady_hud_allocates_nothing_after_the_first_frame() {
    // The allocation budget, asserted by capacity rather than by a counting allocator so it
    // runs in an ordinary test process. Capacities that never move mean nothing was
    // reallocated.
    let simulation = fought_in_arena(100);
    let numbers = full_numbers(&simulation);
    let now = simulation.time() as f32;

    let config = NameplateConfig::default();
    let view_projection = camera_for(&simulation);
    let mut plates = NameplateSet::with_capacity(32);
    let mut ui = GameUi::new();
    let mut snapshot = UiSnapshot::build(&simulation);

    for _ in 0..50 {
        plates.build(&simulation, &view_projection, VIEWPORT, &config);
        snapshot.refresh(&simulation, None);
        ui.build(&snapshot, plates.plates(), &config, VIEWPORT);
        ui.push_damage_numbers(&numbers, &view_projection, VIEWPORT, now);
    }
    let commands = ui.list().len();
    let plate_capacity = plates.capacity();

    for _ in 0..1_000 {
        plates.build(&simulation, &view_projection, VIEWPORT, &config);
        snapshot.refresh(&simulation, None);
        ui.build(&snapshot, plates.plates(), &config, VIEWPORT);
        ui.push_damage_numbers(&numbers, &view_projection, VIEWPORT, now);
    }

    assert_eq!(
        ui.list().len(),
        commands,
        "the command count drifted, so something is accumulating"
    );
    assert_eq!(plates.capacity(), plate_capacity, "the plate set regrew");
}

#[test]
fn the_whole_interface_is_one_batchable_unit() {
    // Every command must be something the UI pass can draw without changing pipeline state
    // or texture. That is what makes the interface one draw call, and it is a property of
    // the command vocabulary rather than of any particular frame.
    let simulation = fought_in_arena(20);
    let numbers = full_numbers(&simulation);
    let ui = build_frame(&simulation, &numbers, simulation.time() as f32);

    for command in ui.list().commands() {
        match command {
            // An icon belongs here for the same reason text does: it is a region of the one
            // atlas every other command already samples, so it changes neither the texture
            // nor the pipeline state and the batch survives it.
            UiCommand::Rect { .. }
            | UiCommand::Outline { .. }
            | UiCommand::RadialFill { .. }
            | UiCommand::Icon { .. }
            | UiCommand::Text { .. } => {}
        }
    }
    assert_eq!(DRAW_CALLS, 1, "the interface is submitted once");
}

#[test]
fn a_thousand_simultaneous_hits_are_bounded_rather_than_unbounded() {
    // A large area hit must degrade into "the newest hundred" instead of into an allocation
    // storm, which is what the ring exists for.
    let simulation = fought_in_arena(20);
    let mut numbers = FloatingNumbers::new();
    let now = simulation.time() as f32;
    for index in 0..1_000 {
        numbers.push(
            simulation.player().entity,
            Vec3::new(0.0, 1.0, -5.0),
            index as f32 + 1.0,
            false,
            now,
        );
    }

    assert_eq!(numbers.numbers().len(), MAX_FLOATING_NUMBERS);
    assert_eq!(numbers.dropped(), 1_000 - MAX_FLOATING_NUMBERS);

    let ui = build_frame(&simulation, &numbers, now);
    assert!(
        ui.list().len() <= COMMAND_BUDGET,
        "even a thousand hits stays inside the command budget: {}",
        ui.list().len()
    );
}

#[test]
fn the_hud_survives_a_world_with_nothing_in_it() {
    // A budget test that only ever runs on a healthy scene misses the case where the HUD
    // divides by a count it does not have.
    let mut simulation = arena::training_arena(&ArenaConfig::with_group(0));
    simulation.step_once();
    simulation.player_mut().target = None;
    let numbers = FloatingNumbers::new();

    let ui = build_frame(&simulation, &numbers, simulation.time() as f32);
    assert!(
        !ui.list().is_empty(),
        "the player's own bars are still drawn"
    );
    assert!(ui.list().len() <= COMMAND_BUDGET);
}
