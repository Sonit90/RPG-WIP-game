//! What combat's effects cost the CPU, and how they scale with the size of the fight.
//!
//! ```text
//! simulation -> observe events -> VfxDirector::update -> extraction -> cull -> group
//!               ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ measured here
//! ```
//!
//! Its own binary, and no device, for the same reason `game_bench` and `env_bench` are: "is
//! the VFX system fast enough?" is a question about a CPU pass over arrays, and asking it
//! inside a frame that also records thousands of draw calls measures the draw calls. The
//! GPU side of the same question — frame time, draw calls, particle fill — is answered by
//! running the game itself with `--scene vfx --frames N`, whose report line prints both the
//! CPU and the GPU window.
//!
//! ```text
//! cargo run --release --bin vfx_bench
//! cargo run --release --bin vfx_bench -- --frames 500
//! ```
//!
//! Every scenario runs the **real** director against the **real** simulation: the events are
//! the ones combat emits, the radii are the ones the abilities hit with, and the effects are
//! the catalogue's. Nothing is synthesised, because a benchmark that synthesised its own
//! events would measure a system nobody ships.

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::HashMap;
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use engine_core::math::{Sphere, Vec3};
use engine_game::arena::{self, ArenaConfig};
use engine_game::command::PlayerCommand;
use engine_game::firemage::FireAbility;
use engine_game::sim::Simulation;
use engine_presentation::camera::ThirdPersonCamera;
use engine_presentation::extract::{RenderExtractor, ViewParams, VisualBindings};
use engine_presentation::vfx::{VfxDirector, VfxVisuals};
use engine_render_data::{Camera, MaterialHandle, MeshHandle, Projection};
use engine_renderer::culling::Culler;
use engine_renderer::render_list::RenderList;
use engine_vfx::{VfxId, VfxShape, VfxSpawn};

/// Measured frames per scenario when none is given.
const DEFAULT_FRAMES: usize = 1_200;

/// Unmeasured frames before each scenario, so every buffer has reached its high-water mark.
const WARMUP_FRAMES: usize = 300;

/// Benchmark aspect ratio: 1920x1080, the stated target resolution.
const ASPECT: f32 = 1920.0 / 1080.0;

/// Frame delta the director is advanced by, in seconds.
///
/// 1/240: effects are advanced with *real* time and the engine's target is 240 FPS, so this
/// is the step the shipped game takes. A larger step would emit the same particles in fewer,
/// larger batches and quietly flatter the per-frame cost.
const FRAME_DELTA: f32 = 1.0 / 240.0;

/// Counts every allocation the process makes.
struct CountingAllocator;

/// Allocations since the process started.
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every method forwards to the system allocator with the same arguments and returns
// its result unchanged, so the allocator contract is upheld by `System`. The counter is a
// side effect that touches no allocation state.
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

/// What the player does during a scenario.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rotation {
    /// Nothing at all. The baseline: a fight standing still with no effects in it.
    Idle,
    /// The single-target rotation: filler, lance, pyroblast, detonate.
    SingleTarget,
    /// Everything with an area: cone, meteor, firestorm, detonate.
    AreaHeavy,
    /// The area rotation with the Ultimate's effect played on top of it.
    ///
    /// The benchmark drives the effect directly through the director rather than casting
    /// Moving Fire Beam: the cast needs a full Ultimate Energy bar and a ninety-second
    /// cooldown, and what is being measured is what the picture costs, not the gameplay.
    Ultimate,
}

/// One scenario: a number of dummies and what the player does to them.
struct Scenario {
    label: &'static str,
    dummies: usize,
    rotation: Rotation,
    /// Whether effects play at all. The paired "off" run is what makes the VFX cost a
    /// difference between two runs of the same binary rather than a guess.
    vfx: bool,
}

/// Mean and percentiles over a set of samples, in microseconds.
struct Stats {
    mean: f64,
    p50: f64,
    p99: f64,
    max: f64,
}

