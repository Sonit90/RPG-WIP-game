//! A character walks *under* a cliff overhang and *through* a cave tunnel.
//!
//! ```text
//!   ▓▓▓▓▓▓▓▓▓        the slab                 ▓▓▓▓▓▓▓▓▓▓▓▓   the roof
//!            ▓                              ▓            ▓
//!      o──►  ▓  the character walks in     ▓   o───►     ▓   and out the far end
//!   ▓▓▓▓▓▓▓▓▓▓                              ▓▓▓▓▓▓▓▓▓▓▓▓▓
//! ```
//!
//! This is the requirement the whole cliff and cave system exists for, and it is the one that
//! a bounding box silently breaks: a box around an overhang is a wall, and a box around a
//! tunnel is a rock. Nothing about the geometry says so — the level looks right in a
//! screenshot and is unwalkable in the game — so the assertion has to be a character actually
//! moving, through the real controller, in a world built by the real level loader.
//!
//! It lives here because it needs three crates that cannot see each other: the sidecar and
//! the starter library (`engine-assets`), the level (`engine-environment`), and the collision
//! world and controller (`engine-collision`, through `engine-game`'s loader).

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_assets::procedural::starter::starter_collision;
use engine_collision::controller::{CharacterConfig, CharacterState, move_character};
use engine_collision::world::{CollisionWorld, QueryBuffers};
use engine_core::math::Vec3;
use engine_core::transform::Transform;
use engine_environment::{EnvironmentAsset, EnvironmentObject, EnvironmentScene};

/// A level holding one placed piece, with its authored collision.
fn level_with(piece: &str, at: Vec3, yaw: f32) -> EnvironmentScene {
    let mut scene = EnvironmentScene::empty("walk_test");
    let shape = starter_collision(piece).unwrap_or_else(|| panic!("{piece} is authored"));
    let collision = scene.intern_collision(shape);
    scene.push(EnvironmentObject {
        asset: EnvironmentAsset::interned(&format!("generated/environment/{piece}")),
        transform: Transform::from_translation(at)
            .with_rotation(engine_core::math::Quat::from_yaw_pitch_roll(yaw, 0.0, 0.0)),
        collision,
    });
    scene
}

/// The collision world a level builds, floor and all.
fn world_of(scene: &EnvironmentScene) -> CollisionWorld {
    let mut world = CollisionWorld::new();
    world.add_ground(scene.ground_height, "FLOOR");
    engine_game::arena::add_terrain_collision(&mut world, scene);
    engine_game::arena::add_environment_collision(&mut world, scene);
    world.rebuild_broad_phase();
    world
}

/// Walks a character from `from` towards `to` and reports where it ended up.
///
/// At four metres a second in sixtieths of a second, for long enough to cover the distance
/// twice over. Twice, so that "it got there" and "it ran out of steps" are never the same
/// result — a character that is blocked has had ample time to prove it, and one that is not
/// arrives well before the end.
fn walk(world: &CollisionWorld, from: Vec3, to: Vec3) -> Vec3 {
    let config = CharacterConfig::default();
    let mut state = CharacterState::at(from);
    let mut buffers = QueryBuffers::new();
    let direction = (to - from).normalize();
    let step = 4.0 / 60.0;
    let steps = ((to - from).length() / step).ceil() as u32 * 2;

    for _ in 0..steps {
        // Gravity as well as the walk, so the character stays on the floor and a ceiling it
        // should not touch is actually tested against the top of its head.
        let displacement = direction * step + Vec3::new(0.0, -9.81 * (1.0 / 60.0) * 0.05, 0.0);
        move_character(world, &config, &mut state, displacement, &mut buffers);
    }
    state.position
}

#[test]
fn a_character_walks_under_the_cliff_overhang_and_out_the_other_side() {
    // The slab spans z from -5 to -1, 3.5 m up. The character walks along -Z beneath it.
    let scene = level_with("cliff_overhang", Vec3::ZERO, 0.0);
    let world = world_of(&scene);

    let start = Vec3::new(0.0, 0.0, -8.0);
    let ended = walk(&world, start, Vec3::new(0.0, 0.0, -1.5));

    assert!(
        ended.z > -3.0,
        "the character stopped at z {:.2}, short of the space under the slab",
        ended.z
    );
    assert!(
        ended.y < 1.0,
        "the character climbed to y {:.2} instead of walking under the overhang",
        ended.y
    );
}

