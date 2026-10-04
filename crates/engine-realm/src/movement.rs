//! One movement input, applied the same way on the server and in the client's prediction.
//!
//! ```text
//! client: InputFrame ──► apply_input (prediction) ──► shown at once
//!            │
//!            └─► move_input reducer ──► InputBudget ──► apply_input (authority) ──► row
//!                                                                     │
//! client: row (pos, vel, last_seq) ──► rewind to it, replay unacknowledged frames ◄┘
//! ```
//!
//! # What the server trusts
//!
//! Only the *intent*: two stick axes, a camera yaw, a jump bit. Never a position, a velocity
//! or a speed. The server integrates every accepted frame through the same character
//! controller the game uses offline ([`engine_game::movement::step_collided`]) against its own
//! copy of the level's collision, so a client that lies about where it is simply is not there.
//!
//! # Speed hacks
//!
//! A client that sends frames faster than real time would move faster. [`InputBudget`] is a
//! token bucket refilled at the simulation rate (60 frames per second of *server* time) with a
//! small burst allowance for jitter; frames beyond it are dropped, not queued. Sequence
//! numbers must increase, so a replayed or reordered frame is dropped too.
//!
//! # The level
//!
//! [`LevelGeometry::build`] builds the collision world from a scene through the same
//! `engine_game::arena` functions the offline game uses, plus a box per closed barrier, so
//! client and server collide against the same walls.

use engine_collision::controller::CharacterConfig;
use engine_collision::world::{CollisionWorld, QueryBuffers};
use engine_core::math::Vec3;
use engine_environment::EnvironmentScene;
use engine_game::arena::{
    ArenaConfig, add_environment_collision, add_terrain_collision, build_collision,
};
use engine_game::command::PlayerCommand;
use engine_game::movement::{Body, MoveIntent, MovementConfig, step_collided};

use crate::Micros;
use crate::catalog::{BarrierBox, DungeonDefinition};

/// The simulation rate a frame stands for. One frame is one 60 Hz tick.
pub const TICK_HZ: u32 = engine_core::time::DEFAULT_TICK_HZ;

/// Seconds one frame integrates.
#[allow(clippy::cast_precision_loss, reason = "a tick rate is a small integer")]
pub const TICK_SECONDS: f32 = 1.0 / TICK_HZ as f32;

/// The most frames one `move_input` message may carry.
pub const MAX_FRAMES_PER_MESSAGE: usize = 16;

/// One tick of player intent: everything a client may say about its own movement.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InputFrame {
    /// Increases by one per frame. The server acknowledges the last one it applied.
    pub seq: u32,
    /// Forward on the camera's axis, [-1, 1].
    pub forward: f32,
    /// Right on the camera's axis, [-1, 1].
    pub right: f32,
    /// The camera's yaw, radians.
    pub yaw: f32,
    /// Whether the camera is steering the character.
    pub face_camera: bool,
    /// A jump this tick.
    pub jump: bool,
}

impl InputFrame {
    /// The frame with every value forced into range, or `None` if any value is not a number.
    ///
    /// A NaN axis would poison the body's position for good; an axis of 50 would be a speed
    /// hack. Both are refused here, before anything is integrated.
    #[must_use]
    pub fn sanitized(self) -> Option<Self> {
        if !(self.forward.is_finite() && self.right.is_finite() && self.yaw.is_finite()) {
            return None;
        }
        Some(Self {
            forward: self.forward.clamp(-1.0, 1.0),
            right: self.right.clamp(-1.0, 1.0),
            yaw: wrap_angle(self.yaw),
            ..self
        })
    }

    /// The intent this frame asks for, exactly as the offline simulation derives it.
    #[must_use]
    pub fn intent(&self) -> MoveIntent {
        let command = PlayerCommand::Move {
            forward: self.forward,
            right: self.right,
            yaw: self.yaw,
            face_camera: self.face_camera,
        };
        MoveIntent {
            direction: command.move_direction(),
            jump: self.jump,
            facing: command.facing_lock(),
        }
    }
}

/// `angle` in [-π, π).
fn wrap_angle(angle: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    (angle + PI).rem_euclid(TAU) - PI
}

