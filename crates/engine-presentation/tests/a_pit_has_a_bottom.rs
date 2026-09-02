//! A character walks into a pit and stands on the bottom of it.
//!
//! ```text
//!   ▓▓▓▓▓▓▓▓            ▓▓▓▓▓▓▓▓     the terrain
//!           ╲          ╱
//!            ╲   o    ╱              the character, standing where the ground is
//!             ▓▓▓▓▓▓▓▓
//!   - - - - - - - - - - - - - -      where the floor plane used to be: y = 0
//! ```
//!
//! A hill worked and a pit did not, and nothing about the terrain was wrong: the arena put an
//! *infinite plane* at y = 0 under everything, so a pit dug to −6 m was a hole with a lid on
//! it at zero. The plane is gone for a level that brought a terrain, and this is the
//! assertion that says so — through the real loader and the real controller, because the two
//! descriptions of the ground live in crates that cannot see each other.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_collision::controller::{CharacterConfig, CharacterState, move_character};
use engine_collision::world::{CollisionWorld, QueryBuffers};
use engine_core::math::Vec3;
use engine_environment::{BrushMode, EnvironmentScene, Terrain};
use engine_game::arena::{self, ArenaConfig};

/// A level whose ground is a terrain with a six-metre pit dug in the middle of it.
fn level_with_a_pit() -> EnvironmentScene {
    let mut scene = EnvironmentScene::empty("pit_test");
    // The playable square is inside the terrain's 64 m span, which is what lets the level
    // drop its floor plane: a terrain that does not reach the walls keeps one.
    scene.half_extent = 30.0;
    let mut terrain = Terrain::flat(33, 2.0, 0.0);
    terrain.apply_brush(Vec3::ZERO, 12.0, 6.0, BrushMode::Lower);
    scene.terrain = Some(terrain);
    scene
}

/// The collision world the game builds for a level: floor if it has one, terrain, placements.
fn world_of(scene: &EnvironmentScene) -> CollisionWorld {
    let config = ArenaConfig::from_environment(scene, 0);
    let mut world = CollisionWorld::new();
    arena::build_collision(&mut world, &config);
    arena::add_terrain_collision(&mut world, scene);
    arena::add_environment_collision(&mut world, scene);
    world.rebuild_broad_phase();
    world
}

/// Drops a character at `at` and reports where it comes to rest.
fn settle(world: &CollisionWorld, at: Vec3) -> Vec3 {
    let config = CharacterConfig::default();
    let mut state = CharacterState::at(at);
    let mut buffers = QueryBuffers::new();
    // Ten seconds of falling, which is far longer than six metres needs.
    for _ in 0..600 {
        let fall = Vec3::new(0.0, -9.81 * (1.0 / 60.0) * 0.2, 0.0);
        move_character(world, &config, &mut state, fall, &mut buffers);
    }
    state.position
}

#[test]
fn a_character_dropped_into_a_pit_lands_on_the_bottom_of_it() {
    let scene = level_with_a_pit();
    let terrain = scene.terrain.as_ref().expect("the level has a terrain");
    let deepest = terrain
        .heights
        .iter()
        .copied()
        .fold(f32::INFINITY, f32::min);
    assert!(deepest < -1.0, "the pit is not a pit: {deepest} m");

    let landed = settle(&world_of(&scene), Vec3::new(0.0, 4.0, 0.0));

    assert!(
        (landed.y - deepest).abs() < 0.2,
        "the character stopped at {} rather than on the pit floor at {deepest}",
        landed.y
    );
}

#[test]
fn a_character_dropped_on_a_hill_still_lands_on_it() {
    // The other half of the same question: taking the plane away must not take the ground
    // away from the parts of the level that were never below zero.
    let mut scene = EnvironmentScene::empty("hill_test");
    // The playable square is inside the terrain's 64 m span, which is what lets the level
    // drop its floor plane: a terrain that does not reach the walls keeps one.
    scene.half_extent = 30.0;
    let mut terrain = Terrain::flat(33, 2.0, 0.0);
    terrain.apply_brush(Vec3::ZERO, 12.0, 5.0, BrushMode::Raise);
    scene.terrain = Some(terrain);

    let landed = settle(&world_of(&scene), Vec3::new(0.0, 20.0, 0.0));
    let highest = scene
        .terrain
        .as_ref()
        .expect("a terrain")
        .heights
        .iter()
        .copied()
        .fold(f32::NEG_INFINITY, f32::max);

    // Within half a metre of the summit rather than exactly on it: the capsule has a radius,
    // so it rests on the cells around the peak as well as on the peak itself.
    assert!(
        landed.y > highest - 0.5,
        "the character stopped at {} rather than on the hilltop at {highest}",
        landed.y
    );
}

#[test]
fn a_level_with_no_terrain_still_has_its_floor() {
    let scene = EnvironmentScene::empty("flat_test");
    let landed = settle(&world_of(&scene), Vec3::new(0.0, 5.0, 0.0));
    assert!(
        (landed.y - scene.ground_height).abs() < 0.1,
        "a level with no terrain dropped the character to {}",
        landed.y
    );
}
