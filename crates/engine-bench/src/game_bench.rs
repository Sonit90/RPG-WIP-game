//! Gameplay benchmark: simulation, combat, extraction and culling at five scene sizes.
//!
//! No device, no window, no draws. Everything measured here is CPU work that happens before
//! a single D3D12 call, which is exactly why it can be measured on its own:
//!
//! ```text
//! Simulation tick -> RenderExtractor -> Culler -> RenderList | D3D12
//! ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ ^^^^^^
//! this benchmark                                             not measured here
//! ```
//!
//! GPU frame time and FPS for the same scenes come from running the game itself
//! (`mygame --dummies <n> --frames <n>`), because those are properties of the whole frame
//! including presentation. Mixing the two would measure the swap chain.
//!
//! ```text
//! cargo run --release --bin game_bench
//! cargo run --release --bin game_bench -- --dummies 1000 --ticks 5000
//! ```
//!
//! The process also counts heap allocations while measuring. "Gameplay does not allocate
//! per frame in the steady state" is exactly the kind of claim that quietly stops being
//! true, so it is counted rather than asserted.

use std::alloc::{GlobalAlloc, Layout, System};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use engine_assets::procedural::{DummyVariant, training_dummy};
use engine_core::math::Frustum;
use engine_game::ability::AbilityId;
use engine_game::arena::{self, ArenaConfig};
use engine_game::command::PlayerCommand;
use engine_game::sim::Simulation;
use engine_presentation::camera::ThirdPersonCamera;
use engine_presentation::extract::{RenderExtractor, VisualBindings};
use engine_render_data::{Camera, Projection};
use engine_renderer::Culler;
use engine_renderer::render_list::RenderList;

/// Dummy counts benchmarked when none are given: scenarios A to E.
const DEFAULT_SCENARIOS: [usize; 5] = [1, 10, 100, 500, 1_000];

/// Measured simulation ticks per scenario.
///
/// 3,000 ticks is fifty seconds of gameplay at 60 Hz, which is long enough for cooldowns,
/// burns and the meter to reach a steady state several times over.
const DEFAULT_TICKS: usize = 3_000;

/// Unmeasured ticks before each scenario, so every buffer has reached its high-water mark.
const WARMUP_TICKS: usize = 600;

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

