//! Character movement, driven the way a game drives it.
//!
//! Every test here is a story: put a character somewhere, feed it a velocity for a number
//! of fixed ticks, and assert where it ended up. Nothing reaches inside the controller —
//! this is the same API `engine-game` uses, at the same 60 Hz.
#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_collision::prelude::*;
use engine_core::math::Vec3;

/// The simulation's fixed step.
const STEP: f32 = 1.0 / 60.0;

/// Gravity, matching the game's tuning.
const GRAVITY: f32 = 25.0;

/// Jump speed, matching the game's tuning.
const JUMP_SPEED: f32 = 8.0;

/// A character, its world and its scratch space, driven together.
struct Harness {
    world: CollisionWorld,
    config: CharacterConfig,
    state: CharacterState,
    buffers: QueryBuffers,
    last: MoveResult,
    ever_airborne: bool,
    ever_hit_wall: bool,
    highest: f32,
    lowest: f32,
}

impl Harness {
    /// A character standing at `position` in `world`.
    fn new(world: CollisionWorld, position: Vec3) -> Self {
        let mut state = CharacterState::at(position);
        state.grounded = true;
        Self {
            world,
            config: CharacterConfig::default(),
            state,
            buffers: QueryBuffers::new(),
            last: MoveResult::default(),
            ever_airborne: false,
            ever_hit_wall: false,
            highest: position.y,
            lowest: position.y,
        }
    }

    /// Runs `ticks` of walking in `direction` (metres per second) under gravity.
    fn walk(&mut self, direction: Vec3, ticks: u32) -> &mut Self {
        for _ in 0..ticks {
            self.state.velocity.x = direction.x;
            self.state.velocity.z = direction.z;
            self.state.velocity.y = (self.state.velocity.y - GRAVITY * STEP).max(-60.0);
            self.tick();
        }
        self
    }

    /// Runs `ticks` with no horizontal input.
    fn fall(&mut self, ticks: u32) -> &mut Self {
        self.walk(Vec3::ZERO, ticks)
    }

    /// Jumps, if the character is standing on something.
    fn jump(&mut self) -> bool {
        if !self.state.grounded {
            return false;
        }
        self.state.velocity.y = JUMP_SPEED;
        self.state.grounded = false;
        true
    }

    /// One tick of whatever velocity the character currently has.
    fn tick(&mut self) {
        let displacement = self.state.velocity * STEP;
        self.last = move_character(
            &self.world,
            &self.config,
            &mut self.state,
            displacement,
            &mut self.buffers,
        );
        self.ever_airborne |= !self.state.grounded;
        self.ever_hit_wall |= self.last.hit_wall;
        self.highest = self.highest.max(self.state.position.y);
        self.lowest = self.lowest.min(self.state.position.y);
    }

    /// Where the character is.
    fn position(&self) -> Vec3 {
        self.state.position
    }
}

/// A world with an infinite floor at y = 0.
fn floor_world() -> CollisionWorld {
    let mut world = CollisionWorld::new();
    world.add_ground(0.0, "FLOOR");
    world.rebuild_broad_phase();
    world
}

/// A world with a floor and a wall across +X at `x`.
fn wall_world(x: f32) -> CollisionWorld {
    let mut world = CollisionWorld::new();
    world.add_ground(0.0, "FLOOR");
    world.add_box(
        Vec3::new(x, 0.0, -20.0),
        Vec3::new(x + 1.0, 4.0, 20.0),
        "WALL",
    );
    world.rebuild_broad_phase();
    world
}

/// A ramp rising along +X from the origin, plus a floor.
fn ramp_world(degrees: f32) -> CollisionWorld {
    let mut world = CollisionWorld::new();
    world.add_ground(0.0, "FLOOR");
    let radians = degrees.to_radians();
    world.add_ramp(
        Obb::ramp(
            Vec3::new(6.0 * radians.cos(), 6.0 * radians.sin(), 0.0),
            Vec3::new(6.0, 0.25, 5.0),
            Vec3::Z,
            radians,
        ),
        "RAMP",
    );
    world.rebuild_broad_phase();
    world
}

// --- Floor ------------------------------------------------------------------------------

#[test]
fn a_character_stands_on_the_floor_and_stays_there() {
    let mut harness = Harness::new(floor_world(), Vec3::new(0.0, 0.0, 0.0));
    harness.fall(300);

    assert!(harness.state.grounded);
    assert!(
        harness.position().y.abs() < 0.02,
        "{:?}",
        harness.position()
    );
    assert!(!harness.ever_airborne, "it fell off a flat floor");
    assert!(harness.state.ground_normal.dot(Vec3::Y) > 0.99);
}

