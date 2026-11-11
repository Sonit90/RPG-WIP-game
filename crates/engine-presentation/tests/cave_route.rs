//! A character walks a whole cave: in at the mouth, along a tunnel, through a junction, into a
//! room, and out the far end onto open ground.
//!
//! ```text
//!   outside ─► entrance ─► tunnel ─► junction ─► tunnel ─► room ─► tunnel ─► exit ─► outside
//!      o──►    ▓▓▓ ▓▓▓     ▓   ▓      ▓  ▓  ▓     ▓   ▓    ▓   ▓    ▓   ▓     ▓▓ ▓▓
//! ```
//!
//! This is the Phase 9 acceptance criterion, and it is the one a screenshot cannot check. Cave
//! pieces are hollow: their whole purpose is the space inside them, and the collision that
//! describes that space is authored beside the mesh rather than derived from it. Every way that
//! can go wrong — a bounding box filling the cavity, a doorway blocked by a wall the mesh does
//! not draw, a lip at the mouth taller than the controller can step — looks completely correct
//! until somebody tries to walk it.
//!
//! So the assertion is a character actually moving, through the real controller, in a world
//! built by the real level loader, over a level made only of the generated starter pieces.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_assets::procedural::starter::{MODULE, SHELL, starter_collision};
use engine_collision::controller::{CharacterConfig, CharacterState, move_character};
use engine_collision::world::{CollisionWorld, QueryBuffers};
use engine_core::math::{Quat, Vec3};
use engine_core::transform::Transform;
use engine_environment::{EnvironmentAsset, EnvironmentObject, EnvironmentScene};

/// The height a tunnel's floor sits at: the top of the shell every piece stands on.
const FLOOR: f32 = SHELL;

/// Adds one starter piece to `scene`, with the collision authored beside it.
fn place(scene: &mut EnvironmentScene, piece: &str, at: Vec3, yaw: f32) {
    let shape = starter_collision(piece).unwrap_or_else(|| panic!("{piece} is authored"));
    let collision = scene.intern_collision(shape);
    scene.push(EnvironmentObject {
        asset: EnvironmentAsset::interned(&format!("generated/environment/caves/{piece}")),
        transform: Transform::from_translation(at)
            .with_rotation(Quat::from_yaw_pitch_roll(yaw, 0.0, 0.0)),
        collision,
    });
}

