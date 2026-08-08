//! Determinism: the same input sequence must produce the same character, every time.
//!
//! ```text
//! input sequence
//!       |
//!       +--> run 1 --> position, velocity, grounded
//!       +--> run 2 --> position, velocity, grounded    all three identical
//!       +--> run 3 --> position, velocity, grounded
//! ```
//!
//! Not "close enough": bit-identical. A replay, a recorded regression case and a future
//! dedicated server all depend on the same run producing the same numbers, and a tolerance
//! here would be a tolerance that grows over a long session.
#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_collision::prelude::*;
use engine_core::math::Vec3;

/// The simulation's fixed step.
const STEP: f32 = 1.0 / 60.0;

/// A course with one of everything: floor, wall, corner, step, ramp and a ledge.
fn course() -> CollisionWorld {
    let mut world = CollisionWorld::with_capacity(16);
    world.add_ground(-20.0, "VOID");
    world.add_box(
        Vec3::new(-20.0, -1.0, -20.0),
        Vec3::new(20.0, 0.0, 20.0),
        "PLATFORM",
    );
    world.add_box(
        Vec3::new(6.0, 0.0, -20.0),
        Vec3::new(7.0, 4.0, 8.0),
        "WALL X",
    );
    world.add_box(
        Vec3::new(-20.0, 0.0, 8.0),
        Vec3::new(7.0, 4.0, 9.0),
        "WALL Z",
    );
    world.add_box(
        Vec3::new(-8.0, 0.0, -4.0),
        Vec3::new(-4.0, 0.35, 4.0),
        "STEP",
    );
    world.add_ramp(
        Obb::ramp(
            Vec3::new(
                -14.0 + 4.0 * 25.0_f32.to_radians().cos(),
                4.0 * 25.0_f32.to_radians().sin(),
                0.0,
            ),
            Vec3::new(4.0, 0.25, 4.0),
            Vec3::Z,
            25.0_f32.to_radians(),
        ),
        "RAMP",
    );
    world.rebuild_broad_phase();
    world
}

/// One tick of scripted input: a horizontal velocity and whether to jump.
#[derive(Debug, Clone, Copy)]
struct Input {
    velocity: Vec3,
    jump: bool,
}

/// A fixed, varied input sequence.
///
/// Derived from the tick number rather than from a random source, so the sequence is part
/// of the test rather than something the test discovers.
fn script(ticks: u32) -> Vec<Input> {
    (0..ticks)
        .map(|tick| {
            let phase = tick % 240;
            let velocity = match phase {
                0..=59 => Vec3::new(6.0, 0.0, 0.0),
                60..=119 => Vec3::new(5.0, 0.0, 5.0),
                120..=179 => Vec3::new(-6.0, 0.0, 2.0),
                _ => Vec3::new(-4.0, 0.0, -6.0),
            };
            Input {
                velocity,
                jump: phase % 37 == 0,
            }
        })
        .collect()
}

/// Runs the script and returns the state after every tick.
fn run(script: &[Input]) -> Vec<CharacterState> {
    let world = course();
    let config = CharacterConfig::default();
    let mut state = CharacterState::at(Vec3::new(0.0, 1.0, 0.0));
    let mut buffers = QueryBuffers::new();
    let mut history = Vec::with_capacity(script.len());

    for input in script {
        if input.jump && state.grounded {
            state.velocity.y = 8.0;
            state.grounded = false;
        }
        state.velocity.x = input.velocity.x;
        state.velocity.z = input.velocity.z;
        state.velocity.y = (state.velocity.y - 25.0 * STEP).max(-60.0);

        let displacement = state.velocity * STEP;
        move_character(&world, &config, &mut state, displacement, &mut buffers);
        history.push(state);
    }
    history
}

#[test]
fn three_runs_of_the_same_input_produce_the_same_character() {
    let script = script(1_200);
    let first = run(&script);
    let second = run(&script);
    let third = run(&script);

    assert_eq!(first.len(), script.len());
    assert_eq!(first, second, "run 2 diverged from run 1");
    assert_eq!(first, third, "run 3 diverged from run 1");

    let last = first.last().copied().expect("the script ran");
    assert!(
        last.position.y > -20.0,
        "it fell out of the world: {last:?}"
    );
}

#[test]
fn divergence_would_be_caught_at_the_tick_it_happened() {
    // A guard on the guard: if two runs differ, the comparison above must actually notice.
    // Here one run is deliberately perturbed by a single tick of extra input.
    let script = script(600);
    let baseline = run(&script);

    let mut perturbed = script.clone();
    perturbed[300].velocity.x += 0.001;
    let other = run(&perturbed);

    assert_ne!(baseline, other, "a perturbed run was indistinguishable");
    let first_difference = baseline
        .iter()
        .zip(&other)
        .position(|(a, b)| a != b)
        .expect("they differ somewhere");
    assert!(
        first_difference >= 300,
        "the runs differed before the perturbation, at tick {first_difference}"
    );
}

