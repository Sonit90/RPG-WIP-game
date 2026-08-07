//! Frustum-culling benchmark.
//!
//! Measures [`engine_renderer::Culler::cull`] on its own: no device, no window, no draws.
//! That separation is the point — "is culling fast enough?" is a question about a CPU pass
//! over an array, and answering it inside a frame that also records 17,000 draw calls
//! measures the draw calls.
//!
//! Three scenarios over the same world, because the cost of culling depends on how much
//! survives: a visible object is pushed onto the output buffer, a culled one is not, and a
//! pass that rejects everything has a different branch profile from one that accepts
//! everything.
//!
//! ```text
//! cargo run --release --bin cull_bench
//! cargo run --release --bin cull_bench -- --objects 100000 --iterations 500
//! ```
//!
//! The process also counts heap allocations while measuring. The claim that the cull path
//! does not allocate after its first pass is exactly the kind of claim that quietly stops
//! being true, so it is counted rather than asserted.

use std::alloc::{GlobalAlloc, Layout, System};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use engine_core::math::{Frustum, Sphere, Vec3};
use engine_core::transform::Transform;
use engine_render_data::{MaterialHandle, MeshHandle, RenderEntityDesc, RenderWorld};
use engine_renderer::{Camera, Culler, Projection};

/// Objects benchmarked when none are given.
const DEFAULT_OBJECTS: usize = 100_000;

/// Measured cull passes per scenario.
const DEFAULT_ITERATIONS: usize = 500;

/// Unmeasured passes before each scenario, so the output buffer has reached its high-water
/// mark and the caches are warm.
const WARMUP_ITERATIONS: usize = 20;

/// Counts every allocation the process makes.
///
/// A thin wrapper around the system allocator: the counter is a relaxed atomic add, which
/// costs far less than the allocation it is counting, so it does not distort what it
/// measures.
struct CountingAllocator;

/// Allocations since the process started.
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every method forwards to the system allocator with the same arguments and
// returns its result unchanged, so the allocator contract is upheld by `System`. The
// counter is a side effect that touches no allocation state.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: `layout` came from the caller and is forwarded unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: `pointer` was returned by `alloc` with this `layout`.
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: `pointer` was returned by `alloc` with this `layout`, and `new_size` is
        // the caller's, forwarded unchanged.
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// What fraction of the world a scenario places inside the frustum.
struct Scenario {
    /// Name printed in the report.
    name: &'static str,
    /// Objects placed in front of the camera, as a fraction of the total.
    visible_fraction: f64,
}

/// The three scenarios, chosen because they bracket the interesting range: culling that
/// rejects nothing is pure cost, culling that rejects nearly everything is the case a large
/// world actually hits, and half and half is the mixed-branch case in between.
const SCENARIOS: [Scenario; 3] = [
    Scenario {
        name: "A: all visible",
        visible_fraction: 1.0,
    },
    Scenario {
        name: "B: 99% outside",
        visible_fraction: 0.01,
    },
    Scenario {
        name: "C: 50% visible",
        visible_fraction: 0.5,
    },
];