/// The test cave, laid along -Z on the module grid.
///
/// Every piece connects at `z = 0` in its own space and reaches back to `z = -MODULE`, so a
/// run is a column of origins four metres apart. The junction and the room are centred rather
/// than end-anchored, which is why their origins sit in the middle of the cell they fill —
/// their doorways still land on the centre line, which is what has to line up.
///
/// ```text
///   z:  0        -4       -8      -12      -16          -29      -33      -37
///       entrance  tunnel   junction tunnel   room ...     tunnel   exit    open ground
/// ```
fn test_cave() -> EnvironmentScene {
    let mut scene = EnvironmentScene::empty("cave_route");
    place(&mut scene, "cave_entrance", Vec3::new(0.0, 0.0, 0.0), 0.0);
    place(
        &mut scene,
        "cave_tunnel_straight",
        Vec3::new(0.0, 0.0, -MODULE),
        0.0,
    );
    // The junction is centred on its origin, so it fills the cell from -8 to -12.
    place(&mut scene, "cave_junction", Vec3::new(0.0, 0.0, -10.0), 0.0);
    place(
        &mut scene,
        "cave_tunnel_straight",
        Vec3::new(0.0, 0.0, -12.0),
        0.0,
    );
    // The room is 13 m across the outside, so it is not module-sized; its doorways are on the
    // centre line, which is the thing that has to align.
    place(
        &mut scene,
        "cave_room_small",
        Vec3::new(0.0, 0.0, -22.5),
        0.0,
    );
    place(
        &mut scene,
        "cave_tunnel_straight",
        Vec3::new(0.0, 0.0, -29.0),
        0.0,
    );
    place(&mut scene, "cave_exit", Vec3::new(0.0, 0.0, -33.0), 0.0);
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

/// Walks a character from `from` towards `to`, reporting every position it passed through.
///
/// At four metres a second in sixtieths of a second, for long enough to cover the distance
/// twice over — so "it arrived" and "it ran out of steps" are never the same result.
fn walk_watching(
    world: &CollisionWorld,
    from: Vec3,
    to: Vec3,
    mut watch: impl FnMut(Vec3),
) -> Vec3 {
    let config = CharacterConfig::default();
    let mut state = CharacterState::at(from);
    // Standing, not falling into the level: the controller's step-up is only attempted for a
    // character that is on the ground, and a harness that starts airborne is a harness that
    // measures the first tick rather than the walk.
    state.grounded = true;
    let mut buffers = QueryBuffers::new();
    let direction = (to - from).normalize();
    let wanted = (to - from).length();
    let speed = 4.0;
    let seconds = 1.0 / 60.0;
    // Twice as many ticks as the distance needs, so "it arrived" and "it ran out of steps"
    // are never the same result — and a stop the moment it *has* arrived, so a character that
    // walks the route does not then keep walking out the far side of the level and report
    // where it ended up standing on open ground.
    let steps = (wanted / (speed * seconds)).ceil() as u32 * 2;

    for _ in 0..steps {
        if (state.position - from).dot(direction) >= wanted {
            break;
        }
        // Velocity rather than a bare displacement, which is how `engine-collision`'s own
        // movement tests drive the controller: it reads the velocity when it decides whether
        // a surface was landed on, and a character with none is one that never quite settles
        // onto a step it has just climbed.
        state.velocity.x = direction.x * speed;
        state.velocity.z = direction.z * speed;
        state.velocity.y = (state.velocity.y - 9.81 * seconds).max(-60.0);
        let displacement = state.velocity * seconds;
        move_character(world, &config, &mut state, displacement, &mut buffers);
        watch(state.position);
    }
    state.position
}

fn walk(world: &CollisionWorld, from: Vec3, to: Vec3) -> Vec3 {
    walk_watching(world, from, to, |_| {})
}

#[test]
fn a_character_walks_the_whole_cave_from_outside_to_outside() {
    let world = world_of(&test_cave());

    // Starting on open ground in front of the mouth, at ground level rather than at the
    // tunnel's floor height: entering is half the thing being tested.
    let start = Vec3::new(0.0, 0.0, 4.0);
    let mut lowest = f32::MAX;
    let mut highest = f32::MIN;
    let ended = walk_watching(&world, start, Vec3::new(0.0, 0.0, -42.0), |position| {
        lowest = lowest.min(position.y);
        highest = highest.max(position.y);
    });

    assert!(
        ended.z < -37.0,
        "the character got to z {:.2} and never came out of the far end",
        ended.z
    );
    // The floor held all the way. A character that dropped through it would show up here as
    // a large negative, and one that climbed over the pieces as a large positive.
    assert!(
        lowest > -0.5,
        "the character fell to y {lowest:.2} somewhere along the route"
    );
    assert!(
        highest < FLOOR + 1.0,
        "the character reached y {highest:.2}, so it went over the cave rather than through it"
    );
}

#[test]
fn a_character_can_turn_around_inside_the_cave_and_walk_back_out() {
    let world = world_of(&test_cave());

    // In to the room, and back out the way it came. A route that only works in one direction
    // is a route with a one-way lip in it.
    let inside = walk(&world, Vec3::new(0.0, 0.0, 4.0), Vec3::new(0.0, 0.0, -22.5));
    assert!(
        inside.z < -18.0,
        "the character only got to z {:.2} on the way in",
        inside.z
    );

    let back = walk(&world, inside, Vec3::new(0.0, FLOOR, 6.0));
    assert!(
        back.z > 1.0,
        "the character got stuck at z {:.2} on the way back out",
        back.z
    );
}

#[test]
fn a_character_crosses_a_room_from_side_to_side() {
    // A room a player can only cross along one axis is a corridor with a high ceiling. The
    // walk runs between the two side doorways, inside the room: the walls are 6.5 m out, so
    // reaching x = 6 means the far wall did not intrude into the space it encloses.
    let world = world_of(&test_cave());
    let ended = walk(
        &world,
        Vec3::new(-5.0, FLOOR, -22.5),
        Vec3::new(5.0, FLOOR, -22.5),
    );
    assert!(
        ended.x > 4.0,
        "the character crossed to x {:.2} and was stopped inside the room",
        ended.x
    );
    assert!(
        (ended.y - FLOOR).abs() < 0.3,
        "the character left the room's floor, at y {:.2}",
        ended.y
    );
}

#[test]
fn a_tunnel_meets_a_room_on_its_side_doorway_too() {
    // The room's doorways are on all four sides, and a cave that can only be entered along
    // one axis is a cave whose rooms are corridors. A turned tunnel is laid against the west
    // doorway and walked out of, into the room.
    let mut scene = EnvironmentScene::empty("room_side");
    place(&mut scene, "cave_room_small", Vec3::ZERO, 0.0);
    // A quarter turn puts the tunnel's own -Z along -X, so it reaches away from the room
    // from the doorway it is standing in.
    place(
        &mut scene,
        "cave_tunnel_straight",
        Vec3::new(-6.5, 0.0, 0.0),
        std::f32::consts::FRAC_PI_2,
    );
    let world = world_of(&scene);

    let ended = walk(
        &world,
        Vec3::new(-9.5, FLOOR, 0.0),
        Vec3::new(4.0, FLOOR, 0.0),
    );
    assert!(
        ended.x > 2.0,
        "the character got to x {:.2} and did not come out of the tunnel into the room",
        ended.x
    );
}

#[test]
fn a_junction_is_open_on_all_four_sides() {
    // A crossing that is only a crossing along one axis is a length of tunnel.
    let mut scene = EnvironmentScene::empty("junction");
    place(&mut scene, "cave_junction", Vec3::ZERO, 0.0);
    let world = world_of(&scene);

    let along_z = walk(
        &world,
        Vec3::new(0.0, FLOOR, 3.0),
        Vec3::new(0.0, FLOOR, -3.0),
    );
    assert!(along_z.z < -2.0, "blocked along Z at {:.2}", along_z.z);

    let along_x = walk(
        &world,
        Vec3::new(-3.0, FLOOR, 0.0),
        Vec3::new(3.0, FLOOR, 0.0),
    );
    assert!(along_x.x > 2.0, "blocked along X at {:.2}", along_x.x);

    // And the corners are solid: a crossing with no corners is an open square.
    let corner = walk(
        &world,
        Vec3::new(-3.0, FLOOR, -3.0),
        Vec3::new(3.0, FLOOR, 3.0),
    );
    assert!(
        corner.x < 1.5 || corner.z < 1.5,
        "the character cut the corner of the junction to {corner:?}"
    );
}

#[test]
fn a_character_walks_around_a_corner_through_the_curve() {
    // The curve arrives along -Z and leaves along +X, so the route turns without the player
    // having to find a doorway they cannot see.
    let mut scene = EnvironmentScene::empty("curve");
    place(&mut scene, "cave_tunnel_curve", Vec3::ZERO, 0.0);
    let world = world_of(&scene);

    let ended = walk(
        &world,
        Vec3::new(0.0, FLOOR, -0.5),
        Vec3::new(0.0, FLOOR, -3.0),
    );
    assert!(ended.z < -2.0, "the character stopped short at {ended:?}");

    let out = walk(&world, ended, Vec3::new(6.0, FLOOR, ended.z));
    assert!(
        out.x > 3.0,
        "the character did not get round the corner: {out:?}"
    );
}

#[test]
fn the_mouth_is_low_enough_to_walk_into() {
    // The bug this piece is shaped around: a tunnel's floor is half a metre up, the controller
    // steps 0.45, and a mouth without a doorstep is a cave that can be seen into and not
    // walked into. It looks completely right in a screenshot.
    let mut scene = EnvironmentScene::empty("mouth");
    place(&mut scene, "cave_entrance", Vec3::ZERO, 0.0);
    let world = world_of(&scene);

    // Stopping half way along the piece: the tunnel behind the mouth ends at z = -4, and a
    // walk aimed past it would come out of the back and land on open ground, which says
    // nothing about the mouth.
    let ended = walk(&world, Vec3::new(0.0, 0.0, 4.0), Vec3::new(0.0, 0.0, -0.5));
    assert!(
        ended.z < -0.4,
        "the character stopped at z {:.2}, outside a cave it can see into",
        ended.z
    );
    assert!(
        (ended.y - FLOOR).abs() < 0.3,
        "the character ended at y {:.2} rather than up on the tunnel floor",
        ended.y
    );
}

#[test]
fn the_cave_survives_being_written_and_read_back() {
    // A cave is only real if the level is. The boxes are interned in the scene and named by
    // index, so this is also the assertion that the shape table round-trips.
    let scene = test_cave();
    let restored = engine_environment::scene_file::SceneDocument::from_scene(&scene)
        .to_scene("memory")
        .expect("the cave is describable");

    assert_eq!(restored.collision_shapes, scene.collision_shapes);
    assert_eq!(restored.objects.len(), scene.objects.len());

    let world = world_of(&restored);
    let ended = walk(&world, Vec3::new(0.0, 0.0, 4.0), Vec3::new(0.0, 0.0, -42.0));
    assert!(
        ended.z < -37.0,
        "the reloaded cave stopped the character at z {:.2}",
        ended.z
    );
}