#[test]
fn walking_on_a_flat_floor_goes_where_it_was_pointed() {
    let mut harness = Harness::new(floor_world(), Vec3::ZERO);
    harness.walk(Vec3::new(0.0, 0.0, -7.0), 60);

    assert!(harness.position().z < -6.0, "{:?}", harness.position());
    assert!(harness.position().x.abs() < 1e-3, "it drifted sideways");
    assert!(harness.state.grounded);
}

// --- Walls, sliding and corners ---------------------------------------------------------

#[test]
fn walking_straight_into_a_wall_stops_at_it() {
    let mut harness = Harness::new(wall_world(5.0), Vec3::ZERO);
    harness.walk(Vec3::new(7.0, 0.0, 0.0), 180);

    let radius = harness.config.radius;
    assert!(
        harness.position().x <= 5.0 - radius + 0.02,
        "it entered the wall: {:?}",
        harness.position()
    );
    assert!(harness.position().x > 4.0, "it stopped too early");
    assert!(harness.ever_hit_wall, "the wall was never reported");
    assert!(
        harness.state.velocity.x.abs() < 1e-3,
        "velocity into the wall was not clipped: {:?}",
        harness.state.velocity
    );
}

#[test]
fn walking_diagonally_into_a_wall_slides_along_it() {
    let mut harness = Harness::new(wall_world(5.0), Vec3::ZERO);
    harness.walk(Vec3::new(5.0, 0.0, -5.0), 120);

    assert!(harness.position().x < 5.0, "it passed the wall");
    assert!(
        harness.position().z < -6.0,
        "it did not slide: {:?}",
        harness.position()
    );
}

#[test]
fn sliding_covers_nearly_as_much_ground_as_walking_free() {
    // The property a slide exists for: pressing into a wall while running along it must not
    // cost most of the speed.
    let mut free = Harness::new(floor_world(), Vec3::ZERO);
    free.walk(Vec3::new(0.0, 0.0, -5.0), 120);

    let mut sliding = Harness::new(wall_world(2.0), Vec3::ZERO);
    sliding.walk(Vec3::new(5.0, 0.0, -5.0), 120);

    let ratio = sliding.position().z / free.position().z;
    assert!(
        ratio > 0.9,
        "sliding kept only {:.0}% of the distance",
        ratio * 100.0
    );
}

#[test]
fn an_inside_corner_holds_the_character_without_trapping_or_ejecting_it() {
    let mut world = CollisionWorld::new();
    world.add_ground(0.0, "FLOOR");
    world.add_box(
        Vec3::new(5.0, 0.0, -20.0),
        Vec3::new(6.0, 4.0, 5.0),
        "WALL X",
    );
    world.add_box(
        Vec3::new(-20.0, 0.0, 5.0),
        Vec3::new(6.0, 4.0, 6.0),
        "WALL Z",
    );
    world.rebuild_broad_phase();

    let mut harness = Harness::new(world, Vec3::ZERO);
    harness.walk(Vec3::new(6.0, 0.0, 6.0), 240);

    let corner = harness.position();
    assert!(
        corner.x < 5.0 && corner.z < 5.0,
        "it entered a wall: {corner:?}"
    );
    assert!(
        corner.x > 4.0 && corner.z > 4.0,
        "it was ejected: {corner:?}"
    );

    harness.walk(Vec3::new(-6.0, 0.0, -6.0), 120);
    assert!(
        harness.position().x < 0.0 && harness.position().z < 0.0,
        "it could not leave the corner: {:?}",
        harness.position()
    );
}

#[test]
fn a_narrow_corridor_can_be_walked_down() {
    // Two walls a little over two radii apart. A controller that pushes out of both at once
    // squeezes the character out of the end of the corridor like a pip.
    let mut world = CollisionWorld::new();
    world.add_ground(0.0, "FLOOR");
    world.add_box(
        Vec3::new(-1.9, 0.0, -20.0),
        Vec3::new(-0.9, 4.0, 20.0),
        "LEFT",
    );
    world.add_box(
        Vec3::new(0.9, 0.0, -20.0),
        Vec3::new(1.9, 4.0, 20.0),
        "RIGHT",
    );
    world.rebuild_broad_phase();

    let mut harness = Harness::new(world, Vec3::ZERO);
    harness.walk(Vec3::new(0.0, 0.0, -6.0), 120);

    assert!(
        harness.position().z < -8.0,
        "it did not get down the corridor: {:?}",
        harness.position()
    );
    assert!(
        harness.position().x.abs() < 0.6,
        "it was squeezed out sideways: {:?}",
        harness.position()
    );
}

