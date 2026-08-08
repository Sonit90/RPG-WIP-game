//! The character controller: acceleration, gravity, ground contact and jumping.
//!
//! This is not a physics engine and does not want to become one. There are no rigid bodies
//! and no solver — there is a capsule-shaped intention, and the rules that turn a key press
//! into a velocity. Everything it does is one function over one [`Body`], so it is
//! deterministic, testable without a world, and cheap enough that a thousand of them cost
//! nothing.
//!
//! There are two of those functions, and the difference between them is the whole of what
//! collision changed:
//!
//! ```text
//! step           .. -> gravity -> integrate -> y <= 0 -> clamp x,z to the arena
//! step_collided  .. -> gravity -> engine_collision::move_character -> arena
//!                                 ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
//!                                 walls, corners, slopes, steps, ledges
//! ```
//!
//! [`step`] is kept for bodies that have no reason to collide with anything — the training
//! dummies, which stand still on a flat floor — and because a movement rule is easier to
//! reason about when it can be tested without building a level. The player goes through
//! [`step_collided`].
//!
//! Position is the **feet**, not the centre. Ground contact is then `y <= 0` rather than
//! `y <= half_height`, which is one fewer number to get wrong, and the presentation layer
//! is the only place that needs to know how tall the body's mesh is.

use engine_collision::controller::{self, CharacterConfig, CharacterState, MoveResult};
use engine_collision::world::{CollisionWorld, QueryBuffers};
use engine_core::math::Vec3;

/// What a body is doing, for animation and for the developer overlay.
///
/// Derived from velocity and ground contact every step rather than stored as a state
/// machine: two sources for "am I falling" is how a character ends up playing a fall
/// animation while standing still.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MovementState {
    /// On the ground, not moving.
    Idle,
    /// On the ground, moving.
    Running,
    /// Airborne and rising.
    Jumping,
    /// Airborne and descending.
    Falling,
}

impl MovementState {
    /// Whether the body is off the ground.
    #[must_use]
    pub fn is_airborne(self) -> bool {
        matches!(self, Self::Jumping | Self::Falling)
    }

    /// Short uppercase name, for the developer overlay's font.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "IDLE",
            Self::Running => "RUNNING",
            Self::Jumping => "JUMPING",
            Self::Falling => "FALLING",
        }
    }
}

/// How a body moves. Tuning, not state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MovementConfig {
    /// Top horizontal speed, metres per second.
    pub max_speed: f32,
    /// How fast the body reaches its intended velocity on the ground, m/s².
    pub acceleration: f32,
    /// How fast it stops when there is no intent, m/s².
    pub deceleration: f32,
    /// Fraction of ground acceleration available while airborne.
    ///
    /// Not zero and not one: zero makes a jump feel like a commitment to a direction chosen
    /// a moment earlier, and one makes the jump irrelevant because the player steers
    /// through it exactly as if they were running.
    pub air_control: f32,
    /// Downward acceleration, m/s². Larger than 9.81 on purpose: real gravity makes a jump
    /// that reaches a useful height feel like it happens underwater.
    pub gravity: f32,
    /// Upward speed a jump starts with, m/s.
    pub jump_speed: f32,
    /// Height of the floor the body stands on.
    pub ground_height: f32,
    /// Half-width of the square arena the body is confined to.
    pub arena_half_extent: f32,
    /// Terminal downward speed, m/s. Bounds the integration step of a long fall.
    pub terminal_velocity: f32,
}

impl Default for MovementConfig {
    fn default() -> Self {
        Self {
            max_speed: 7.0,
            acceleration: 60.0,
            deceleration: 50.0,
            air_control: 0.35,
            gravity: 25.0,
            // 8 m/s against 25 m/s² gravity is a 1.28 m jump lasting 0.64 s: high enough to
            // read as a jump, short enough that it does not interrupt combat.
            jump_speed: 8.0,
            ground_height: 0.0,
            arena_half_extent: 40.0,
            terminal_velocity: 60.0,
        }
    }
}

