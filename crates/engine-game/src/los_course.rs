//! The line-of-sight course: the fight that stands in [`engine_environment::los_course`].
//!
//! ```text
//!   engine_environment::los_course      engine_game::los_course
//!   ------------------------------      -----------------------
//!   three domes, r = 7, h = 3.15  -->   a tiled surface each, in the collision world
//!   six dummy stands              -->   six dummies, standing on the surface
//!   two pits and their ramps      -->   a floor with holes in it, and a way back out
//!   the expected answer per stand -->   what a test asserts and the overlay prints
//! ```
//!
//! # What it is for
//!
//! One place to stand and one thing to try: target a dummy on the near slope of a dome, one
//! on its crest, and one on its far slope, and see which of the three the ability refuses.
//! The course states the expected answer for each of the six ([`engine_environment::
//! los_course::LineOfSight`]), so the test below is the same claim the player is checking by
//! hand rather than a second opinion about it.
//!
//! The pits ask the same question from below: a caster three and a half metres down has its
//! chest under the lip, and the floor is on the `WORLD` layer like every other blocker.
//!
//! # Why the domes are boxes
//!
//! `engine-collision` has a plane, a box and an oriented box. A dome is none of them, and
//! one test course is not a reason to grow the narrow phase — so the surface is tiled with
//! *tilted* boxes, five bands of them following the profile the mesh is built from, added
//! with `add_ramp` because that is what each one is. The dummies stand on the analytic
//! surface; the tiles are within ten centimetres of it.
//!
//! Tilted rather than upright, because the course is walked as well as looked at. Upright
//! boxes as tall as the surface above them block a ray perfectly well and are a staircase to
//! stand on — at this radius, steps of over a metre against a character that can climb 0.45,
//! which is a hill that can only be looked at. `a_player_can_walk_up_a_dome_and_stand_on_its_crest`
//! is the test that says this is not that.

use engine_collision::shape::Obb;
use engine_collision::world::CollisionWorld;
use engine_core::math::{Quat, Vec3};
use engine_environment::los_course::{self, LineOfSight};

use crate::arena::{self, ArenaConfig};
use crate::entity::EntityId;
use crate::movement::MovementConfig;
use crate::sim::{SimConfig, Simulation};
use crate::world::{EntityDesc, GameWorld};

/// Height of the course's boundary wall, in metres. Taller than a jump.
const WALL_HEIGHT: f32 = 4.0;

/// Thickness of the course's boundary wall, in metres.
const WALL_THICKNESS: f32 = 1.0;

/// One dummy on the course, and what the course expects of it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CourseDummy {
    /// The entity that was spawned.
    pub entity: EntityId,
    /// Which dome it stands on: `"LEFT DOME"`, `"RIGHT DOME"`.
    pub dome: &'static str,
    /// Where on that dome: `"NEAR SLOPE"`, `"CREST"`, `"FAR SLOPE"`.
    pub name: &'static str,
    /// What a cast from the player's spawn should be told.
    pub expected: LineOfSight,
}

/// Everything the course spawned, so a caller can name the parts without searching.
#[derive(Debug, Clone)]
pub struct LosCourse {
    /// The player's entity.
    pub player: EntityId,
    /// The six dummies, left dome first, near slope first.
    pub dummies: Vec<CourseDummy>,
}

impl LosCourse {
    /// The dummy standing at `name` on `dome`, if there is one.
    #[must_use]
    pub fn stand(&self, dome: &str, name: &str) -> Option<CourseDummy> {
        self.dummies
            .iter()
            .copied()
            .find(|dummy| dummy.dome == dome && dummy.name == name)
    }
}

/// Builds the line-of-sight course and the fight standing in it.
///
/// The same shape as [`arena::environment_arena`] — a scene supplies the layout and
/// gameplay puts entities on it — with two differences the course needs: its floor has holes
/// in it, so it is slabs rather than a plane, and its dummies stand on top of something, so
/// they carry their own support height.
#[must_use]
pub fn los_course_arena() -> (Simulation, LosCourse) {
    let scene = los_course::los_course();
    let health = ArenaConfig::default().dummy_health;

    let mut simulation = Simulation::new(SimConfig {
        movement: MovementConfig {
            arena_half_extent: los_course::COURSE_HALF_EXTENT,
            ..MovementConfig::default()
        },
        ..SimConfig::default()
    });

    let course = populate(simulation.world_mut(), health);

    let world = simulation.collision_world_mut();
    build_collision(world);
    let statics = arena::add_environment_collision(world, &scene);
    world.rebuild_broad_phase();
    log::info!(
        "line-of-sight course: {} objects, {statics} scene shapes, {} dummies",
        scene.len(),
        course.dummies.len()
    );

    simulation.set_player_entity(course.player);
    simulation.select_nearest_target();
    (simulation, course)
}