#[test]
fn the_wall_behind_the_overhang_still_stops_the_character() {
    // The other half of the requirement: walkable underneath, and solid where the rock is.
    // An overhang a character walks *through* is not an overhang.
    let scene = level_with("cliff_overhang", Vec3::ZERO, 0.0);
    let world = world_of(&scene);

    let ended = walk(&world, Vec3::new(0.0, 0.0, -8.0), Vec3::new(0.0, 0.0, 4.0));
    assert!(
        ended.z < -0.5,
        "the character walked through the cliff to z {:.2}",
        ended.z
    );
}

#[test]
fn a_character_walks_the_length_of_a_cave_tunnel() {
    // The tunnel runs from z = 0 to z = -4, and its floor is half a metre up.
    let scene = level_with("cave_tunnel_straight", Vec3::ZERO, 0.0);
    let world = world_of(&scene);

    let start = Vec3::new(0.0, 0.5, 1.0);
    let ended = walk(&world, start, Vec3::new(0.0, 0.5, -6.0));

    assert!(
        ended.z < -4.0,
        "the character got to z {:.2} and did not come out of the far end",
        ended.z
    );
    assert!(
        ended.y < 1.5,
        "the character ended at y {:.2}, so it went over the tunnel rather than through it",
        ended.y
    );
}

#[test]
fn the_wall_of_a_tunnel_is_solid() {
    // Walking across the tunnel rather than along it: the wall stops the character.
    let scene = level_with("cave_tunnel_straight", Vec3::ZERO, 0.0);
    let world = world_of(&scene);

    let ended = walk(
        &world,
        Vec3::new(0.0, 0.5, -2.0),
        Vec3::new(8.0, 0.5, -2.0),
    );
    assert!(
        ended.x < 2.0,
        "the character walked out through the side of the tunnel to x {:.2}",
        ended.x
    );
}

#[test]
fn a_bounding_box_would_have_failed_the_walk_under() {
    // What the sidecar buys, stated as a test: the same piece with one box around all of it
    // is a wall. If this ever starts passing, compound collision has stopped being applied
    // and the tests above are passing for the wrong reason.
    let mut scene = EnvironmentScene::empty("bounding_box");
    scene.push(EnvironmentObject {
        asset: EnvironmentAsset::interned("generated/environment/cliffs/cliff_overhang"),
        transform: Transform::from_translation(Vec3::ZERO),
        collision: engine_environment::EnvironmentCollision::Box {
            center: Vec3::new(0.0, 3.0, -2.5),
            half_extents: Vec3::new(2.0, 3.0, 2.5),
        },
    });
    let world = world_of(&scene);

    let ended = walk(&world, Vec3::new(0.0, 0.0, -8.0), Vec3::new(0.0, 0.0, -1.5));
    assert!(
        ended.z < -4.5,
        "a single bounding box let the character walk under the overhang, at z {:.2}",
        ended.z
    );
}

#[test]
fn a_placed_piece_carries_its_shape_through_the_scene_file() {
    // The level is what the game loads, so the boxes have to survive being written and read.
    let scene = level_with("cave_tunnel_straight", Vec3::new(12.0, 0.0, -4.0), 0.0);
    let restored = engine_environment::scene_file::SceneDocument::from_scene(&scene)
        .to_scene("memory")
        .expect("the level is describable");

    assert_eq!(restored.collision_shapes, scene.collision_shapes);
    assert_eq!(
        restored.world_boxes(&restored.objects[0]).len(),
        scene.world_boxes(&scene.objects[0]).len(),
        "the placement lost boxes on the way through the file"
    );

    // And the world built from the reloaded level is the same world.
    let before = world_of(&scene).statics().count();
    let after = world_of(&restored).statics().count();
    assert_eq!(before, after);
}
