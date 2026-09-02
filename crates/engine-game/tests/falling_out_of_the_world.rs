//! A level with a terrain has no floor plane, so a fall has to end some other way.
//!
//! ```text
//!   ▓▓▓▓▓▓▓        ▓▓▓▓▓▓▓        the terrain, with a hole an author left in it
//!          ╲      ╱
//!            o                    the player, falling
//!            ┆
//!  ─ ─ ─ ─ ─ ┆ ─ ─ ─ ─ ─ ─ ─      void_height: the level's lowest ground, less 25 m
//!            ▼   10 s             → killed, and the ordinary respawn brings them back
//! ```
//!
//! The floor plane used to be the answer to every fall, and it was also the reason a pit dug
//! below zero had a lid on it. These are the two halves of taking it away: a pit is walkable,
//! and a fall through a hole is fatal rather than eternal.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_core::math::Vec3;
use engine_environment::spawn::SpawnPoint;
use engine_environment::{BrushMode, EnvironmentScene, Terrain};
use engine_game::sim::{Simulation, VOID_KILL_SECONDS};

/// A level whose ground is a terrain with a pit dug into it.
fn level_with_a_pit() -> EnvironmentScene {
    let mut scene = EnvironmentScene::empty("void_test");
    // The playable square is inside the terrain's 64 m span, which is what lets the level
    // drop its floor plane: a terrain that does not reach the walls keeps one.
    scene.half_extent = 30.0;
    let mut terrain = Terrain::flat(33, 2.0, 0.0);
    terrain.apply_brush(Vec3::ZERO, 12.0, 6.0, BrushMode::Lower);
    scene.terrain = Some(terrain);
    scene.push_spawn(SpawnPoint::new(Vec3::new(24.0, 0.0, 24.0)));
    scene
}

/// The deepest point of the level's ground.
fn lowest(scene: &EnvironmentScene) -> f32 {
    scene
        .terrain
        .as_ref()
        .expect("the level has a terrain")
        .heights
        .iter()
        .copied()
        .fold(f32::INFINITY, f32::min)
}

fn run(simulation: &mut Simulation, seconds: f32) {
    let ticks = (seconds * 60.0).round() as u32;
    simulation.step_many(ticks);
}

/// Puts the player at `y` and holds them there, tick by tick, for `seconds`.
///
/// Held rather than dropped, because this is a test about the clock and not about gravity:
/// a body in free fall would be somewhere else on every tick and the assertion would be
/// about how fast it fell.
fn hold_at(simulation: &mut Simulation, y: f32, seconds: f32) {
    let player = simulation.player().entity;
    let ticks = (seconds * 60.0).round() as u32;
    for _ in 0..ticks {
        if let Some(body) = simulation.world_mut().body_mut(player) {
            body.position.y = y;
            body.velocity = Vec3::ZERO;
        }
        simulation.step_once();
    }
}

#[test]
fn a_level_with_a_terrain_has_no_floor_plane_under_its_pits() {
    let scene = level_with_a_pit();
    let config = engine_game::arena::ArenaConfig::from_environment(&scene, 0);
    assert_eq!(
        config.floor, None,
        "the terrain is the ground; a plane under it is a lid on every pit"
    );

    let flat = EnvironmentScene::empty("no_terrain");
    let config = engine_game::arena::ArenaConfig::from_environment(&flat, 0);
    assert_eq!(
        config.floor,
        Some(flat.ground_height),
        "a level with no terrain still needs a floor"
    );
}

#[test]
fn the_bottom_of_the_world_is_below_the_deepest_ground() {
    let scene = level_with_a_pit();
    let void = engine_game::arena::void_height_of(&scene);
    assert!(
        void < lowest(&scene),
        "the void at {void} is not below the deepest ground at {}",
        lowest(&scene)
    );
}

#[test]
fn standing_in_the_deepest_pit_is_not_falling_out_of_the_world() {
    let scene = level_with_a_pit();
    let mut simulation = engine_game::arena::environment_arena(&scene, 0);

    // Twice as long as the fall would take to become fatal, at the very bottom of the level.
    hold_at(&mut simulation, lowest(&scene), VOID_KILL_SECONDS * 2.0);

    assert!(
        simulation.player_alive(),
        "a player standing in a pit was counted as having fallen out of the level"
    );
    assert_eq!(simulation.void_seconds(), 0.0);
}

#[test]
fn ten_seconds_below_the_world_kills_the_player() {
    let scene = level_with_a_pit();
    let mut simulation = engine_game::arena::environment_arena(&scene, 0);
    let under = simulation.void_height() - 5.0;

    // A second short of the limit: still falling, still alive.
    hold_at(&mut simulation, under, VOID_KILL_SECONDS - 1.0);
    assert!(
        simulation.player_alive(),
        "the fall was fatal before {VOID_KILL_SECONDS} s"
    );
    assert!(simulation.void_seconds() > 0.0, "the clock did not start");

    hold_at(&mut simulation, under, 1.5);
    assert!(
        !simulation.player_alive(),
        "{VOID_KILL_SECONDS} s below the world did not kill the player"
    );
}

#[test]
fn coming_back_above_the_line_forgets_the_fall() {
    let scene = level_with_a_pit();
    let mut simulation = engine_game::arena::environment_arena(&scene, 0);
    let under = simulation.void_height() - 5.0;

    hold_at(&mut simulation, under, VOID_KILL_SECONDS - 2.0);
    assert!(simulation.void_seconds() > 0.0);

    // Back on the ground, and then below the line again for almost as long as before: a fall
    // that was survived must not be added to the next one.
    hold_at(&mut simulation, 0.0, 0.5);
    assert_eq!(simulation.void_seconds(), 0.0, "the clock did not reset");

    hold_at(&mut simulation, under, VOID_KILL_SECONDS - 2.0);
    assert!(
        simulation.player_alive(),
        "two survivable falls added up to a fatal one"
    );
}

#[test]
fn the_fall_is_an_ordinary_death_and_the_player_comes_back() {
    let scene = level_with_a_pit();
    let mut simulation = engine_game::arena::environment_arena(&scene, 0);
    let under = simulation.void_height() - 5.0;

    hold_at(&mut simulation, under, VOID_KILL_SECONDS + 0.5);
    assert!(!simulation.player_alive());

    // The respawn clock is the one the rest of the game uses; nothing here is a second way
    // back into the level.
    run(
        &mut simulation,
        engine_game::sim::respawn::RESPAWN_DELAY + 0.5,
    );
    assert!(simulation.player_alive(), "the player never came back");
    let position = simulation.player_position();
    assert!(
        position.y > simulation.void_height(),
        "they came back below the world at {position:?}"
    );
}

#[test]
fn a_level_with_no_terrain_can_never_fall_out_of_the_world() {
    // Every test bed and every benchmark is this: a floor plane at zero and nothing else.
    let mut simulation =
        engine_game::arena::training_arena(&engine_game::arena::ArenaConfig::with_group(0));
    assert!(simulation.void_height() < -1_000.0);

    hold_at(&mut simulation, -20.0, VOID_KILL_SECONDS * 2.0);
    assert!(
        simulation.player_alive(),
        "a layout with a floor plane killed somebody for standing below zero"
    );
}
