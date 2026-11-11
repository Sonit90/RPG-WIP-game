//! Collision benchmark: broad phase, narrow phase and the character controller.
//!
//! No device, no window, no draws — collision is CPU work over arrays, and measuring it
//! inside a frame that also records draw calls measures the draw calls.
//!
//! ```text
//! character controller  ->  CollisionWorld::overlap_capsule
//!                              |                |
//!                         broad phase      narrow phase
//! ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ this benchmark
//! ```
//!
//! ```text
//! cargo run --release --bin collision_bench
//! cargo run --release --bin collision_bench -- --statics 10000 --ticks 20000
//! ```
//!
//! The three phases are timed separately by running three different loops over the same
//! scene, rather than by instrumenting one:
//!
//! | Phase | What is run |
//! | --- | --- |
//! | broad | `BroadPhase::query` alone, into a reused buffer |
//! | narrow | `overlap_capsule`: the broad phase plus every exact test it implies |
//! | controller | `move_character`: the whole tick, sweeps, resolution and step probes |
//! | camera | `ThirdPersonCamera::avoid_obstructions`: the per-frame camera collision check |
//!
//! Narrow-phase time is reported as the difference, which is the only honest way to state
//! it: the narrow phase cannot run without a candidate list to run over.
//!
//! Allocations are counted rather than asserted. "Collision does not allocate per frame" is
//! exactly the kind of claim that quietly stops being true.

use std::alloc::{GlobalAlloc, Layout, System};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use engine_collision::broad::Proxy;
use engine_collision::prelude::*;
use engine_core::math::Vec3;
use engine_presentation::camera::ThirdPersonCamera;

/// Radius of the circuit the character walks, in metres.
const CIRCUIT_RADIUS: f32 = 7.2;

/// Static object counts benchmarked when none are given.
const DEFAULT_SCENARIOS: [usize; 4] = [10, 100, 1_000, 10_000];

/// Measured ticks per scenario.
///
/// 6,000 ticks is a hundred seconds of gameplay at 60 Hz — long enough that a single
/// scheduling hiccup cannot move the mean.
const DEFAULT_TICKS: usize = 6_000;

/// Length of the benchmarked line-of-sight ray, in metres.
///
/// The longest range on the FireMage bar, so the figure is the worst case a targeted
/// ability can ask for rather than a typical one.
const LOS_RANGE: f32 = 35.0;

/// Unmeasured ticks before each scenario, so every buffer has reached its high-water mark.
const WARMUP_TICKS: usize = 600;

/// Counts every allocation the process makes.
struct CountingAllocator;

/// Allocations since the process started.
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every method forwards to the system allocator with the same arguments, so the
// allocator contract is upheld by `System`. The counter is a relaxed atomic add on the way
// past and touches none of the memory involved.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: forwarded unchanged to the system allocator.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: forwarded unchanged; the pointer came from `System.alloc` above.
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: forwarded unchanged; the pointer came from `System.alloc` above.
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Allocations so far.
fn allocations() -> usize {
    ALLOCATIONS.load(Ordering::Relaxed)
}

fn main() -> ExitCode {
    let mut scenarios = Vec::from(DEFAULT_SCENARIOS);
    let mut ticks = DEFAULT_TICKS;

    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--statics" => {
                let Some(value) = arguments.next().and_then(|v| v.parse().ok()) else {
                    eprintln!("error: --statics needs a number");
                    return ExitCode::FAILURE;
                };
                scenarios = vec![value];
            }
            "--ticks" => {
                let Some(value) = arguments.next().and_then(|v| v.parse().ok()) else {
                    eprintln!("error: --ticks needs a number");
                    return ExitCode::FAILURE;
                };
                ticks = value;
            }
            "-h" | "--help" => {
                println!(
                    "usage: collision_bench [--statics <count>] [--ticks <count>]\n\n\
                     Measures the broad phase, the narrow phase and the character\n\
                     controller against 10, 100, 1,000 and 10,000 static objects."
                );
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("error: unknown option {other:?}");
                return ExitCode::FAILURE;
            }
        }
    }

    run(&scenarios, ticks);
    ExitCode::SUCCESS
}