// --- Gravity, ledges and landing --------------------------------------------------------

#[test]
fn a_character_falls_when_it_runs_out_of_floor_and_lands_on_what_is_below() {
    let mut world = CollisionWorld::new();
    world.add_box(
        Vec3::new(-10.0, 2.0, -10.0),
        Vec3::new(0.0, 3.0, 10.0),
        "PLATFORM",
    );
    world.add_ground(0.0, "FLOOR");
    world.rebuild_broad_phase();

    let mut harness = Harness::new(world, Vec3::new(-3.0, 3.0, 0.0));
    harness.walk(Vec3::new(5.0, 0.0, 0.0), 60);
    assert!(harness.ever_airborne, "it walked off the edge onto nothing");

    harness.fall(120);
    assert!(harness.state.grounded, "it never landed");
    assert!(
        harness.position().y.abs() < 0.02,
        "{:?}",
        harness.position()
    );
    assert!(
        harness.state.velocity.y.abs() < 1e-3,
        "the landing was absorbed"
    );
}

#[test]
fn a_long_fall_lands_rather_than_passing_through_the_world() {
    let mut harness = Harness::new(floor_world(), Vec3::new(0.0, 400.0, 0.0));
    harness.state.grounded = false;
    harness.fall(1_200);

    assert!(harness.lowest > -0.05, "it reached {}", harness.lowest);
    assert!(harness.state.grounded);
}

// --- Jumping ----------------------------------------------------------------------------

#[test]
fn a_jump_leaves_the_ground_reaches_its_height_and_comes_back() {
    let mut harness = Harness::new(floor_world(), Vec3::ZERO);
    harness.fall(5);
    assert!(harness.jump(), "it should have been able to jump");

    harness.fall(120);
    let expected = JUMP_SPEED * JUMP_SPEED / (2.0 * GRAVITY);
    assert!(
        (harness.highest - expected).abs() < 0.25,
        "reached {} against an expected {expected}",
        harness.highest
    );
    assert!(harness.state.grounded, "it never came down");
}

#[test]
fn a_jump_is_refused_in_mid_air_and_allowed_again_on_landing() {
    let mut harness = Harness::new(floor_world(), Vec3::ZERO);
    harness.fall(5);
    assert!(harness.jump());
    harness.fall(10);
    assert!(!harness.jump(), "a second jump was allowed in the air");

    harness.fall(120);
    assert!(harness.state.grounded);
    assert!(harness.jump(), "it could not jump again after landing");
}

#[test]
fn a_jump_against_a_wall_goes_up_and_not_through() {
    let mut harness = Harness::new(wall_world(3.0), Vec3::ZERO);
    harness.walk(Vec3::new(7.0, 0.0, 0.0), 60);
    let against = harness.position().x;
    assert!(harness.jump());
    harness.walk(Vec3::new(7.0, 0.0, 0.0), 40);

    assert!(harness.highest > 0.5, "the jump did not happen");
    assert!(
        harness.position().x <= against + 0.02,
        "it climbed the wall: {:?}",
        harness.position()
    );
}

#[test]
fn a_jump_can_clear_an_obstacle_that_is_too_tall_to_step_onto() {
    let mut world = CollisionWorld::new();
    world.add_ground(0.0, "FLOOR");
    world.add_box(Vec3::new(4.0, 0.0, -5.0), Vec3::new(6.0, 0.9, 5.0), "BLOCK");
    world.rebuild_broad_phase();

    let mut harness = Harness::new(world, Vec3::ZERO);
    harness.walk(Vec3::new(6.0, 0.0, 0.0), 30);
    assert!(
        harness.position().x < 4.0,
        "the step-up climbed a 0.9 m block"
    );

    assert!(harness.jump());
    for _ in 0..90 {
        harness.walk(Vec3::new(6.0, 0.0, 0.0), 1);
        if harness.position().x > 5.0 {
            break;
        }
    }
    assert!(
        harness.position().x > 5.0,
        "the jump did not carry it on top: {:?}",
        harness.position()
    );
    assert!(
        harness.position().y > 0.8,
        "it is not on the block: {:?}",
        harness.position()
    );
}

// --- Steps ------------------------------------------------------------------------------