impl Stats {
    /// Summarizes `samples`, which is sorted in place.
    fn of(samples: &mut [f64]) -> Self {
        if samples.is_empty() {
            return Self {
                mean: 0.0,
                p50: 0.0,
                p99: 0.0,
                max: 0.0,
            };
        }
        let sum: f64 = samples.iter().sum();
        samples.sort_by(f64::total_cmp);
        Self {
            mean: sum / samples.len() as f64,
            p50: engine_core::time::percentile_of_sorted(samples, 0.50),
            p99: engine_core::time::percentile_of_sorted(samples, 0.99),
            max: samples[samples.len() - 1],
        }
    }
}

/// What one scenario measured.
struct Measurement {
    label: &'static str,
    dummies: usize,
    vfx: bool,
    /// Microseconds per frame, by pass.
    simulation: Stats,
    observe: Stats,
    vfx_update: Stats,
    extraction: Stats,
    culling: Stats,
    grouping: Stats,
    total: Stats,
    /// Peaks over the measured frames.
    peak_particles: usize,
    peak_instances: usize,
    peak_mesh_draws: usize,
    mean_particles: f64,
    draw_calls: usize,
    drawables: usize,
    allocations: usize,
    dropped_particles: u64,
    dropped_effects: u64,
}

fn main() -> ExitCode {
    if let Err(error) = engine_core::logging::init() {
        eprintln!("fatal: failed to install logger: {error}");
        return ExitCode::FAILURE;
    }

    let mut frames = DEFAULT_FRAMES;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--frames" => {
                let Some(value) = arguments.next().and_then(|value| value.parse().ok()) else {
                    eprintln!("error: --frames needs a number");
                    return ExitCode::FAILURE;
                };
                frames = value;
            }
            "-h" | "--help" => {
                println!(
                    "usage: vfx_bench [--frames <count>]\n\n\
                     Measures what combat's effects cost the CPU: the event observation, the\n\
                     VFX update, extraction, culling and batching, at 1 to 100 targets and\n\
                     with the area rotation and the Ultimate on top. No device is created."
                );
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("error: unknown option {other:?}");
                return ExitCode::FAILURE;
            }
        }
    }

    run(frames);
    ExitCode::SUCCESS
}

/// Runs every scenario and prints the table.
fn run(frames: usize) {
    let scenarios = [
        Scenario {
            label: "idle, no vfx",
            dummies: 10,
            rotation: Rotation::Idle,
            vfx: false,
        },
        Scenario {
            label: "idle",
            dummies: 10,
            rotation: Rotation::Idle,
            vfx: true,
        },
        Scenario {
            label: "1 target, no vfx",
            dummies: 0,
            rotation: Rotation::SingleTarget,
            vfx: false,
        },
        Scenario {
            label: "1 target",
            dummies: 0,
            rotation: Rotation::SingleTarget,
            vfx: true,
        },
        Scenario {
            label: "10 targets",
            dummies: 9,
            rotation: Rotation::SingleTarget,
            vfx: true,
        },
        Scenario {
            label: "50 targets",
            dummies: 49,
            rotation: Rotation::SingleTarget,
            vfx: true,
        },
        Scenario {
            label: "100 targets",
            dummies: 99,
            rotation: Rotation::SingleTarget,
            vfx: true,
        },
        Scenario {
            label: "aoe-heavy, 50, no vfx",
            dummies: 49,
            rotation: Rotation::AreaHeavy,
            vfx: false,
        },
        Scenario {
            label: "aoe-heavy, 50",
            dummies: 49,
            rotation: Rotation::AreaHeavy,
            vfx: true,
        },
        Scenario {
            label: "ultimate, 50",
            dummies: 49,
            rotation: Rotation::Ultimate,
            vfx: true,
        },
        Scenario {
            label: "worst case, 100",
            dummies: 99,
            rotation: Rotation::Ultimate,
            vfx: true,
        },
    ];

    println!(
        "vfx benchmark: {frames} measured frames per scenario, {WARMUP_FRAMES} warm-up, \
         no device"
    );
    println!(
        "{:<22} {:>5} {:>8} {:>8} {:>8} {:>8} {:>8} {:>9} {:>9} {:>8} {:>7} {:>6} {:>6}",
        "scenario",
        "vfx",
        "sim us",
        "obs us",
        "vfx us",
        "extr us",
        "cull us",
        "total us",
        "p99 us",
        "particle",
        "effect",
        "draws",
        "allocs"
    );

    let mut results = Vec::with_capacity(scenarios.len());
    for scenario in &scenarios {
        let measurement = measure(scenario, frames);
        report(&measurement);
        results.push(measurement);
    }

    println!();
    println!("what the effects cost, as the difference between paired runs:");
    compare(&results, "idle, no vfx", "idle");
    compare(&results, "1 target, no vfx", "1 target");
    compare(&results, "aoe-heavy, 50, no vfx", "aoe-heavy, 50");

    println!();
    println!("peak load per scenario:");
    for result in &results {
        println!(
            "  {:<22} particles peak {:>6} mean {:>8.1} | effects peak {:>3} | mesh draws peak \
             {:>3} | drawables {:>5} | dropped {} particles, {} effects",
            result.label,
            result.peak_particles,
            result.mean_particles,
            result.peak_instances,
            result.peak_mesh_draws,
            result.drawables,
            result.dropped_particles,
            result.dropped_effects,
        );
    }
}