/// How a player character moves: the tuning and the capsule.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MovementRules {
    /// Speeds, accelerations, gravity, the arena bound.
    pub movement: MovementConfig,
    /// The capsule.
    pub character: CharacterConfig,
}

/// Applies one frame to `body`. The one movement function shared by server and client.
pub fn apply_input(
    body: &mut Body,
    frame: &InputFrame,
    rules: &MovementRules,
    world: &CollisionWorld,
    buffers: &mut QueryBuffers,
) {
    step_collided(
        body,
        frame.intent(),
        &rules.movement,
        &rules.character,
        world,
        buffers,
        TICK_SECONDS,
    );
}

/// Integrates a body with no intent for one tick: gravity and friction for a player who has
/// stopped sending input (a lost connection mid-jump lands rather than hanging in the air).
pub fn apply_idle(
    body: &mut Body,
    rules: &MovementRules,
    world: &CollisionWorld,
    buffers: &mut QueryBuffers,
) {
    step_collided(
        body,
        MoveIntent::NONE,
        &rules.movement,
        &rules.character,
        world,
        buffers,
        TICK_SECONDS,
    );
}

/// A token bucket of movement frames, refilled by server time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InputBudget {
    /// Frames that may still be applied.
    pub tokens: f32,
    /// When it was last refilled.
    pub refilled_at: Micros,
}

impl InputBudget {
    /// The most frames that may be banked: a fifth of a second of jitter.
    pub const BURST: f32 = 12.0;

    /// A full bucket at `now`.
    #[must_use]
    pub fn new(now: Micros) -> Self {
        Self {
            tokens: Self::BURST,
            refilled_at: now,
        }
    }

    /// Refills for the time since the last call and returns how many of `requested` frames
    /// may be applied, spending them.
    pub fn admit(&mut self, requested: usize, now: Micros) -> usize {
        let elapsed = (now - self.refilled_at).max(0);
        #[allow(clippy::cast_precision_loss, reason = "elapsed is clamped below")]
        let earned = (elapsed.min(10_000_000) as f32) * 1e-6 * TICK_HZ as f32;
        self.tokens = (self.tokens + earned).min(Self::BURST);
        self.refilled_at = now;
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "tokens is in [0, BURST]"
        )]
        let whole = self.tokens.floor().max(0.0) as usize;
        let admitted = requested.min(whole);
        #[allow(clippy::cast_precision_loss, reason = "admitted is at most BURST")]
        let spent = admitted as f32;
        self.tokens -= spent;
        admitted
    }
}

/// The frames of a message the server should apply, in order: sanitized, strictly after
/// `last_seq`, and within budget. Returns them and the number dropped.
#[must_use]
pub fn accept_frames(
    frames: &[InputFrame],
    last_seq: u32,
    budget: &mut InputBudget,
    now: Micros,
) -> (Vec<InputFrame>, usize) {
    let mut fresh: Vec<InputFrame> = Vec::with_capacity(frames.len());
    let mut highest = last_seq;
    for frame in frames.iter().take(MAX_FRAMES_PER_MESSAGE) {
        if frame.seq <= highest {
            continue;
        }
        let Some(clean) = frame.sanitized() else {
            continue;
        };
        highest = frame.seq;
        fresh.push(clean);
    }
    let admitted = budget.admit(fresh.len(), now);
    let dropped = frames.len() - admitted;
    fresh.truncate(admitted);
    (fresh, dropped)
}

/// Which kind of barrier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BarrierKind {
    /// Holds the party in the start area during the countdown.
    Start,
    /// Seals the boss until the dungeon is complete enough.
    BossGate,
}

/// A barrier's collision box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Barrier {
    /// Which barrier.
    pub kind: BarrierKind,
    /// Minimum corner.
    pub min: Vec3,
    /// Maximum corner.
    pub max: Vec3,
}