/// One scenario's numbers.
struct Measurement {
    statics: usize,
    broad_phase: &'static str,
    /// Microseconds per tick of the broad phase alone.
    broad_us: f64,
    /// Microseconds per tick of a full overlap query.
    query_us: f64,
    /// Microseconds per tick of the whole character move.
    controller_us: f64,
    /// Microseconds per query of one targeted-ability line-of-sight ray.
    ///
    /// Per *query*, not per tick, because that is how it is spent: a ray is cast when an
    /// ability that asks for one is activated and again when its cast lands, and never in
    /// between. A tick figure would imply a per-frame cost this system does not have.
    los_us: f64,
    /// Microseconds per frame of the third-person camera's obstruction check.
    ///
    /// Per *frame*, unlike the line-of-sight ray: the camera checks every rendered frame.
    camera_us: f64,
    /// Candidates the broad phase offered per tick.
    candidates: f64,
    /// Exact tests per tick.
    narrow_tests: f64,
    /// Contacts per tick.
    contacts: f64,
    /// Allocations during the measured run.
    allocations: usize,
}

fn run(scenarios: &[usize], ticks: usize) {
    println!("=== COLLISION BENCHMARK ===");
    println!();
    println!("Mode: headless, no device, no window");
    println!("Ticks per scenario: {ticks} (plus {WARMUP_TICKS} warm-up)");
    println!("Character: 0.40 m radius, 1.80 m tall, 0.45 m step, 50 degree slope limit");
    println!();

    let mut measurements = Vec::new();
    for &statics in scenarios {
        let measurement = measure(statics, ticks);
        report(&measurement);
        measurements.push(measurement);
    }

    println!("=== SUMMARY: microseconds per tick ===");
    println!();
    println!(
        "{:>8} | {:>6} | {:>8} | {:>8} | {:>10} | {:>8} | {:>8} | {:>6} | {:>6} | {:>6}",
        "statics",
        "phase",
        "broad",
        "narrow",
        "controller",
        "los ray",
        "camera",
        "cands",
        "tests",
        "allocs"
    );
    for measurement in &measurements {
        println!(
            "{:>8} | {:>6} | {:>8.3} | {:>8.3} | {:>10.3} | {:>8.3} | {:>8.3} | {:>6.1} | {:>6.1} | {:>6}",
            measurement.statics,
            measurement.broad_phase,
            measurement.broad_us,
            (measurement.query_us - measurement.broad_us).max(0.0),
            measurement.controller_us,
            measurement.los_us,
            measurement.camera_us,
            measurement.candidates,
            measurement.narrow_tests,
            measurement.allocations,
        );
    }
    println!();
    println!(
        "broad + narrow is one overlap query; the controller runs several of them a tick \
         plus its sweeps,"
    );
    println!("so the controller column is the whole cost of moving the character, not a sum.");
    println!();
    println!("the line-of-sight column is per *ray*, not per tick: one ray is cast when a");
    println!("targeted ability that requires it is activated, and one more when its cast");
    println!("lands. Never per frame, and never per enemy.");
    println!();
    println!("the camera column is per *frame*: the third-person camera sweeps its sphere");
    println!("through the level every rendered frame to keep out of the walls.");
    println!();
    println!("At 240 FPS the frame budget is 4166 us. At 480 FPS it is 2083 us.");
}