/// Prints the difference the effects make between two runs of the same fight.
fn compare(results: &[Measurement], without: &str, with: &str) {
    let find = |label: &str| results.iter().find(|result| result.label == label);
    let (Some(off), Some(on)) = (find(without), find(with)) else {
        return;
    };
    println!(
        "  {:<22} {:>8.1} us -> {:>8.1} us  (+{:.1} us mean, +{:.1} us p99, +{} draws)",
        with,
        off.total.mean,
        on.total.mean,
        on.total.mean - off.total.mean,
        on.total.p99 - off.total.p99,
        on.draw_calls as i64 - off.draw_calls as i64,
    );
}

/// Runs one scenario and measures it.
fn measure(scenario: &Scenario, frames: usize) -> Measurement {
    let mut simulation = arena::training_arena(&ArenaConfig::with_group(scenario.dummies));
    simulation.step_once();
    simulation.select_nearest_target();

    let mut extractor = RenderExtractor::new(bindings(), 240.0);
    let mut director = VfxDirector::new(vfx_visuals());
    director.set_enabled(scenario.vfx);
    let mut culler = Culler::new();
    let mut list = RenderList::new();
    let mut controller = ThirdPersonCamera::following(simulation.player_position());
    let Ok(mut camera) = Camera::new(Projection::perspective(ASPECT)) else {
        panic!("a 16:9 perspective is valid");
    };

    let mut peak_particles = 0;
    let mut peak_instances = 0;
    let mut peak_mesh_draws = 0;
    let mut particle_sum = 0.0;

    let frame = |index: usize,
                 simulation: &mut Simulation,
                 extractor: &mut RenderExtractor,
                 director: &mut VfxDirector,
                 camera: &mut Camera,
                 controller: &mut ThirdPersonCamera,
                 culler: &mut Culler,
                 list: &mut RenderList|
     -> [f64; 5] {
        drive(simulation, director, scenario.rotation, index);

        let started = Instant::now();
        simulation.step_once();
        let sim = started.elapsed().as_secs_f64() * 1e6;

        let started = Instant::now();
        director.observe(simulation);
        let observe = started.elapsed().as_secs_f64() * 1e6;

        controller.follow(simulation.player_position(), FRAME_DELTA);
        let _ = controller.apply(camera, ASPECT);

        let started = Instant::now();
        director.update(
            simulation,
            FRAME_DELTA,
            extractor.render_world_mut(),
            &engine_presentation::ui::ground_decal::GroundSurface::Plane(0.0),
        );
        let vfx = started.elapsed().as_secs_f64() * 1e6;

        let started = Instant::now();
        extractor.set_view(ViewParams {
            camera_position: camera.position(),
            delta_seconds: FRAME_DELTA,
            lod_bias: 1.0,
        });
        extractor.extract(simulation);
        let extraction = started.elapsed().as_secs_f64() * 1e6;

        let frustum = engine_core::math::Frustum::from_view_projection(&camera.view_projection());
        let started = Instant::now();
        culler.cull(extractor.render_world(), &frustum, list);
        let culling = started.elapsed().as_secs_f64() * 1e6;

        [sim, observe, vfx, extraction, culling]
    };

    for index in 0..WARMUP_FRAMES {
        frame(
            index,
            &mut simulation,
            &mut extractor,
            &mut director,
            &mut camera,
            &mut controller,
            &mut culler,
            &mut list,
        );
    }

    let mut simulation_samples = Vec::with_capacity(frames);
    let mut observe_samples = Vec::with_capacity(frames);
    let mut vfx_samples = Vec::with_capacity(frames);
    let mut extraction_samples = Vec::with_capacity(frames);
    let mut culling_samples = Vec::with_capacity(frames);
    let mut grouping_samples = Vec::with_capacity(frames);
    let mut total_samples = Vec::with_capacity(frames);

    // Counted over the measured frames only: building the arena and warming up are expected
    // to allocate, and the question is whether the steady state does.
    let before = ALLOCATIONS.load(Ordering::Relaxed);

    for index in 0..frames {
        let [sim, observe, vfx, extraction, culling] = frame(
            index + WARMUP_FRAMES,
            &mut simulation,
            &mut extractor,
            &mut director,
            &mut camera,
            &mut controller,
            &mut culler,
            &mut list,
        );

        let started = Instant::now();
        list.group_by_material_and_mesh(MATERIAL_COUNT, MESH_COUNT);
        let grouping = started.elapsed().as_secs_f64() * 1e6;

        let stats = director.stats();
        peak_particles = peak_particles.max(stats.system.particles);
        peak_instances = peak_instances.max(stats.system.instances);
        peak_mesh_draws = peak_mesh_draws.max(stats.system.mesh_draws);
        particle_sum += stats.system.particles as f64;

        simulation_samples.push(sim);
        observe_samples.push(observe);
        vfx_samples.push(vfx);
        extraction_samples.push(extraction);
        culling_samples.push(culling);
        grouping_samples.push(grouping);
        total_samples.push(sim + observe + vfx + extraction + culling + grouping);
    }

    let allocations = ALLOCATIONS.load(Ordering::Relaxed) - before;
    let stats = director.stats();

    Measurement {
        label: scenario.label,
        dummies: scenario.dummies + 1,
        vfx: scenario.vfx,
        simulation: Stats::of(&mut simulation_samples),
        observe: Stats::of(&mut observe_samples),
        vfx_update: Stats::of(&mut vfx_samples),
        extraction: Stats::of(&mut extraction_samples),
        culling: Stats::of(&mut culling_samples),
        grouping: Stats::of(&mut grouping_samples),
        total: Stats::of(&mut total_samples),
        peak_particles,
        peak_instances,
        peak_mesh_draws,
        mean_particles: particle_sum / frames as f64,
        draw_calls: list.batches().len(),
        drawables: extractor.render_world().len(),
        allocations,
        dropped_particles: stats.system.dropped_particles,
        dropped_effects: stats.system.dropped_instances + stats.refused,
    }
}