/// Every barrier a dungeon has.
#[must_use]
pub fn barriers_of(scene: &EnvironmentScene, dungeon: &DungeonDefinition) -> Vec<Barrier> {
    let mut barriers = vec![start_barrier(&dungeon.start_barrier)];
    if let Some(gate) = scene
        .dungeon
        .as_ref()
        .and_then(|settings| settings.boss_gate)
    {
        let (sin, cos) = gate.yaw.sin_cos();
        let half = gate.half_extents;
        let x = (half.x * cos).abs() + (half.z * sin).abs();
        let z = (half.x * sin).abs() + (half.z * cos).abs();
        barriers.push(Barrier {
            kind: BarrierKind::BossGate,
            min: Vec3::new(gate.center.x - x, gate.center.y, gate.center.z - z),
            max: Vec3::new(
                gate.center.x + x,
                gate.center.y + half.y * 2.0,
                gate.center.z + z,
            ),
        });
    }
    barriers
}

fn start_barrier(barrier: &BarrierBox) -> Barrier {
    let [x0, y0, z0] = barrier.min();
    let [x1, y1, z1] = barrier.max();
    Barrier {
        kind: BarrierKind::Start,
        min: Vec3::new(x0, y0, z0),
        max: Vec3::new(x1, y1, z1),
    }
}

/// A level's collision and how characters move in it.
pub struct LevelGeometry {
    /// Static collision, including whichever barriers it was built with.
    pub world: CollisionWorld,
    /// How a character moves here.
    pub rules: MovementRules,
    /// Below this height a body has fallen out of the level.
    pub void_height: f32,
}

impl LevelGeometry {
    /// The collision of `scene` with the given barriers standing.
    #[must_use]
    pub fn build(scene: &EnvironmentScene, closed: &[Barrier]) -> Self {
        let config = ArenaConfig::from_environment(scene, 0);
        let mut world = CollisionWorld::new();
        build_collision(&mut world, &config);
        add_terrain_collision(&mut world, scene);
        add_environment_collision(&mut world, scene);
        for barrier in closed {
            world.add_box(barrier.min, barrier.max, barrier_label(barrier.kind));
        }
        world.rebuild_broad_phase();
        Self {
            world,
            rules: MovementRules {
                movement: config.movement(),
                character: engine_game::SimConfig::default().character,
            },
            void_height: config.void_height,
        }
    }
}