#[test]
fn a_world_built_twice_answers_identically() {
    let capsule = CharacterConfig::default().capsule_at(Vec3::new(6.4, 0.0, 0.0));
    let describe = || {
        let world = course();
        let mut buffers = QueryBuffers::new();
        let stats = world.overlap_capsule(&capsule, CollisionMask::WORLD, &mut buffers);
        let contacts: Vec<_> = buffers
            .contacts()
            .iter()
            .map(|hit| (hit.id, hit.contact.normal, hit.contact.penetration))
            .collect();
        (stats, contacts)
    };
    assert_eq!(describe(), describe());
    assert!(!describe().1.is_empty(), "the probe touched something");
}

#[test]
fn the_broad_phase_in_use_does_not_change_the_answer() {
    // The same query against the same geometry, once with a linear scan and once with the
    // grid. Movement resolution walks the contact list, so a different order or a different
    // set is a different final position.
    let small = course();

    let mut large = CollisionWorld::with_capacity(1_024);
    large.add_ground(-20.0, "VOID");
    large.add_box(
        Vec3::new(-20.0, -1.0, -20.0),
        Vec3::new(20.0, 0.0, 20.0),
        "PLATFORM",
    );
    large.add_box(
        Vec3::new(6.0, 0.0, -20.0),
        Vec3::new(7.0, 4.0, 8.0),
        "WALL X",
    );
    large.add_box(
        Vec3::new(-20.0, 0.0, 8.0),
        Vec3::new(7.0, 4.0, 9.0),
        "WALL Z",
    );
    large.add_box(
        Vec3::new(-8.0, 0.0, -4.0),
        Vec3::new(-4.0, 0.35, 4.0),
        "STEP",
    );
    // Far away, and only there to push the world past the grid threshold.
    for index in 0..200 {
        let x = 200.0 + (index % 20) as f32 * 3.0;
        let z = (index / 20) as f32 * 3.0;
        large.add_block(Vec3::new(x, 0.0, z), Vec3::splat(1.0), "FILLER");
    }
    large.rebuild_broad_phase();

    assert_eq!(small.broad_phase().name(), "LINEAR");
    assert_eq!(large.broad_phase().name(), "GRID");

    let config = CharacterConfig::default();
    let mut buffers = QueryBuffers::new();
    for step in 0..40 {
        let capsule = config.capsule_at(Vec3::new(
            5.0 + step as f32 * 0.05,
            0.0,
            step as f32 * 0.2 - 4.0,
        ));

        small.overlap_capsule(&capsule, CollisionMask::WORLD, &mut buffers);
        let scanned: Vec<_> = buffers
            .contacts()
            .iter()
            .map(|hit| (hit.contact.normal, hit.contact.penetration))
            .collect();

        large.overlap_capsule(&capsule, CollisionMask::WORLD, &mut buffers);
        let binned: Vec<_> = buffers
            .contacts()
            .iter()
            .map(|hit| (hit.contact.normal, hit.contact.penetration))
            .collect();

        assert_eq!(
            scanned, binned,
            "the two broad phases disagreed at step {step}"
        );
    }
}

#[test]
fn a_pause_between_ticks_changes_nothing() {
    // The controller has no clock and no state of its own, so splitting a run in two must
    // produce the same result as running it in one go. If that ever stops being true, the
    // controller has grown hidden state.
    let script = script(400);
    let whole = run(&script);

    let world = course();
    let config = CharacterConfig::default();
    let mut state = CharacterState::at(Vec3::new(0.0, 1.0, 0.0));
    let mut history = Vec::with_capacity(script.len());
    for (index, input) in script.iter().enumerate() {
        // A fresh set of buffers every hundred ticks: the scratch space must not carry
        // anything from one move to the next.
        let mut buffers = if index % 100 == 0 {
            QueryBuffers::new()
        } else {
            QueryBuffers::with_capacity(4)
        };

        if input.jump && state.grounded {
            state.velocity.y = 8.0;
            state.grounded = false;
        }
        state.velocity.x = input.velocity.x;
        state.velocity.z = input.velocity.z;
        state.velocity.y = (state.velocity.y - 25.0 * STEP).max(-60.0);

        let displacement = state.velocity * STEP;
        move_character(&world, &config, &mut state, displacement, &mut buffers);
        history.push(state);
    }

    assert_eq!(whole, history, "the buffers carried state between moves");
}