/// What one scenario measured, in microseconds per tick.
struct Measurement {
    dummies: usize,
    entities: usize,
    simulation: Stats,
    combat: Stats,
    extraction: Stats,
    culling: Stats,
    /// Render entities in the world — no longer one per gameplay entity, since a model is
    /// drawn as one render entity per submesh of its selected level of detail.
    drawables: usize,
    visible: u32,
    draw_batches: usize,
    allocations: usize,
    damage: f64,
    dps: f64,
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

fn main() -> ExitCode {
    if let Err(error) = engine_core::logging::init() {
        eprintln!("fatal: failed to install logger: {error}");
        return ExitCode::FAILURE;
    }

    let mut scenarios = Vec::from(DEFAULT_SCENARIOS);
    let mut ticks = DEFAULT_TICKS;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--dummies" => {
                let Some(value) = arguments.next().and_then(|value| value.parse().ok()) else {
                    eprintln!("error: --dummies needs a number");
                    return ExitCode::FAILURE;
                };
                scenarios = vec![value];
            }
            "--ticks" => {
                let Some(value) = arguments.next().and_then(|value| value.parse().ok()) else {
                    eprintln!("error: --ticks needs a number");
                    return ExitCode::FAILURE;
                };
                ticks = value;
            }
            "-h" | "--help" => {
                println!(
                    "usage: game_bench [--dummies <count>] [--ticks <count>]\n\n\
                     Measures the gameplay pipeline with no device: simulation, combat,\n\
                     render extraction and frustum culling."
                );
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("error: unknown option {other:?}");
                return ExitCode::FAILURE;
            }
        }
    }

    match run(&scenarios, ticks) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("fatal: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Runs every scenario and prints the report.
fn run(scenarios: &[usize], ticks: usize) -> Result<(), String> {
    println!("=== GAMEPLAY BENCHMARK ===");
    println!();
    println!("Mode: headless. No device, no window, no draws.");
    println!("Simulation: 60 Hz fixed timestep, one extraction and one cull per tick.");
    println!("Measured ticks per scenario: {ticks} (plus {WARMUP_TICKS} warm-up)");
    println!();

    let mut results = Vec::new();
    for &dummies in scenarios {
        results.push(measure(dummies, ticks)?);
    }

    println!(
        "{:<9} {:>8} {:>9} {:>9} {:>9} {:>9} {:>9} {:>8} {:>8}",
        "dummies",
        "entities",
        "sim us",
        "combat us",
        "extr us",
        "cull us",
        "sim p99",
        "visible",
        "allocs"
    );
    for result in &results {
        println!(
            "{:<9} {:>8} {:>9.3} {:>9.3} {:>9.3} {:>9.3} {:>9.3} {:>8} {:>8}",
            result.dummies,
            result.entities,
            result.simulation.mean,
            result.combat.mean,
            result.extraction.mean,
            result.culling.mean,
            result.simulation.p99,
            result.visible,
            result.allocations
        );
    }
    println!();

    for result in &results {
        report(result, ticks);
    }

    println!("A frame at 300 FPS is 3333 us. The simulation column is per *tick*, and a tick");
    println!("happens 60 times a second, so its cost per frame at 300 FPS is a fifth of it.");
    Ok(())
}

/// Measures one scenario.
fn measure(dummies: usize, ticks: usize) -> Result<Measurement, String> {
    let config = ArenaConfig::with_group(dummies);
    let mut simulation = arena::training_arena(&config);
    let mut extractor = RenderExtractor::new(bindings(), config.arena_half_extent * 2.0);
    let mut culler = Culler::new();
    let mut list = RenderList::new();
    let mut controller = ThirdPersonCamera::following(simulation.player_position());
    let mut camera = Camera::new(Projection::perspective(1920.0 / 1080.0))
        .map_err(|error| format!("the benchmark camera is invalid: {error}"))?;

    // Warm-up: every buffer reaches its high-water mark, the dummies take their first
    // damage, and the meter sees every target at least once.
    for tick in 0..WARMUP_TICKS {
        drive(&mut simulation, tick);
        simulation.step_once();
        extractor.extract(&simulation);
        controller.follow(simulation.player_position(), 1.0 / 60.0);
        controller
            .apply(&mut camera, 1920.0 / 1080.0)
            .map_err(|error| format!("the benchmark camera is invalid: {error}"))?;
        let frustum = Frustum::from_view_projection(&camera.view_projection());
        culler.cull(extractor.render_world(), &frustum, &mut list);
    }

    let mut simulation_samples = Vec::with_capacity(ticks);
    let mut combat_samples = Vec::with_capacity(ticks);
    let mut extraction_samples = Vec::with_capacity(ticks);
    let mut culling_samples = Vec::with_capacity(ticks);

    // Counted across the measured ticks only: building the arena and warming up are
    // expected to allocate, and the question is whether the steady state does. The sample
    // vectors are reserved above so their growth is not attributed to a tick.
    let before = ALLOCATIONS.load(Ordering::Relaxed);

    for tick in 0..ticks {
        drive(&mut simulation, tick + WARMUP_TICKS);

        let started = Instant::now();
        simulation.step_once();
        simulation_samples.push(started.elapsed().as_secs_f64() * 1e6);
        // Everything except movement: abilities, statuses and telemetry. Measured by the
        // simulation itself, which is the same instrumentation the overlay reads.
        let breakdown = simulation.stats().breakdown_ms();
        combat_samples.push((breakdown[1] + breakdown[2] + breakdown[3]) * 1e3);

        let started = Instant::now();
        extractor.extract(&simulation);
        extraction_samples.push(started.elapsed().as_secs_f64() * 1e6);

        controller.follow(simulation.player_position(), 1.0 / 60.0);
        let _ = controller.apply(&mut camera, 1920.0 / 1080.0);
        let frustum = Frustum::from_view_projection(&camera.view_projection());
        let started = Instant::now();
        culler.cull(extractor.render_world(), &frustum, &mut list);
        culling_samples.push(started.elapsed().as_secs_f64() * 1e6);
    }

    let allocations = ALLOCATIONS.load(Ordering::Relaxed) - before;
    list.group_by_material_and_mesh(8, 8);

    Ok(Measurement {
        dummies,
        entities: simulation.world().len(),
        simulation: Stats::of(&mut simulation_samples),
        combat: Stats::of(&mut combat_samples),
        extraction: Stats::of(&mut extraction_samples),
        culling: Stats::of(&mut culling_samples),
        drawables: extractor.render_world().len(),
        visible: culler.stats().visible,
        draw_batches: list.batches().len(),
        allocations,
        damage: simulation.meter().total_damage(),
        dps: simulation.meter().dps(),
    })
}

/// Plays the same rotation on every scenario, so the scenes differ only in size.
///
/// Deterministic and free of wall-clock time: the same tick index always issues the same
/// command, which is what makes two runs of the benchmark comparable measurements rather
/// than two different workloads.
fn drive(simulation: &mut Simulation, tick: usize) {
    // Movement every tick, so the character controller, the camera and the extraction all
    // do real work rather than measuring a static scene.
    let angle = tick as f32 * 0.01;
    simulation.push_command(PlayerCommand::Move {
        forward: angle.cos(),
        right: angle.sin(),
        yaw: angle * 0.25,
        face_camera: false,
    });

    // A rotation that keeps every ability busy: fireball on its cooldown, ignite and
    // detonate on theirs, and a placeholder in between.
    let cast = match tick % 90 {
        0 => Some(0),
        30 => Some(1),
        45 => Some(2),
        60 => Some(3),
        75 => Some(5),
        _ => None,
    };
    if let Some(slot) = cast
        && let Some(ability) = AbilityId::from_slot(slot)
    {
        simulation.push_command(PlayerCommand::UseAbility(ability.slot()));
    }

    // Cycling the target spreads damage across the group rather than deleting one dummy.
    if tick % 15 == 0 {
        simulation.push_command(PlayerCommand::CycleTarget);
    }
    // A reset every ten seconds, so the dummies never all die and the scene stays the size
    // the scenario says it is. Placed mid-cycle rather than at the end, so a run never
    // finishes on a reset and reports a meter that was cleared a tick ago.
    if tick % 600 == 300 {
        simulation.push_command(PlayerCommand::ResetDummies);
    }
}

/// Prints one scenario's detail.
fn report(result: &Measurement, ticks: usize) {
    println!(
        "Scenario: {} dummies ({} gameplay entities, {} drawable)",
        result.dummies, result.entities, result.drawables
    );
    for (name, stats) in [
        ("simulation tick", &result.simulation),
        ("  of which combat", &result.combat),
        ("render extraction", &result.extraction),
        ("frustum culling", &result.culling),
    ] {
        println!(
            "  {name:<18} mean {:>8.3} us   p50 {:>8.3}   p99 {:>8.3}   max {:>8.3}",
            stats.mean, stats.p50, stats.p99, stats.max
        );
    }
    println!(
        "  visible {} of {}, {} draw batches",
        result.visible, result.drawables, result.draw_batches
    );
    println!(
        "  allocations over {ticks} measured ticks: {} ({:.4} per tick)",
        result.allocations,
        result.allocations as f64 / ticks as f64
    );
    println!(
        "  combat produced {:.0} damage at {:.0} DPS",
        result.damage, result.dps
    );
    println!();
}

/// Fabricated handles: this benchmark has no device, and extraction only copies them.
///
/// The handles never reach a renderer, so they name nothing and that is fine — what is
/// being measured is the transform arithmetic and the writes into the `RenderWorld`. The
/// *models* are the real ones, so the number of render entities a dummy costs is the number
/// the game pays.
fn bindings() -> VisualBindings {
    VisualBindings::unbound(
        &training_dummy(DummyVariant::Single),
        &training_dummy(DummyVariant::Group),
    )
}