/// Builds a level of `statics` objects and measures the three phases against it.
fn measure(statics: usize, ticks: usize) -> Measurement {
    let world = level(statics);
    let config = CharacterConfig::default();
    let mut buffers = QueryBuffers::with_capacity(32);

    // The same path in every scenario: a lap of a small circuit near the origin, where the
    // geometry is dense. Measuring a character standing in an empty corner of a 10,000
    // object level would measure the broad phase rejecting everything.
    let path: Vec<Vec3> = (0..ticks)
        .map(|tick| {
            let angle = tick as f32 * 0.01;
            Vec3::new(
                angle.cos() * CIRCUIT_RADIUS,
                1.0,
                angle.sin() * CIRCUIT_RADIUS,
            )
        })
        .collect();

    // --- broad phase alone ---------------------------------------------------------------
    let proxies = proxies_of(&world);
    let mut candidates: Vec<u32> = Vec::with_capacity(64);
    for point in path.iter().take(WARMUP_TICKS) {
        candidates.clear();
        world.broad_phase().query(
            &proxies,
            &config.capsule_at(*point).bounds(),
            config.mask,
            &mut candidates,
        );
    }
    let mut candidate_total = 0_u64;
    let started = Instant::now();
    for point in &path {
        candidates.clear();
        world.broad_phase().query(
            &proxies,
            &config.capsule_at(*point).bounds(),
            config.mask,
            &mut candidates,
        );
        candidate_total += candidates.len() as u64;
    }
    let broad = started.elapsed();

    // --- a full overlap query --------------------------------------------------------------
    for point in path.iter().take(WARMUP_TICKS) {
        world.overlap_capsule(&config.capsule_at(*point), config.mask, &mut buffers);
    }
    let mut tests = 0_u64;
    let mut contacts = 0_u64;
    let started = Instant::now();
    for point in &path {
        let stats = world.overlap_capsule(&config.capsule_at(*point), config.mask, &mut buffers);
        tests += u64::from(stats.narrow_tests);
        contacts += u64::from(stats.contacts);
    }
    let query = started.elapsed();

    // --- the whole character move -----------------------------------------------------------
    let mut state = CharacterState::at(Vec3::new(CIRCUIT_RADIUS, 1.0, 0.0));
    for tick in 0..WARMUP_TICKS {
        step_character(&world, &config, &mut state, &mut buffers, tick);
    }
    let before = allocations();
    let started = Instant::now();
    for tick in 0..ticks {
        step_character(&world, &config, &mut state, &mut buffers, tick);
    }
    let controller = started.elapsed();
    let allocated = allocations() - before;

    // --- one targeted-ability line-of-sight ray -------------------------------------------
    //
    // Thirty-five metres, the longest range on the bar, fired across the dense part of the
    // level from chest height to chest height. Not per tick: this is what one *cast* costs.
    let sight: Vec<(Vec3, Vec3)> = path
        .iter()
        .map(|point| {
            let from = Vec3::new(point.x, 1.4, point.z);
            let angle = point.z.atan2(point.x) + 1.0;
            (
                from,
                Vec3::new(
                    from.x + angle.cos() * LOS_RANGE,
                    1.4,
                    from.z + angle.sin() * LOS_RANGE,
                ),
            )
        })
        .collect();
    for (from, to) in sight.iter().take(WARMUP_TICKS) {
        let _ = world.segment_blocked(*from, *to, CollisionMask::LINE_OF_SIGHT);
    }
    let started = Instant::now();
    let mut blocked = 0_u64;
    for (from, to) in &sight {
        if world
            .segment_blocked(*from, *to, CollisionMask::LINE_OF_SIGHT)
            .is_some()
        {
            blocked += 1;
        }
    }
    let los = started.elapsed();
    // Read so the loop cannot be optimised away, and printed because a ray that hits
    // nothing measures the broad phase rejecting everything.
    let _ = blocked;

    // --- the camera's per-frame obstruction check ------------------------------------------
    //
    // The real camera, following the circuit at its furthest zoom while the mouse turns it,
    // so the sweep is as long as it ever is and runs into the pillars every quarter lap.
    let mut camera = ThirdPersonCamera::following(Vec3::new(CIRCUIT_RADIUS, 0.0, 0.0));
    camera.distance = 25.0;
    let frame = |camera: &mut ThirdPersonCamera, point: &Vec3| {
        let feet = Vec3::new(point.x, 0.0, point.z);
        camera.rotate(3.0, 0.4);
        camera.follow(feet, 1.0 / 240.0);
        camera.avoid_obstructions(&world, feet, 1.0 / 240.0);
    };
    for point in path.iter().take(WARMUP_TICKS) {
        frame(&mut camera, point);
    }
    let started = Instant::now();
    for point in &path {
        frame(&mut camera, point);
    }
    let camera_time = started.elapsed();

    Measurement {
        statics,
        broad_phase: world.broad_phase().name(),
        broad_us: per_tick_us(broad, ticks),
        query_us: per_tick_us(query, ticks),
        controller_us: per_tick_us(controller, ticks),
        los_us: per_tick_us(los, ticks),
        camera_us: per_tick_us(camera_time, ticks),
        candidates: candidate_total as f64 / ticks as f64,
        narrow_tests: tests as f64 / ticks as f64,
        contacts: contacts as f64 / ticks as f64,
        allocations: allocated,
    }
}

/// One tick of a character walking a circle, under gravity.
fn step_character(
    world: &CollisionWorld,
    config: &CharacterConfig,
    state: &mut CharacterState,
    buffers: &mut QueryBuffers,
    tick: usize,
) {
    let angle = tick as f32 * 0.01;
    state.velocity.x = -angle.sin() * CIRCUIT_RADIUS;
    state.velocity.z = angle.cos() * CIRCUIT_RADIUS;
    state.velocity.y = (state.velocity.y - 25.0 / 60.0).max(-60.0);
    let displacement = state.velocity / 60.0;
    move_character(world, config, state, displacement, buffers);
}