/// Prints one row of the table.
fn report(result: &Measurement) {
    println!(
        "{:<22} {:>5} {:>8.1} {:>8.1} {:>8.1} {:>8.1} {:>8.1} {:>9.1} {:>9.1} {:>8} {:>7} {:>6} \
         {:>6}",
        result.label,
        if result.vfx { "on" } else { "off" },
        result.simulation.mean,
        result.observe.mean,
        result.vfx_update.mean,
        result.extraction.mean,
        result.culling.mean,
        result.total.mean,
        result.total.p99,
        result.peak_particles,
        result.peak_instances,
        result.draw_calls,
        result.allocations,
    );
    let _ = result.dummies;
    let _ = result.grouping.p50;
    let _ = result.simulation.max;
}

/// Presses what the scenario's rotation presses.
///
/// Frame-indexed rather than random, so two runs of the same scenario do the same fight and
/// a difference between them is the code rather than the dice.
fn drive(
    simulation: &mut Simulation,
    director: &mut VfxDirector,
    rotation: Rotation,
    frame: usize,
) {
    if matches!(rotation, Rotation::Idle) {
        return;
    }
    let target = simulation
        .player()
        .target
        .and_then(|target| simulation.world().position(target));

    // A press every eighth frame at 240 FPS is one every 33 ms — faster than the global
    // cooldown, so the class is always doing the most it is allowed to.
    if frame % 8 != 0 {
        return;
    }

    match rotation {
        Rotation::Idle => {}
        Rotation::SingleTarget => {
            let slot = match (frame / 8) % 4 {
                0 => 0,
                1 => 1,
                2 => 4,
                _ => 6,
            };
            simulation.push_command(PlayerCommand::UseAbility(slot));
        }
        Rotation::AreaHeavy | Rotation::Ultimate => {
            let Some(position) = target else {
                return;
            };
            match (frame / 8) % 4 {
                0 => {
                    let _ = simulation.cast_at(FireAbility::FlameCone, position);
                }
                1 => {
                    let _ = simulation.cast_at(FireAbility::Meteor, position);
                }
                2 => {
                    let _ = simulation.cast_at(FireAbility::Firestorm, position);
                }
                _ => simulation.push_command(PlayerCommand::UseAbility(6)),
            }
            // The Ultimate's effect is played directly, at the size the beam really is.
            if matches!(rotation, Rotation::Ultimate) && (frame / 8) % 16 == 0 {
                director.system_mut().spawn(
                    VfxSpawn::at(VfxId::FireBeam, position)
                        .with_radius(engine_game::firemage::BEAM_WIDTH)
                        .with_length(engine_game::firemage::BEAM_LENGTH)
                        .raised(1.3)
                        .lasting(1.0),
                );
            }
        }
    }
}

