//! A level says where the player starts, and the player starts there.
//!
//! ```text
//!   level ──► spawn_points[0] ──► ArenaConfig ──► the player's body: position and facing
//!               │
//!               └── none authored ──► spawns.player, as it always was
//! ```
//!
//! The editor decides *where*; this crate decides what that means. The two are joined by
//! [`EnvironmentScene::player_spawn`], and the whole point of that function is that nothing
//! here has to know whether the answer came from an authored spawn point or from the layout
//! field that predates them — which is what keeps every level built before spawn points
//! existed loading exactly as it did.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_core::math::Vec3;
use engine_environment::spawn::SpawnPoint;
use engine_environment::{BrushMode, EnvironmentScene};
use engine_game::arena::ArenaConfig;

/// Where the simulation actually put the player, and which way it turned them.
fn player_at(scene: &EnvironmentScene) -> (Vec3, f32) {
    let simulation = engine_game::arena::environment_arena(scene, 0);
    let player = simulation.player().entity;
    let position = simulation
        .world()
        .position(player)
        .expect("the player is in the world");
    let facing = simulation
        .world()
        .body(player)
        .expect("the player has a body")
        .facing_yaw;
    (position, facing)
}

#[test]
fn the_player_starts_on_the_levels_spawn_point_facing_the_way_it_points() {
    let mut scene = EnvironmentScene::empty("spawn_test");
    scene.push_spawn(SpawnPoint::new(Vec3::new(12.0, 0.0, -8.0)).with_yaw(1.25));

    let (position, facing) = player_at(&scene);
    assert!(
        (position.x - 12.0).abs() < 0.5 && (position.z + 8.0).abs() < 0.5,
        "the player started at {position:?} rather than on the spawn point"
    );
    assert!(
        (facing - 1.25).abs() < 1e-4,
        "the player faced {facing} rather than the way the spawn point points"
    );
}

#[test]
fn a_level_with_no_spawn_point_starts_the_player_where_it_always_did() {
    // Every level built before spawn points existed, and every built-in layout. The absence
    // of a spawn point is not an error and must never be treated as one.
    let mut scene = EnvironmentScene::empty("no_spawn");
    scene.spawns.player = Vec3::new(-5.0, 0.0, 3.0);
    assert!(scene.spawn_points.is_empty());

    let (position, facing) = player_at(&scene);
    assert!(
        (position.x + 5.0).abs() < 0.5 && (position.z - 3.0).abs() < 0.5,
        "the player started at {position:?} rather than at the layout's own spawn"
    );
    assert_eq!(facing, 0.0, "an unauthored start faces the way it always did");
}

#[test]
fn a_spawn_point_that_is_not_a_place_is_stepped_over_rather_than_used() {
    // A hand-edited level is refused by the loader, but a scene built in memory can still
    // hold one — and a player put at a position that is not a number is a player nothing can
    // find again. The next usable spawn point wins, and a level with none falls back.
    let mut scene = EnvironmentScene::empty("broken_spawn");
    scene.push_spawn(SpawnPoint {
        position: Vec3::new(f32::NAN, 0.0, 0.0),
        yaw: 0.0,
    });
    scene.push_spawn(SpawnPoint::new(Vec3::new(7.0, 0.0, 7.0)));

    let (position, _) = player_at(&scene);
    assert!(
        (position.x - 7.0).abs() < 0.5,
        "the player was put at {position:?}"
    );
}

#[test]
fn a_spawn_point_is_lifted_onto_ground_that_was_raised_under_it() {
    // A spawn authored at y = 0 on a level whose ground was sculpted afterwards would start
    // the player inside a hill. The lift is `EnvironmentScene::ground_at`'s, so it is the same
    // rule a placement and a debug marker are put down with.
    let mut scene = EnvironmentScene::empty("hill_spawn");
    let mut terrain = engine_environment::Terrain::flat(33, 2.0, 0.0);
    terrain.apply_brush(Vec3::ZERO, 12.0, 5.0, BrushMode::Raise);
    scene.terrain = Some(terrain);
    scene.push_spawn(SpawnPoint::new(Vec3::ZERO));

    let config = ArenaConfig::from_environment(&scene, 0);
    assert!(
        config.player_spawn.y > 0.5,
        "the spawn stayed at {:?}, inside the hill",
        config.player_spawn
    );
}

#[test]
fn the_first_spawn_point_is_the_one_used_however_many_the_level_has() {
    // A level has to start the same way twice. Anything that wants more than one reads the
    // list itself and decides — the runtime's own answer is "the first".
    let mut scene = EnvironmentScene::empty("many_spawns");
    scene.push_spawn(SpawnPoint::new(Vec3::new(1.0, 0.0, 1.0)));
    scene.push_spawn(SpawnPoint::new(Vec3::new(20.0, 0.0, 20.0)));
    scene.push_spawn(SpawnPoint::new(Vec3::new(-20.0, 0.0, -20.0)));

    for _ in 0..3 {
        let (position, _) = player_at(&scene);
        assert!((position.x - 1.0).abs() < 0.5, "{position:?}");
    }
    assert_eq!(scene.spawn_points.len(), 3, "the list is still the level's");
}
