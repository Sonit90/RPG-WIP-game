//! A parallel particle step has to be the same particles, reached faster.
//!
//! Two things are asserted, and they are different claims. The **integration** must produce
//! bit-identical positions, because a particle that lands a float apart on a machine with
//! more cores is a rendering difference nobody can reproduce. The **compaction** must
//! produce the same *ordering*, because `swap_remove` makes the surviving order a function
//! of the removal sequence, and the draw list is written in that order.
//!
//! No device and no simulation: the pool is plain arrays, which is what lets this run
//! anywhere.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use engine_core::jobs::JobSystem;
use engine_core::math::Vec3;
use engine_render_data::{ParticleBlend, ParticleDrawList};
use engine_vfx::particle::ParticlePool;
use engine_vfx::{NO_OWNER, ParticleSpawn};

/// Big enough that `update_with` actually splits it — below `MIN_TOTAL_PARTICLES` (32,768)
/// the pool takes the serial path and every test here would pass without a worker running.
const PARTICLES: usize = 40_000;

fn spawn_at(index: usize) -> ParticleSpawn {
    let f = index as f32;
    ParticleSpawn {
        position: Vec3::new(f * 0.01, 1.0 + f * 0.002, -f * 0.03),
        velocity: Vec3::new(0.5 - f * 0.001, 2.0, 0.25),
        // A spread of lifetimes, so each step retires a scattered subset rather than a
        // block: a compaction bug shows up as a different survivor order only when the
        // holes are interleaved.
        lifetime: 0.05 + (index % 37) as f32 * 0.01,
        size: (0.2, 0.05),
        color_start: [1.0, 0.8, 0.4, 1.0],
        color_end: [0.6, 0.1, 0.0, 0.0],
        rotation: f * 0.001,
        spin: 0.5,
        gravity: 9.81,
        drag: 0.2,
        blend: if index % 3 == 0 {
            ParticleBlend::Alpha
        } else {
            ParticleBlend::Additive
        },
        // Every fourth particle rides an owner, so the owner-motion branch is exercised on
        // both sides of a chunk boundary.
        owner: if index % 4 == 0 {
            (index % 8) as u32
        } else {
            NO_OWNER
        },
        owner_generation: 1,
    }
}

fn filled_pool() -> ParticlePool {
    let mut pool = ParticlePool::with_capacity(PARTICLES);
    for index in 0..PARTICLES {
        assert!(pool.emit(spawn_at(index)));
    }
    pool
}

fn motion() -> (Vec<Vec3>, Vec<u32>) {
    (vec![Vec3::new(0.01, 0.0, -0.02); 8], vec![1; 8])
}

/// The whole pool as a draw list, which is the only view of it a test can compare.
fn drawn(pool: &ParticlePool) -> ParticleDrawList {
    let mut list = ParticleDrawList::with_capacity(PARTICLES);
    pool.write(&mut list);
    list
}

fn same_lists(actual: &ParticleDrawList, expected: &ParticleDrawList, what: &str) {
    for blend in [ParticleBlend::Additive, ParticleBlend::Alpha] {
        let a = actual.of(blend);
        let b = expected.of(blend);
        assert_eq!(a.len(), b.len(), "{what}: {blend:?} count differs");
        for (index, (left, right)) in a.iter().zip(b).enumerate() {
            assert_eq!(
                (left.position, left.size, left.color, left.rotation),
                (right.position, right.size, right.color, right.rotation),
                "{what}: {blend:?} instance {index} differs"
            );
        }
    }
}

#[test]
fn the_parallel_step_is_identical_to_the_serial_one() {
    let (owner_motion, generations) = motion();

    let mut expected = filled_pool();
    // Several steps, so the second one starts from a pool the first one compacted: a
    // compaction that reordered differently would show up here and not in one step.
    for _ in 0..4 {
        expected.update(0.033, &owner_motion, &generations);
    }
    let expected_list = drawn(&expected);
    assert!(
        expected.len() < PARTICLES && !expected.is_empty(),
        "the fixture must retire some particles and keep some, kept {}",
        expected.len()
    );

    for workers in [1, 2, 4, 8, 16] {
        let Ok(jobs) = JobSystem::with_workers(workers) else {
            panic!("{workers} workers must start");
        };
        let mut actual = filled_pool();
        for _ in 0..4 {
            actual.update_with(0.033, &owner_motion, &generations, Some(&jobs));
        }
        assert_eq!(
            actual.len(),
            expected.len(),
            "{workers} workers kept a different number of particles"
        );
        same_lists(
            &drawn(&actual),
            &expected_list,
            &format!("{workers} workers"),
        );
    }
}

#[test]
fn zero_workers_runs_inline_and_agrees() {
    let (owner_motion, generations) = motion();
    let Ok(jobs) = JobSystem::with_workers(0) else {
        panic!("a job system with no workers must start");
    };

    let mut expected = filled_pool();
    expected.update(0.033, &owner_motion, &generations);

    let mut actual = filled_pool();
    actual.update_with(0.033, &owner_motion, &generations, Some(&jobs));

    assert_eq!(actual.len(), expected.len());
    same_lists(&drawn(&actual), &drawn(&expected), "zero workers");
}

#[test]
fn the_parallel_write_is_identical_to_the_serial_one() {
    let pool = filled_pool();
    let expected = drawn(&pool);

    for workers in [1, 2, 4, 8, 16] {
        let Ok(jobs) = JobSystem::with_workers(workers) else {
            panic!("{workers} workers must start");
        };
        let mut scratch = Vec::new();
        let mut actual = ParticleDrawList::with_capacity(PARTICLES);
        pool.write_with(&mut actual, Some(&jobs), &mut scratch);
        same_lists(&actual, &expected, &format!("write, {workers} workers"));
    }
}

#[test]
fn a_pool_that_shrank_does_not_write_the_previous_frames_particles() {
    // The write scratch is reused across frames. A merge that walked last frame's longer
    // chunks would draw particles that have already expired.
    let Ok(jobs) = JobSystem::with_workers(8) else {
        panic!("8 workers must start");
    };
    let (owner_motion, generations) = motion();
    let mut scratch = Vec::new();

    let mut pool = filled_pool();
    let mut long = ParticleDrawList::with_capacity(PARTICLES);
    pool.write_with(&mut long, Some(&jobs), &mut scratch);

    // Long enough that everything retires.
    pool.update_with(10.0, &owner_motion, &generations, Some(&jobs));
    assert_eq!(pool.len(), 0, "the fixture must empty the pool");

    let mut short = ParticleDrawList::with_capacity(PARTICLES);
    pool.write_with(&mut short, Some(&jobs), &mut scratch);
    assert_eq!(short.len(), 0, "an empty pool must draw nothing");
    assert!(!long.is_empty());
}

#[test]
fn a_small_pool_stays_serial() {
    // The threshold is the point: a combat-sized pool must not pay submission overhead to
    // save nothing. Asserted through the answer — an identical result is what taking the
    // serial path looks like from the outside.
    let (owner_motion, generations) = motion();
    let Ok(jobs) = JobSystem::with_workers(8) else {
        panic!("8 workers must start");
    };

    let mut expected = ParticlePool::with_capacity(4096);
    let mut actual = ParticlePool::with_capacity(4096);
    for index in 0..2600 {
        assert!(expected.emit(spawn_at(index)));
        assert!(actual.emit(spawn_at(index)));
    }
    expected.update(0.033, &owner_motion, &generations);
    actual.update_with(0.033, &owner_motion, &generations, Some(&jobs));

    assert_eq!(actual.len(), expected.len());
    same_lists(&drawn(&actual), &drawn(&expected), "small pool");
}