/// Materials the benchmark's grouping pass is told the renderer holds.
///
/// The arena's handful plus the VFX system's one. The number only has to bound the handles
/// the list actually contains; a larger one costs the counting sort a bigger histogram.
const MATERIAL_COUNT: usize = 16;

/// Meshes the benchmark's grouping pass is told the renderer holds.
const MESH_COUNT: usize = 24;

/// The handles the extractor draws gameplay entities with.
///
/// Fabricated: nothing is uploaded, and no pass here dereferences a handle. What matters is
/// that two things sharing a mesh and a material share them here too, so the grouping pass
/// produces the batches the real frame would.
fn bindings() -> VisualBindings {
    let dummy =
        engine_assets::procedural::training_dummy(engine_assets::procedural::DummyVariant::Single);
    let group =
        engine_assets::procedural::training_dummy(engine_assets::procedural::DummyVariant::Group);
    VisualBindings::unbound(&dummy, &group)
}

/// The six shapes and one material every effect is drawn with, over handles nothing
/// uploaded.
fn vfx_visuals() -> VfxVisuals {
    let mut visuals = VfxVisuals::new(MaterialHandle::new(9, 1));
    for (index, shape) in VfxShape::ALL.into_iter().enumerate() {
        visuals.bind(
            shape,
            MeshHandle::new(12 + index as u32, 1),
            Sphere::new(Vec3::ZERO, 1.0),
        );
    }
    visuals
}

/// Kept so the unused-import warning does not hide a real one if the table changes.
#[allow(dead_code, reason = "documents the counts the grouping pass is given")]
fn counts() -> HashMap<&'static str, usize> {
    HashMap::from([("materials", MATERIAL_COUNT), ("meshes", MESH_COUNT)])
}
