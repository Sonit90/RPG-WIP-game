//! A character walks straight through a body of water, and the level agrees about when it is
//! under one.
//!
//! ```text
//!            surface
//!   ~~~~~~~~~~~~~~~~~~~~~~~~
//!   │                      │
//!   │   o───────────────►  │   the character walks in one side and out the other
//!   └──────────────────────┘
//!  ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓  the floor, which is the only thing holding it up
//! ```
//!
//! Water adds nothing to the collision world — see `engine_environment::water` — and this is
//! the assertion that says so in the only way that cannot be satisfied by accident: by moving
//! a character through the real controller, in a world built by the real level loader, rather
//! than by counting the shapes the world came out with. A body that quietly became a box would
//! pass a count and fail this.
//!
//! The second half is the pair of questions being consistent: where the character is standing,
//! and whether the level says that place is under water. A query that disagreed with where the
//! character actually is would be a query gameplay could not build a death trigger on.
//!
//! It lives here because it needs two crates that cannot see each other: the level
//! (`engine-environment`) and the collision world and controller (`engine-collision`, through
//! `engine-game`'s loader).

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_collision::controller::{CharacterConfig, CharacterState, move_character};
use engine_collision::world::{CollisionWorld, QueryBuffers};
use engine_core::math::{Vec2, Vec3};
use engine_environment::water::WaterBody;
use engine_environment::{
    EnvironmentAsset, EnvironmentCollision, EnvironmentObject, EnvironmentScene,
};

/// A level whose middle is a pond: twelve metres across, its surface a metre up, two deep.
///
/// The surface above the floor and the bottom below it, so a character standing on the ground
/// inside the rectangle is inside the volume — which is the case gameplay will ask about. A
/// pond whose surface sat exactly on the floor would be one no character standing on that
/// floor was ever quite inside.
fn level_with_a_pond() -> EnvironmentScene {
    let mut scene = EnvironmentScene::empty("water_test");
    scene.push_water(WaterBody::new(
        Vec3::new(0.0, 1.0, 0.0),
        Vec2::new(6.0, 6.0),
        2.0,
    ));
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

/// Walks a character from `from` towards `to`, calling `watch` with every position it passed
/// through, and reports where it ended up.
fn walk(
    world: &CollisionWorld,
    from: Vec3,
    to: Vec3,
    mut watch: impl FnMut(Vec3),
) -> Vec3 {
    let config = CharacterConfig::default();
    let mut state = CharacterState::at(from);
    let mut buffers = QueryBuffers::new();
    let direction = (to - from).normalize();
    let step = 4.0 / 60.0;
    let steps = ((to - from).length() / step).ceil() as u32 * 2;

    for _ in 0..steps {
        // Gravity as well as the walk, so the character stays on the floor: a character
        // hovering over the pond would be a test that never entered it.
        let displacement = direction * step + Vec3::new(0.0, -9.81 * (1.0 / 60.0) * 0.05, 0.0);
        move_character(world, &config, &mut state, displacement, &mut buffers);
        watch(state.position);
    }
    state.position
}

#[test]
fn a_character_walks_straight_through_a_body_of_water() {
    let scene = level_with_a_pond();
    let world = world_of(&scene);

    let start = Vec3::new(0.0, 0.0, 12.0);
    let target = Vec3::new(0.0, 0.0, -12.0);
    let ended = walk(&world, start, target, |_| {});

    assert!(
        ended.z < -10.0,
        "the character stopped at z {:.2}, so the water was a wall",
        ended.z
    );
    assert!(
        ended.y.abs() < 0.5,
        "the character ended at y {:.2}, so the water was a floor or a ceiling",
        ended.y
    );
}

#[test]
fn the_level_says_the_character_is_in_the_water_exactly_while_it_is() {
    let scene = level_with_a_pond();
    let world = world_of(&scene);

    // Every position the walk passed through, with what the level said about it.
    let mut samples: Vec<(Vec3, bool)> = Vec::new();
    walk(
        &world,
        Vec3::new(0.0, 0.0, 12.0),
        Vec3::new(0.0, 0.0, -12.0),
        |position| samples.push((position, scene.is_submerged(position))),
    );

    let inside = |position: Vec3| position.z.abs() <= 5.5;
    let outside = |position: Vec3| position.z.abs() >= 6.5;

    // Well inside the rectangle the answer is yes, and well outside it is no. The half-metre
    // margin is where the rim is, and a sample landing exactly on it is not a disagreement.
    let submerged_inside = samples
        .iter()
        .filter(|(position, _)| inside(*position))
        .count();
    assert!(submerged_inside > 0, "the walk never crossed the pond");
    for (position, submerged) in &samples {
        if inside(*position) {
            assert!(
                *submerged,
                "standing at {position:?}, inside the pond, and the level says dry"
            );
        }
        if outside(*position) {
            assert!(
                !*submerged,
                "standing at {position:?}, well outside the pond, and the level says wet"
            );
        }
    }
}

#[test]
fn a_placement_inside_the_pond_still_blocks_the_character() {
    // The other half of the claim: water adds no collision, and it does not take any away
    // either. A rock standing in a pond is a rock.
    let mut scene = level_with_a_pond();
    scene.push(
        EnvironmentObject::decoration(
            EnvironmentAsset::interned("generated/environment/rock_large"),
            Vec3::ZERO,
        )
        .with_collision(EnvironmentCollision::standing(1.5, 2.0, 1.5)),
    );
    let world = world_of(&scene);

    let ended = walk(
        &world,
        Vec3::new(0.0, 0.0, 12.0),
        Vec3::new(0.0, 0.0, -12.0),
        |_| {},
    );
    assert!(
        ended.z > -2.0,
        "the character walked through the rock in the pond to z {:.2}",
        ended.z
    );
}