/// The label a barrier's collision carries.
#[must_use]
pub fn barrier_label(kind: BarrierKind) -> &'static str {
    match kind {
        BarrierKind::Start => "start barrier",
        BarrierKind::BossGate => "boss gate",
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code asserts on known-good values")]
mod tests {
    use super::*;
    use crate::SECOND;
    use crate::catalog::Catalog;

    fn frame(seq: u32, forward: f32) -> InputFrame {
        InputFrame {
            seq,
            forward,
            right: 0.0,
            yaw: 0.0,
            face_camera: false,
            jump: false,
        }
    }

    fn flat_level() -> LevelGeometry {
        let scene = EnvironmentScene::empty("flat");
        LevelGeometry::build(&scene, &[])
    }

    #[test]
    fn garbage_is_refused_and_axes_are_clamped() {
        assert!(frame(1, f32::NAN).sanitized().is_none());
        assert!(
            InputFrame {
                yaw: f32::INFINITY,
                ..frame(1, 1.0)
            }
            .sanitized()
            .is_none()
        );
        let clean = frame(1, 50.0).sanitized().expect("finite");
        assert!((clean.forward - 1.0).abs() < f32::EPSILON);
        let wrapped = InputFrame {
            yaw: 7.0,
            ..frame(1, 0.0)
        }
        .sanitized()
        .expect("finite");
        assert!(wrapped.yaw >= -std::f32::consts::PI && wrapped.yaw < std::f32::consts::PI);
    }

    #[test]
    fn a_clamped_axis_cannot_outrun_max_speed() {
        let level = flat_level();
        let mut buffers = QueryBuffers::new();
        let mut honest = Body::at(Vec3::ZERO);
        let mut cheat = Body::at(Vec3::ZERO);
        for seq in 1..=120 {
            apply_input(
                &mut honest,
                &frame(seq, 1.0),
                &level.rules,
                &level.world,
                &mut buffers,
            );
            let forged = frame(seq, 1000.0).sanitized().expect("finite");
            apply_input(
                &mut cheat,
                &forged,
                &level.rules,
                &level.world,
                &mut buffers,
            );
        }
        assert!((honest.position - cheat.position).length() < 1e-4);
        assert!(honest.horizontal_speed() <= level.rules.movement.max_speed + 1e-3);
    }

    #[test]
    fn the_budget_admits_real_time_and_no_more() {
        let mut budget = InputBudget::new(0);
        // A burst is fine…
        assert_eq!(budget.admit(12, 0), 12);
        // …but not twice without time passing.
        assert_eq!(budget.admit(12, 0), 0);
        // A second of server time is sixty frames, capped at the burst.
        assert_eq!(budget.admit(100, SECOND), 12);
        // A steady 60 Hz stream is admitted in full.
        let mut budget = InputBudget::new(0);
        budget.tokens = 0.0;
        let mut admitted = 0;
        for message in 1..=30 {
            // Two frames every 1/30 s.
            admitted += budget.admit(2, message * SECOND / 30);
        }
        assert!(admitted >= 59, "admitted {admitted}");
    }

    #[test]
    fn a_speed_hacker_is_held_to_real_time() {
        // A client sending 600 frames a second for ten seconds moves at most ~60/s.
        let mut budget = InputBudget::new(0);
        let mut admitted = 0;
        for message in 0..1000_i64 {
            let frames: Vec<InputFrame> = (0..6)
                .map(|i| frame(u32::try_from(message * 6 + i + 1).expect("small"), 1.0))
                .collect();
            let last = u32::try_from(message * 6).expect("small");
            let (accepted, _) = accept_frames(&frames, last, &mut budget, message * SECOND / 100);
            admitted += accepted.len();
        }
        assert!(admitted <= 10 * 60 + 12, "admitted {admitted}");
    }

    #[test]
    fn replayed_and_reordered_frames_are_dropped() {
        let mut budget = InputBudget::new(0);
        let frames = [frame(5, 1.0), frame(4, 1.0), frame(5, 1.0), frame(6, 1.0)];
        let (accepted, dropped) = accept_frames(&frames, 4, &mut budget, 0);
        assert_eq!(
            accepted.iter().map(|f| f.seq).collect::<Vec<_>>(),
            vec![5, 6]
        );
        assert_eq!(dropped, 2);
    }

    #[test]
    fn the_start_barrier_holds_the_party_in() {
        let catalog = Catalog::builtin().expect("catalog");
        let dungeon = &catalog.dungeons()[0];
        let text = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../assets")
                .join(&dungeon.scene),
        )
        .expect("the dungeon's level file is in the repository");
        let scene =
            engine_environment::scene_file::parse(&text, &dungeon.scene).expect("level parses");
        let barriers = barriers_of(&scene, dungeon);
        let closed = LevelGeometry::build(&scene, &barriers);
        let open = LevelGeometry::build(
            &scene,
            &barriers
                .iter()
                .copied()
                .filter(|barrier| barrier.kind != BarrierKind::Start)
                .collect::<Vec<_>>(),
        );
        let spawn = dungeon.spawn_point(0);
        let start = Vec3::new(spawn.position[0], spawn.position[1], spawn.position[2]);
        let run = |level: &LevelGeometry| -> Body {
            let mut body = Body::at(start);
            let mut buffers = QueryBuffers::new();
            // Ten seconds straight ahead, towards -Z and the gate.
            for seq in 1..=600 {
                apply_input(
                    &mut body,
                    &frame(seq, 1.0),
                    &level.rules,
                    &level.world,
                    &mut buffers,
                );
            }
            body
        };
        let held = run(&closed);
        let free = run(&open);
        let barrier_z = dungeon.start_barrier.center[2] + dungeon.start_barrier.half_extents[2];
        assert!(
            held.position.z > barrier_z,
            "held at {:?}, barrier face at z = {barrier_z}",
            held.position
        );
        assert!(
            free.position.z < barrier_z - 5.0,
            "with the barrier open the party walks on: {:?}",
            free.position
        );
    }
}