impl MovementConfig {
    /// The height a jump from rest reaches, in metres.
    ///
    /// Derived rather than tuned, so changing `jump_speed` or `gravity` cannot leave a
    /// stale comment behind.
    #[must_use]
    pub fn jump_height(&self) -> f32 {
        if self.gravity <= 0.0 {
            return f32::INFINITY;
        }
        self.jump_speed * self.jump_speed / (2.0 * self.gravity)
    }
}

/// What a controller wants a body to do this step.
///
/// The direction is in world space and need not be normalized — the caller builds it from
/// the camera's yaw and two key axes, and normalizing at the call site is how diagonal
/// movement ends up faster than straight movement.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct MoveIntent {
    /// Desired horizontal direction in world space. The Y component is ignored.
    pub direction: Vec3,
    /// Whether a jump was requested this step.
    pub jump: bool,
}

impl MoveIntent {
    /// No movement and no jump.
    pub const NONE: Self = Self {
        direction: Vec3::ZERO,
        jump: false,
    };

    /// An intent to move in `direction`.
    #[must_use]
    pub fn moving(direction: Vec3) -> Self {
        Self {
            direction,
            jump: false,
        }
    }
}

/// Where a body is and what it is doing.
///
/// Plain data. Systems act on it; it acts on nothing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Body {
    /// Position of the feet in world space.
    pub position: Vec3,
    /// Velocity in world space, metres per second.
    pub velocity: Vec3,
    /// Whether the feet are in contact with the floor.
    pub grounded: bool,
    /// The normal of the surface under the feet. `+Y` when airborne or on flat ground.
    ///
    /// Carried on the body rather than recomputed by whoever wants it, because it is an
    /// output of the one place that resolved the move and re-deriving it anywhere else
    /// would be a second opinion about which way is up.
    pub ground_normal: Vec3,
    /// Which way the body faces, radians about +Y. Zero faces −Z.
    pub facing_yaw: f32,
    /// What the body is doing, derived every step.
    pub state: MovementState,
}

impl Default for Body {
    fn default() -> Self {
        Self::at(Vec3::ZERO)
    }
}

impl Body {
    /// A body standing still at `position`.
    #[must_use]
    pub fn at(position: Vec3) -> Self {
        Self {
            position,
            velocity: Vec3::ZERO,
            grounded: true,
            ground_normal: Vec3::Y,
            facing_yaw: 0.0,
            state: MovementState::Idle,
        }
    }

    /// Horizontal speed, ignoring any rise or fall.
    #[must_use]
    pub fn horizontal_speed(&self) -> f32 {
        self.velocity.x.hypot(self.velocity.z)
    }

    /// The angle of the surface under the feet, in radians. Zero when airborne.
    #[must_use]
    pub fn slope_angle(&self) -> f32 {
        if !self.grounded {
            return 0.0;
        }
        self.ground_normal.dot(Vec3::Y).clamp(-1.0, 1.0).acos()
    }
}

/// Distance from the floor within which the body counts as standing on it.
///
/// Floating-point integration lands a fraction of a millimetre off exactly zero; without a
/// tolerance the body would flicker between grounded and airborne while standing still,
/// and a jump would be refused on the frames it read as airborne.
const GROUND_EPSILON: f32 = 1e-3;

/// Advances one body by one fixed simulation step.
///
/// The order is load-bearing. Jumping before gravity means a jump gets its full upward
/// speed on the tick it was requested rather than a tick's worth of gravity subtracted from
/// it; ground contact after integration means the body can never end a step below the
/// floor, which is the only "collision" this controller has.
pub fn step(body: &mut Body, intent: MoveIntent, config: &MovementConfig, seconds: f32) {
    if seconds <= 0.0 {
        return;
    }

    accelerate_horizontally(body, intent, config, seconds);

    // Only from the ground: without this check a held jump key is a flight control.
    if intent.jump && body.grounded {
        body.velocity.y = config.jump_speed;
        body.grounded = false;
    }

    if !body.grounded {
        body.velocity.y =
            (body.velocity.y - config.gravity * seconds).max(-config.terminal_velocity);
    }

    body.position += body.velocity * seconds;

    resolve_ground(body, config);
    confine_to_arena(body, config);
    face_movement(body);
    body.state = classify(body);
}