/// Walks +X into a block of `height` that spans x in [4, 12], stopping in the middle of it.
///
/// Where the character is when it stops is the answer: standing on top of the block, or
/// still in front of it. Stopping in the middle rather than running to the end matters —
/// a character that climbs the block walks off its far side and ends up back on the floor,
/// which would look exactly like never having climbed it.
fn step_over(height: f32) -> CharacterState {
    let mut world = CollisionWorld::new();
    world.add_ground(0.0, "FLOOR");
    world.add_box(
        Vec3::new(4.0, 0.0, -5.0),
        Vec3::new(12.0, height, 5.0),
        "STEP",
    );
    world.rebuild_broad_phase();

    let mut harness = Harness::new(world, Vec3::ZERO);
    for _ in 0..600 {
        harness.walk(Vec3::new(4.0, 0.0, 0.0), 1);
        if harness.position().x > 7.0 {
            break;
        }
    }
    harness.state
}

#[test]
fn a_flat_marking_on_the_floor_is_walked_straight_over() {
    let state = step_over(0.0);
    assert!(state.position.x > 7.0, "{state:?}");
    assert!(
        state.position.y < 0.02,
        "it climbed a flat surface: {state:?}"
    );
}

#[test]
fn a_small_step_is_walked_up_without_a_jump() {
    let state = step_over(0.2);
    assert!(state.position.x > 7.0, "the step blocked it: {state:?}");
    assert!(
        state.grounded,
        "it is airborne on top of the step: {state:?}"
    );
    assert!((state.position.y - 0.2).abs() < 0.05, "{state:?}");
}

#[test]
fn a_step_at_exactly_the_limit_is_walked_up() {
    let limit = CharacterConfig::default().max_step_height;
    let state = step_over(limit);
    assert!(
        state.position.x > 7.0,
        "a step at the limit blocked it: {state:?}"
    );
    assert!((state.position.y - limit).abs() < 0.05, "{state:?}");
}

#[test]
fn a_step_above_the_limit_is_a_wall() {
    let limit = CharacterConfig::default().max_step_height;
    for height in [limit + 0.3, 0.9, 2.5] {
        let state = step_over(height);
        assert!(
            state.position.x < 4.0,
            "a {height} m wall was climbed: {state:?}"
        );
        assert!(state.position.y < 0.05, "it climbed part way: {state:?}");
    }
}

#[test]
fn a_flight_of_steps_is_climbed_one_after_another() {
    let mut world = CollisionWorld::new();
    world.add_ground(0.0, "FLOOR");
    for index in 0..5 {
        let near = 4.0 + index as f32 * 1.5;
        world.add_box(
            Vec3::new(near, 0.0, -5.0),
            Vec3::new(near + 1.5, 0.3 * (index as f32 + 1.0), 5.0),
            "TREAD",
        );
    }
    world.rebuild_broad_phase();

    let mut harness = Harness::new(world, Vec3::ZERO);
    for _ in 0..900 {
        harness.walk(Vec3::new(3.0, 0.0, 0.0), 1);
        if harness.position().x > 10.5 {
            break;
        }
    }

    assert!(
        harness.position().y > 1.2,
        "it did not climb the flight: {:?}",
        harness.position()
    );
    assert!(harness.state.grounded);
}

// --- Slopes -----------------------------------------------------------------------------

#[test]
fn walkable_slopes_are_climbed_and_report_their_own_angle() {
    for degrees in [15.0_f32, 30.0, 45.0] {
        let mut harness = Harness::new(ramp_world(degrees), Vec3::new(-1.0, 0.5, 0.0));
        for _ in 0..600 {
            harness.walk(Vec3::new(4.0, 0.0, 0.0), 1);
            if harness.position().x > 4.0 {
                break;
            }
        }

        assert!(harness.state.grounded, "{degrees}: it left the ramp");
        assert!(
            harness.position().y > 0.3,
            "{degrees}: it did not climb ({:?})",
            harness.position()
        );
        let reported = harness.state.slope_angle().to_degrees();
        assert!(
            (reported - degrees).abs() < 3.0,
            "{degrees} degrees reported as {reported}"
        );
    }
}

#[test]
fn a_slope_past_the_limit_cannot_be_climbed_and_is_reported_as_a_wall() {
    let mut harness = Harness::new(ramp_world(70.0), Vec3::new(-1.0, 0.5, 0.0));
    harness.walk(Vec3::new(4.0, 0.0, 0.0), 240);

    assert!(
        harness.position().y < 1.0,
        "a 70-degree face was climbed to {}",
        harness.position().y
    );
    assert!(harness.ever_hit_wall, "the steep face was never a wall");
}