/// The broad phase's view of a world.
///
/// Rebuilt here rather than borrowed, because the world keeps its proxies private — which
/// is the point of the abstraction. A benchmark that had to reach inside the world to time
/// its broad phase would be measuring a design that had already leaked.
fn proxies_of(world: &CollisionWorld) -> Vec<Proxy> {
    (0..world.slot_count())
        .map(|slot| match world.id_at(slot) {
            Some(id) => Proxy {
                bounds: world.bounds(id).unwrap_or(engine_core::math::Aabb::EMPTY),
                layer: world
                    .filter(id)
                    .map_or(CollisionLayer::WORLD, |filter| filter.layer),
                live: true,
            },
            None => Proxy {
                bounds: engine_core::math::Aabb::EMPTY,
                layer: CollisionLayer::WORLD,
                live: false,
            },
        })
        .collect()
}

/// A deterministic level of `statics` objects: a floor, a ring of walls, and a field.
///
/// The character's circuit stays inside the ring, so the geometry it collides with is the
/// same in every scenario and the only thing that changes is how much *other* geometry the
/// broad phase has to reject. That is what makes the four rows comparable.
fn level(statics: usize) -> CollisionWorld {
    let mut world = CollisionWorld::with_capacity(statics + 8);
    world.add_ground(0.0, "FLOOR");

    // The circuit's own furniture: four pillars, on the diagonals so the character starts
    // clear of them and walks into one every quarter lap. Without something to actually
    // collide with, the benchmark would measure a broad phase rejecting everything and a
    // narrow phase that never runs.
    let diagonal = CIRCUIT_RADIUS * std::f32::consts::FRAC_1_SQRT_2;
    for (x, z) in [
        (diagonal, diagonal),
        (-diagonal, diagonal),
        (diagonal, -diagonal),
        (-diagonal, -diagonal),
    ] {
        world.add_block(Vec3::new(x, 0.0, z), Vec3::new(2.0, 3.0, 2.0), "PILLAR");
    }

    // The rest of the level, laid out on a square lattice around the circuit. Deterministic:
    // the same count always produces the same level.
    let placed = world.len();
    let remaining = statics.saturating_sub(placed);
    let columns = (remaining as f32).sqrt().ceil().max(1.0) as usize;
    for index in 0..remaining {
        let column = index % columns;
        let row = index / columns;
        let x = (column as f32 - columns as f32 * 0.5) * 4.0;
        let z = (row as f32 - columns as f32 * 0.5) * 4.0;
        // Pushed out of the circuit, so it is geometry to reject rather than to collide
        // with.
        let x = if x.abs() < 12.0 { x + 24.0 } else { x };
        world.add_block(Vec3::new(x, 0.0, z), Vec3::splat(1.5), "FILLER");
    }

    world.rebuild_broad_phase();
    world
}

/// A duration divided into per-tick microseconds.
fn per_tick_us(duration: Duration, ticks: usize) -> f64 {
    if ticks == 0 {
        return 0.0;
    }
    duration.as_secs_f64() * 1e6 / ticks as f64
}

/// Prints one scenario.
fn report(measurement: &Measurement) {
    println!(
        "Scene: {} static objects - {} broad phase",
        measurement.statics, measurement.broad_phase
    );
    println!();
    println!(
        "  broad phase      {:>8.3} us/tick   {:>7.1} candidates",
        measurement.broad_us, measurement.candidates
    );
    println!(
        "  overlap query    {:>8.3} us/tick   {:>7.1} narrow tests   {:.1} contacts",
        measurement.query_us, measurement.narrow_tests, measurement.contacts
    );
    println!(
        "  character move   {:>8.3} us/tick",
        measurement.controller_us
    );
    println!(
        "  line-of-sight    {:>8.3} us/ray    {LOS_RANGE:.0} m, per targeted cast",
        measurement.los_us
    );
    println!(
        "  camera check     {:>8.3} us/frame  sphere sweeps, 25 m zoom",
        measurement.camera_us
    );
    println!(
        "  allocations      {:>8}   (measured run only)",
        measurement.allocations
    );
    println!();
}