fn main() -> ExitCode {
    if let Err(error) = engine_core::logging::init() {
        eprintln!("fatal: failed to install logger: {error}");
        return ExitCode::FAILURE;
    }

    let mut objects = DEFAULT_OBJECTS;
    let mut iterations = DEFAULT_ITERATIONS;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--objects" => {
                let Some(value) = arguments.next().and_then(|value| value.parse().ok()) else {
                    eprintln!("error: --objects needs a number");
                    return ExitCode::FAILURE;
                };
                objects = value;
            }
            "--iterations" => {
                let Some(value) = arguments.next().and_then(|value| value.parse().ok()) else {
                    eprintln!("error: --iterations needs a number");
                    return ExitCode::FAILURE;
                };
                iterations = value;
            }
            "-h" | "--help" => {
                println!(
                    "usage: cull_bench [--objects <count>] [--iterations <count>]\n\n\
                     Measures frustum culling on its own: no device, no draws."
                );
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("error: unknown option {other:?}");
                return ExitCode::FAILURE;
            }
        }
    }

    match run(objects, iterations) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("fatal: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Runs every scenario and prints the report.
fn run(objects: usize, iterations: usize) -> Result<(), String> {
    let camera = Camera::new(Projection::perspective(1920.0 / 1080.0))
        .map_err(|error| format!("the benchmark camera is invalid: {error}"))?;
    let frustum = Frustum::from_view_projection(&camera.view_projection());

    println!("frustum culling benchmark");
    println!("objects: {objects}, measured passes per scenario: {iterations}");
    println!(
        "{:<16} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9} {:>8}",
        "scenario", "visible", "culled", "mean us", "p50 us", "p95 us", "p99 us", "ns/obj"
    );

    for scenario in &SCENARIOS {
        let world = build_world(objects, scenario.visible_fraction);
        let mut culler = Culler::new();

        for _ in 0..WARMUP_ITERATIONS {
            culler.cull(&world, &frustum);
        }

        // Counted across the measured passes only: the world build and the warm-up are
        // expected to allocate, and the question is whether the steady state does.
        let before = ALLOCATIONS.load(Ordering::Relaxed);
        let mut samples = Vec::with_capacity(iterations);
        for _ in 0..iterations {
            let started = Instant::now();
            culler.cull(&world, &frustum);
            samples.push(started.elapsed().as_secs_f64() * 1e6);
        }
        let allocations = ALLOCATIONS.load(Ordering::Relaxed) - before;

        let stats = culler.stats();
        let summary = summarize(&mut samples);
        println!(
            "{:<16} {:>9} {:>9} {:>9.1} {:>9.1} {:>9.1} {:>9.1} {:>8.2}",
            scenario.name,
            stats.visible,
            stats.culled,
            summary.mean,
            summary.p50,
            summary.p95,
            summary.p99,
            summary.mean * 1e3 / objects as f64,
        );

        // One allocation for the samples `Vec` growing is this loop's own cost, not the
        // culler's; anything beyond a handful would mean the cull path allocates.
        if allocations > iterations {
            println!(
                "  WARNING: {allocations} heap allocations during {iterations} measured passes"
            );
        }
    }

    // The same world, accepted wholesale, so the cost of the frustum test can be separated
    // from the cost of building the visible list at all.
    let world = build_world(objects, 1.0);
    let mut culler = Culler::new();
    for _ in 0..WARMUP_ITERATIONS {
        culler.accept_all(&world);
    }
    let mut samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let started = Instant::now();
        culler.accept_all(&world);
        samples.push(started.elapsed().as_secs_f64() * 1e6);
    }
    let summary = summarize(&mut samples);
    println!(
        "{:<16} {:>9} {:>9} {:>9.1} {:>9.1} {:>9.1} {:>9.1} {:>8.2}",
        "accept_all",
        culler.stats().visible,
        0,
        summary.mean,
        summary.p50,
        summary.p95,
        summary.p99,
        summary.mean * 1e3 / objects as f64,
    );

    println!(
        "total process allocations: {}",
        ALLOCATIONS.load(Ordering::Relaxed)
    );
    Ok(())
}

/// A world of `objects` entities, `visible_fraction` of them in front of the camera.
///
/// The camera sits at the origin looking down −Z, so "in front" is negative Z within the
/// field of view and "outside" is behind it. Deterministic: the same arguments always
/// produce the same world, or two runs are two different workloads.
fn build_world(objects: usize, visible_fraction: f64) -> RenderWorld {
    let mut world = RenderWorld::with_capacity(objects);
    let visible = (objects as f64 * visible_fraction) as usize;

    for index in 0..objects {
        // A lattice rather than a random cloud: the positions are irrelevant to the cost as
        // long as the inside/outside split is exact, and a lattice makes it exact.
        let step = index as f32;
        let position = if index < visible {
            Vec3::new(
                (step % 41.0) - 20.0,
                ((step / 41.0) % 41.0) - 20.0,
                -30.0 - (step % 17.0),
            )
        } else {
            // Behind the camera: rejected by the near plane, which is the first plane the
            // test evaluates, so this is also the cheapest rejection.
            Vec3::new((step % 41.0) - 20.0, ((step / 41.0) % 41.0) - 20.0, 60.0)
        };

        world.spawn(RenderEntityDesc {
            transform: Transform::from_translation(position),
            mesh: MeshHandle::INVALID,
            material: MaterialHandle::INVALID,
            bounds: Sphere::new(Vec3::ZERO, 0.5),
            visible: true,
        });
    }
    world
}

/// Mean and percentiles of a sample set, in the sample's own units.
struct Summary {
    mean: f64,
    p50: f64,
    p95: f64,
    p99: f64,
}

/// Sorts `samples` in place and summarizes them.
fn summarize(samples: &mut [f64]) -> Summary {
    let mean = samples.iter().sum::<f64>() / samples.len().max(1) as f64;
    samples.sort_by(|a, b| a.total_cmp(b));
    let percentile = |fraction: f64| {
        if samples.is_empty() {
            return 0.0;
        }
        let index = ((samples.len() as f64 - 1.0) * fraction).round() as usize;
        samples[index.min(samples.len() - 1)]
    };
    Summary {
        mean,
        p50: percentile(0.50),
        p95: percentile(0.95),
        p99: percentile(0.99),
    }
}