#[test]
fn a_flat_ramp_is_just_a_floor() {
    let mut harness = Harness::new(ramp_world(0.0), Vec3::new(1.0, 1.0, 0.0));
    harness.walk(Vec3::new(4.0, 0.0, 0.0), 120);
    assert!(harness.state.grounded);
    assert!(harness.state.slope_angle().to_degrees() < 1.0);
}

#[test]
fn walking_down_a_slope_is_a_walk_and_not_a_series_of_falls() {
    let mut harness = Harness::new(ramp_world(25.0), Vec3::new(5.0, 3.0, 0.0));
    harness.fall(120);
    assert!(
        harness.state.grounded,
        "it did not settle: {:?}",
        harness.position()
    );

    let mut airborne = 0;
    for _ in 0..120 {
        harness.walk(Vec3::new(-4.0, 0.0, 0.0), 1);
        if !harness.state.grounded {
            airborne += 1;
        }
    }
    assert!(airborne <= 2, "it bounced downhill for {airborne} ticks");
}

// --- Tunnelling -------------------------------------------------------------------------

#[test]
fn a_very_fast_run_cannot_cross_a_thin_wall() {
    let mut world = CollisionWorld::new();
    world.add_ground(0.0, "FLOOR");
    world.add_box(
        Vec3::new(10.0, 0.0, -20.0),
        Vec3::new(10.05, 4.0, 20.0),
        "THIN WALL",
    );
    world.rebuild_broad_phase();

    let mut harness = Harness::new(world, Vec3::ZERO);
    harness.walk(Vec3::new(300.0, 0.0, 0.0), 120);
    assert!(
        harness.position().x < 10.0,
        "it tunnelled to {}",
        harness.position().x
    );
}

#[test]
fn a_very_fast_fall_cannot_cross_a_thin_floor() {
    let mut world = CollisionWorld::new();
    world.add_box(
        Vec3::new(-10.0, 20.0, -10.0),
        Vec3::new(10.0, 20.02, 10.0),
        "THIN SLAB",
    );
    world.rebuild_broad_phase();

    let mut harness = Harness::new(world, Vec3::new(0.0, 300.0, 0.0));
    harness.state.grounded = false;
    harness.fall(900);

    assert!(
        harness.state.grounded && harness.position().y > 20.0,
        "it fell through the slab: {:?}",
        harness.position()
    );
}

#[test]
fn a_fast_diagonal_run_at_a_corner_does_not_slip_between_two_walls() {
    let mut world = CollisionWorld::new();
    world.add_ground(0.0, "FLOOR");
    world.add_box(
        Vec3::new(6.0, 0.0, -20.0),
        Vec3::new(6.1, 4.0, 6.0),
        "THIN X",
    );
    world.add_box(
        Vec3::new(-20.0, 0.0, 6.0),
        Vec3::new(6.1, 4.0, 6.1),
        "THIN Z",
    );
    world.rebuild_broad_phase();

    let mut harness = Harness::new(world, Vec3::ZERO);
    harness.walk(Vec3::new(120.0, 0.0, 120.0), 120);
    assert!(
        harness.position().x < 6.1 && harness.position().z < 6.1,
        "it slipped through the corner: {:?}",
        harness.position()
    );
}

// --- Cost -------------------------------------------------------------------------------

#[test]
fn a_thousand_ticks_of_movement_allocate_nothing() {
    let mut harness = Harness::new(wall_world(6.0), Vec3::ZERO);
    harness.walk(Vec3::new(4.0, 0.0, 1.0), 60);
    let capacity = harness.buffers.capacity();

    for tick in 0..1_000 {
        let z = ((tick % 11) as f32 - 5.0) * 1.2;
        harness.walk(Vec3::new(4.0, 0.0, z), 1);
    }
    assert_eq!(harness.buffers.capacity(), capacity, "a move allocated");
}

#[test]
fn the_work_a_move_does_is_counted_and_bounded() {
    let mut harness = Harness::new(wall_world(4.0), Vec3::ZERO);
    harness.walk(Vec3::new(7.0, 0.0, 0.0), 90);

    let stats = harness.last.stats;
    // Narrow tests can exceed broad candidates: the step probe asks the world directly,
    // one small query at a time, and each of those is a test with no candidate list behind
    // it. What must always hold is that a contact came from a test.
    assert!(stats.narrow_tests >= stats.contacts, "{stats:?}");
    assert!(
        stats.broad_candidates > 0,
        "nothing was ever offered: {stats:?}"
    );
    assert!(
        harness.last.iterations <= harness.config.max_resolution_iterations * 8,
        "resolution ran away: {}",
        harness.last.iterations
    );
}