/// Spawns the player and the six dummies into an existing world.
///
/// A dummy is put down at the height of the *drawn* dome surface and told that is its floor,
/// because [`crate::movement::step`] — the integrator a body that never moves goes through —
/// has no collision world and would otherwise drop it to y = 0 and leave it inside the hill.
pub fn populate(world: &mut GameWorld, dummy_health: f32) -> LosCourse {
    world.clear();

    let player = world.spawn(EntityDesc::player(los_course::PLAYER_SPAWN));
    let mut dummies = Vec::with_capacity(6);
    for stand in los_course::dummy_stands() {
        let entity = world.spawn(EntityDesc::dummy(stand.position, dummy_health));
        if let Some(body) = world.body_mut(entity) {
            body.support_height = stand.position.y;
        }
        dummies.push(CourseDummy {
            entity,
            dome: stand.dome,
            name: stand.name,
            expected: stand.expected,
        });
    }

    LosCourse { player, dummies }
}

/// Gives the course its floor, its pits, its domes and its boundary wall.
///
/// The order is the order a player meets them: the catch floor at the bottom of the pits
/// first, then the slabs that make up the floor above it, the ramps out, the domes, and the
/// wall around the whole thing.
pub fn build_collision(world: &mut CollisionWorld) {
    world.clear();

    // The bottom of both pits, and the one thing under the floor slabs. A plane rather than
    // two boxes because it is also the backstop: nothing that gets under the floor can then
    // keep going.
    world.add_ground(-los_course::PIT_DEPTH, "PIT FLOOR");

    for slab in los_course::floor_slabs() {
        world.add_box(slab.min, slab.max, slab.label);
    }

    for pit in los_course::pits() {
        let ramp = pit.ramp();
        world.add_ramp(
            Obb::ramp(ramp.center, ramp.half_extents, Vec3::X, ramp.pitch),
            ramp.label,
        );
    }

    // The domes, as bands of tilted boxes following the profile the mesh is built from.
    // `add_ramp` rather than `add_box`, because that is what they are: a character walks up
    // a dome the same way it walks up the collision course's slopes.
    for dome in los_course::domes() {
        for tile in dome.collision() {
            world.add_ramp(
                Obb::new(
                    tile.center,
                    tile.half_extents,
                    Quat::from_yaw_pitch_roll(tile.yaw, tile.pitch, 0.0),
                ),
                tile.label,
            );
        }
    }

    let limit = los_course::COURSE_HALF_EXTENT;
    let outer = limit + WALL_THICKNESS;
    let bottom = -los_course::PIT_DEPTH;
    // The same overlapping square the arena's boundary is, so the four walls meet at the
    // corners instead of leaving a gap the width of a wall at each one.
    world.add_box(
        Vec3::new(limit, bottom, -outer),
        Vec3::new(outer, WALL_HEIGHT, outer),
        "COURSE WALL +X",
    );
    world.add_box(
        Vec3::new(-outer, bottom, -outer),
        Vec3::new(-limit, WALL_HEIGHT, outer),
        "COURSE WALL -X",
    );
    world.add_box(
        Vec3::new(-outer, bottom, limit),
        Vec3::new(outer, WALL_HEIGHT, outer),
        "COURSE WALL +Z",
    );
    world.add_box(
        Vec3::new(-outer, bottom, -outer),
        Vec3::new(outer, WALL_HEIGHT, -limit),
        "COURSE WALL -Z",
    );
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::los;

    /// The check the ability path runs, from the player to one dummy.
    fn visible(simulation: &Simulation, course: &LosCourse, dummy: CourseDummy) -> bool {
        los::has_line_of_sight(
            simulation.collision_world(),
            simulation.world(),
            course.player,
            dummy.entity,
        )
    }

    #[test]
    fn the_course_answers_what_it_says_it_will_from_the_player_spawn() {
        let (simulation, course) = los_course_arena();
        assert_eq!(course.dummies.len(), 6);
        for dummy in &course.dummies {
            assert_eq!(
                visible(&simulation, &course, *dummy),
                dummy.expected.is_visible(),
                "{} {} should be {}",
                dummy.dome,
                dummy.name,
                dummy.expected.as_str()
            );
        }
    }

    #[test]
    fn both_dome_sides_answer_the_same_way_so_the_course_is_symmetric() {
        let (simulation, course) = los_course_arena();
        for name in ["NEAR SLOPE", "CREST", "FAR SLOPE"] {
            let left = course.stand("LEFT DOME", name).expect("a left stand");
            let right = course.stand("RIGHT DOME", name).expect("a right stand");
            assert_eq!(
                visible(&simulation, &course, left),
                visible(&simulation, &course, right),
                "the two domes disagree about {name}"
            );
        }
    }

    #[test]
    fn the_far_slope_is_blocked_by_the_dome_it_stands_on_and_not_by_something_else() {
        let (simulation, course) = los_course_arena();
        let far = course.stand("LEFT DOME", "FAR SLOPE").expect("a stand");
        let check = los::line_of_sight(
            simulation.collision_world(),
            simulation.world(),
            course.player,
            far.entity,
        )
        .expect("both entities have a position");
        assert!(!check.visible);
        assert_eq!(check.blocker_label, "LEFT DOME");
    }

    #[test]
    fn a_caster_on_the_floor_of_a_pit_cannot_see_out_of_it() {
        let (mut simulation, course) = los_course_arena();
        let pit = engine_environment::los_course::pits()[0];
        let (_, _, z_min, _) = pit.bounds();
        // The -Z end of the bottom: the ramp takes up the +Z half of the shaft, and the
        // further from the -Z lip a caster stands the more of the course it can see over
        // it — walking a pit from end to end is exactly the thing the pit is there for.
        // Two spots rather than one, because "cannot see out" is a claim about standing in
        // the hole and not about one lucky position in it.
        for z in [z_min + 0.4, z_min + 1.2] {
            let standing = Vec3::new(pit.center.x, pit.center.y - pit.depth, z);
            // Put the player down rather than walking there: where the player stands is the
            // input to this test, and walking is a different one.
            if let Some(body) = simulation.world_mut().body_mut(course.player) {
                body.position = standing;
                body.grounded = true;
            }
            for dummy in &course.dummies {
                assert!(
                    !visible(&simulation, &course, *dummy),
                    "{} {} is visible from the floor of {} at z = {z}",
                    dummy.dome,
                    dummy.name,
                    pit.label
                );
            }
        }
    }

    #[test]
    fn what_stops_the_ray_out_of_a_pit_is_the_floor_the_player_is_under() {
        let (mut simulation, course) = los_course_arena();
        let pit = engine_environment::los_course::pits()[0];
        let (_, _, z_min, _) = pit.bounds();
        if let Some(body) = simulation.world_mut().body_mut(course.player) {
            body.position = Vec3::new(pit.center.x, pit.center.y - pit.depth, z_min + 0.8);
        }
        let crest = course.stand("LEFT DOME", "CREST").expect("a stand");
        let check = los::line_of_sight(
            simulation.collision_world(),
            simulation.world(),
            course.player,
            crest.entity,
        )
        .expect("both entities have a position");
        assert!(!check.visible);
        assert_eq!(check.blocker_label, "COURSE FLOOR");
    }

    #[test]
    fn the_player_stands_on_the_floor_and_the_dummies_stay_on_their_domes() {
        let (mut simulation, course) = los_course_arena();
        let heights: Vec<f32> = course
            .dummies
            .iter()
            .map(|dummy| {
                simulation
                    .world()
                    .body(dummy.entity)
                    .expect("a dummy has a body")
                    .position
                    .y
            })
            .collect();

        simulation.step_many(120);

        for (dummy, height) in course.dummies.iter().zip(heights) {
            let body = simulation.world().body(dummy.entity).expect("a body");
            assert!(
                (body.position.y - height).abs() < 1e-3,
                "{} {} slid from {height} to {}",
                dummy.dome,
                dummy.name,
                body.position.y
            );
        }
        let player = simulation.world().body(course.player).expect("a body");
        assert!(
            player.position.y.abs() < 0.05,
            "the player settled at y = {}",
            player.position.y
        );
    }

    #[test]
    fn a_player_who_walks_into_a_pit_can_walk_back_out_of_it() {
        let (mut simulation, course) = los_course_arena();
        let pit = engine_environment::los_course::pits()[0];
        if let Some(body) = simulation.world_mut().body_mut(course.player) {
            body.position = pit.floor();
        }
        simulation.step_many(30);
        let sunk = simulation
            .world()
            .body(course.player)
            .expect("a body")
            .position;
        assert!(
            sunk.y < -1.0 && sunk.y > -engine_environment::los_course::PIT_DEPTH - 0.1,
            "the player should be down the pit, and is at {}",
            sunk.y
        );

        // Up the ramp: it climbs towards +Z, which is the way the player came in. The
        // camera's forward at yaw zero is -Z, so walking +Z is walking backwards.
        simulation.push_command(crate::command::PlayerCommand::Move {
            forward: -1.0,
            right: 0.0,
            yaw: 0.0,
            face_camera: false,
        });
        simulation.step_many(240);
        let out = simulation
            .world()
            .body(course.player)
            .expect("a body")
            .position;
        assert!(
            out.y > -0.1,
            "the player is still {} m down after walking up the ramp",
            -out.y
        );
    }

    #[test]
    fn a_player_can_walk_up_a_dome_and_stand_on_its_crest() {
        let (mut simulation, course) = los_course_arena();
        let dome = engine_environment::los_course::domes()[1];

        // Start at the rim of the middle dome, on the player's side of it, and walk at the
        // crest. The dome is the reason the course exists to be walked on: if the surface
        // were the field of upright boxes it used to be, this walk would stop at the first
        // step and the assertion below would fail by three metres.
        let start = Vec3::new(dome.center.x, 0.0, dome.center.z + dome.radius + 1.0);
        if let Some(body) = simulation.world_mut().body_mut(course.player) {
            body.position = start;
            body.grounded = true;
        }
        // Walking towards -Z is the camera's forward at yaw zero.
        let walk = |simulation: &mut Simulation| {
            simulation.push_command(crate::command::PlayerCommand::Move {
                forward: 1.0,
                right: 0.0,
                yaw: 0.0,
                face_camera: false,
            });
        };

        let mut highest = 0.0_f32;
        let mut reached_crest = false;
        for _ in 0..120 {
            walk(&mut simulation);
            simulation.step_once();
            let body = simulation.world().body(course.player).expect("a body");
            highest = highest.max(body.position.y);
            // Past the middle of the dome is as far as the climb goes; beyond it the walk
            // is a descent, and a test that keeps walking measures the far rim.
            if body.position.z <= dome.center.z {
                reached_crest = true;
                break;
            }
            // At every step of the way, the feet are on the surface the player can see —
            // that is the difference between a hill and a staircase with a lip on it.
            let surface = dome.surface_height(body.position.x, body.position.z);
            assert!(
                (body.position.y - surface).abs() < 0.35,
                "on the way up, the player is at {:.2} and the surface is at {surface:.2}",
                body.position.y
            );
        }

        assert!(reached_crest, "the player never got over the middle");
        assert!(
            highest > dome.height() - 0.4,
            "the player got {highest:.2} m up a {:.2} m hill",
            dome.height()
        );

        // And standing still on it, rather than sliding back down: the surface is inside the
        // controller's slope limit everywhere. Movement intent is held between commands, so
        // letting go means saying so.
        simulation.push_command(crate::command::PlayerCommand::Move {
            forward: 0.0,
            right: 0.0,
            yaw: 0.0,
            face_camera: false,
        });
        simulation.step_many(30);
        let settled = simulation
            .world()
            .body(course.player)
            .expect("a body")
            .position;
        simulation.step_many(60);
        let body = simulation.world().body(course.player).expect("a body");
        assert!(
            (body.position - settled).length() < 0.5,
            "the player slid from {settled:?} to {:?}",
            body.position
        );
        assert!(body.grounded, "the player is not standing on the dome");
    }
}