/// Advances one body by one fixed simulation step, against real collision geometry.
///
/// The same rules as [`step`] — the same acceleration, the same jump, the same gravity —
/// with the integrate-and-clamp replaced by a swept move through a
/// [`CollisionWorld`]. Everything about *how* the body wants to move stays here in
/// gameplay; everything about what stops it lives in `engine-collision`.
///
/// ```text
/// intent -> accelerate -> jump -> gravity -> move_character -> arena -> facing -> state
///                                            ^^^^^^^^^^^^^^
///                                            walls, slopes, steps, ground
/// ```
///
/// `buffers` is the caller's scratch space, reused across ticks: this function does not
/// allocate, and neither does anything it calls.
///
/// Returns what the move did, for a developer readout. Gameplay ignores it.
pub fn step_collided(
    body: &mut Body,
    intent: MoveIntent,
    config: &MovementConfig,
    character: &CharacterConfig,
    world: &CollisionWorld,
    buffers: &mut QueryBuffers,
    seconds: f32,
) -> MoveResult {
    if seconds <= 0.0 {
        return MoveResult::default();
    }

    accelerate_horizontally(body, intent, config, seconds);

    if intent.jump && body.grounded {
        body.velocity.y = config.jump_speed;
        body.grounded = false;
    }

    // Gravity always, not only when airborne. A character standing on a slope needs a
    // downward push to stay pressed against it, and the resolution removes whatever part of
    // it points into the ground — so a body at rest on flat floor still ends the tick with
    // zero vertical velocity.
    body.velocity.y = (body.velocity.y - config.gravity * seconds).max(-config.terminal_velocity);

    let mut state = CharacterState {
        position: body.position,
        velocity: body.velocity,
        grounded: body.grounded,
        ground_normal: body.ground_normal,
        ground_id: None,
    };
    let displacement = state.velocity * seconds;
    let result = controller::move_character(world, character, &mut state, displacement, buffers);

    body.position = state.position;
    body.velocity = state.velocity;
    body.grounded = state.grounded;
    body.ground_normal = state.ground_normal;

    confine_to_arena(body, config);
    face_movement(body);
    body.state = classify(body);
    result
}

/// Moves the horizontal velocity towards what the intent asks for.
fn accelerate_horizontally(
    body: &mut Body,
    intent: MoveIntent,
    config: &MovementConfig,
    seconds: f32,
) {
    let flat = Vec3::new(intent.direction.x, 0.0, intent.direction.z);
    let has_intent = flat.length_squared() > f32::EPSILON;
    let desired = if has_intent {
        flat.normalize() * config.max_speed
    } else {
        Vec3::ZERO
    };

    let rate = if has_intent {
        config.acceleration
    } else {
        config.deceleration
    } * if body.grounded {
        1.0
    } else {
        config.air_control
    };

    let current = Vec3::new(body.velocity.x, 0.0, body.velocity.z);
    let delta = desired - current;
    let distance = delta.length();
    if distance <= f32::EPSILON {
        return;
    }

    // Clamped to what is left rather than stepped blindly: overshooting produces a jitter
    // that looks exactly like an input bug at high tick rates.
    let change = (rate * seconds).min(distance);
    let applied = delta.normalize() * change;
    body.velocity.x += applied.x;
    body.velocity.z += applied.z;
}

/// Puts the body back on the floor if it went through it.
fn resolve_ground(body: &mut Body, config: &MovementConfig) {
    if body.position.y <= config.ground_height + GROUND_EPSILON {
        body.position.y = config.ground_height;
        if body.velocity.y < 0.0 {
            body.velocity.y = 0.0;
        }
        body.grounded = true;
    } else {
        body.grounded = false;
    }
}

/// Keeps the body inside the arena.
fn confine_to_arena(body: &mut Body, config: &MovementConfig) {
    let limit = config.arena_half_extent;
    if limit <= 0.0 {
        return;
    }
    for (position, velocity) in [
        (&mut body.position.x, &mut body.velocity.x),
        (&mut body.position.z, &mut body.velocity.z),
    ] {
        if *position > limit {
            *position = limit;
            *velocity = velocity.min(0.0);
        } else if *position < -limit {
            *position = -limit;
            *velocity = velocity.max(0.0);
        }
    }
}

/// Turns the body to face the way it is moving.
fn face_movement(body: &mut Body) {
    // Below walking pace the direction is numerical noise, and turning to face it makes a
    // standing character spin.
    if body.horizontal_speed() > 0.1 {
        body.facing_yaw = (-body.velocity.x).atan2(-body.velocity.z);
    }
}

/// Derives the movement state from velocity and ground contact.
fn classify(body: &Body) -> MovementState {
    if !body.grounded {
        if body.velocity.y > 0.0 {
            MovementState::Jumping
        } else {
            MovementState::Falling
        }
    } else if body.horizontal_speed() > 0.1 {
        MovementState::Running
    } else {
        MovementState::Idle
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    /// One 60 Hz tick.
    const STEP: f32 = 1.0 / 60.0;

    fn run(body: &mut Body, intent: MoveIntent, config: &MovementConfig, ticks: u32) {
        for _ in 0..ticks {
            step(body, intent, config, STEP);
        }
    }

    #[test]
    fn a_standing_body_stays_where_it_is() {
        let config = MovementConfig::default();
        let mut body = Body::at(Vec3::new(3.0, 0.0, -2.0));
        run(&mut body, MoveIntent::NONE, &config, 120);

        assert_eq!(body.position, Vec3::new(3.0, 0.0, -2.0));
        assert!(body.grounded);
        assert_eq!(body.state, MovementState::Idle);
    }

    #[test]
    fn movement_accelerates_to_the_configured_top_speed_and_no_further() {
        let config = MovementConfig::default();
        let mut body = Body::default();
        run(&mut body, MoveIntent::moving(-Vec3::Z), &config, 240);

        assert!(
            (body.horizontal_speed() - config.max_speed).abs() < 1e-3,
            "{}",
            body.horizontal_speed()
        );
        assert!(body.position.z < -20.0, "{:?}", body.position);
        assert_eq!(body.state, MovementState::Running);
    }

    #[test]
    fn releasing_the_input_decelerates_to_a_stop() {
        let config = MovementConfig::default();
        let mut body = Body::default();
        run(&mut body, MoveIntent::moving(Vec3::X), &config, 60);
        assert!(body.horizontal_speed() > 1.0);

        run(&mut body, MoveIntent::NONE, &config, 60);
        assert_eq!(body.horizontal_speed(), 0.0);
        assert_eq!(body.state, MovementState::Idle);
    }

    #[test]
    fn diagonal_movement_is_not_faster_than_straight_movement() {
        let config = MovementConfig::default();
        let mut straight = Body::default();
        run(&mut straight, MoveIntent::moving(-Vec3::Z), &config, 120);
        let mut diagonal = Body::default();
        run(
            &mut diagonal,
            MoveIntent::moving(Vec3::new(1.0, 0.0, -1.0)),
            &config,
            120,
        );

        assert!(
            (straight.horizontal_speed() - diagonal.horizontal_speed()).abs() < 1e-3,
            "{} vs {}",
            straight.horizontal_speed(),
            diagonal.horizontal_speed()
        );
    }

    #[test]
    fn standing_jumping_falling_and_landing_happen_in_that_order() {
        // The sequence the milestone asks for, asserted as one story rather than four.
        let config = MovementConfig::default();
        let mut body = Body::default();

        assert!(body.grounded && body.state == MovementState::Idle);

        step(
            &mut body,
            MoveIntent {
                jump: true,
                ..MoveIntent::NONE
            },
            &config,
            STEP,
        );
        assert!(!body.grounded, "the jump left the ground");
        assert_eq!(body.state, MovementState::Jumping);
        assert!(body.position.y > 0.0);

        // Rise to the apex.
        let mut apex: f32 = 0.0;
        let mut saw_falling = false;
        for _ in 0..200 {
            step(&mut body, MoveIntent::NONE, &config, STEP);
            apex = apex.max(body.position.y);
            saw_falling |= body.state == MovementState::Falling;
            if body.grounded {
                break;
            }
        }

        assert!(saw_falling, "the body never started descending");
        assert!(body.grounded, "the body never landed");
        assert_eq!(body.position.y, 0.0, "it landed exactly on the floor");
        assert_eq!(body.velocity.y, 0.0);
        assert_eq!(body.state, MovementState::Idle);
        assert!(
            (apex - config.jump_height()).abs() < 0.2,
            "apex {apex} against a computed {}",
            config.jump_height()
        );
    }

    #[test]
    fn jumping_is_refused_while_airborne() {
        // Holding the key must not be a flight control.
        let config = MovementConfig::default();
        let mut body = Body::default();
        let jump = MoveIntent {
            jump: true,
            ..MoveIntent::NONE
        };

        step(&mut body, jump, &config, STEP);
        let after_first = body.position.y;
        let mut peak = after_first;
        for _ in 0..30 {
            step(&mut body, jump, &config, STEP);
            peak = peak.max(body.position.y);
        }
        assert!(
            peak < config.jump_height() + 0.05,
            "a held jump climbed to {peak}"
        );
    }

    #[test]
    fn a_body_spawned_in_the_air_falls_and_never_passes_the_floor() {
        let config = MovementConfig::default();
        let mut body = Body::at(Vec3::new(0.0, 30.0, 0.0));
        body.grounded = false;

        let mut lowest = f32::MAX;
        for _ in 0..600 {
            step(&mut body, MoveIntent::NONE, &config, STEP);
            lowest = lowest.min(body.position.y);
        }
        assert!(lowest >= 0.0, "the body reached {lowest}");
        assert!(body.grounded);
        assert_eq!(body.position.y, 0.0);
    }

    #[test]
    fn a_long_fall_is_bounded_by_terminal_velocity() {
        let config = MovementConfig::default();
        let mut body = Body::at(Vec3::new(0.0, 10_000.0, 0.0));
        body.grounded = false;
        for _ in 0..2000 {
            step(&mut body, MoveIntent::NONE, &config, STEP);
            assert!(
                body.velocity.y >= -config.terminal_velocity,
                "{}",
                body.velocity.y
            );
        }
    }

    #[test]
    fn air_control_is_weaker_than_ground_control() {
        let config = MovementConfig::default();
        let intent = MoveIntent::moving(Vec3::X);

        let mut ground = Body::default();
        run(&mut ground, intent, &config, 6);

        let mut air = Body {
            grounded: false,
            position: Vec3::new(0.0, 5.0, 0.0),
            ..Body::default()
        };
        run(&mut air, intent, &config, 6);

        assert!(
            air.horizontal_speed() < ground.horizontal_speed(),
            "air {} ground {}",
            air.horizontal_speed(),
            ground.horizontal_speed()
        );
        assert!(air.horizontal_speed() > 0.0, "some control, not none");
    }

    #[test]
    fn the_arena_confines_the_body_without_trapping_it() {
        let config = MovementConfig::default();
        let mut body = Body::default();
        run(&mut body, MoveIntent::moving(Vec3::X), &config, 2000);
        assert!(
            (body.position.x - config.arena_half_extent).abs() < 1e-3,
            "{:?}",
            body.position
        );

        // The wall must not hold the body: walking back has to work immediately.
        run(&mut body, MoveIntent::moving(-Vec3::X), &config, 60);
        assert!(body.position.x < config.arena_half_extent - 1.0);
    }

    #[test]
    fn the_body_turns_to_face_the_way_it_moves() {
        let config = MovementConfig::default();
        let mut body = Body::default();
        run(&mut body, MoveIntent::moving(-Vec3::Z), &config, 60);
        assert!(body.facing_yaw.abs() < 1e-3, "{}", body.facing_yaw);

        run(&mut body, MoveIntent::moving(Vec3::X), &config, 120);
        // Facing +X is a quarter turn clockwise from facing −Z.
        assert!(
            (body.facing_yaw + std::f32::consts::FRAC_PI_2).abs() < 1e-2,
            "{}",
            body.facing_yaw
        );
    }

    #[test]
    fn a_zero_length_step_changes_nothing() {
        let config = MovementConfig::default();
        let mut body = Body::default();
        let before = body;
        step(&mut body, MoveIntent::moving(Vec3::X), &config, 0.0);
        assert_eq!(body, before, "the first frame has a zero delta");
    }
}
