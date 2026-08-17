//! UI benchmark: layout, batching and drawing at six interface sizes.
//!
//! ```text
//! GameUi layout -> UiDrawList -> UiBatcher -> vertices | upload -> one draw -> GPU
//! ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^   ^^^^^^^^^^^^^^^^^^^^^^^^
//! CPU phase, no device                                  GPU phase, hidden window
//! ```
//!
//! Two phases, because the two questions are different. "How long does it take to lay out
//! and batch N elements?" is a CPU pass over arrays and is measured with no device at all,
//! thousands of times, so its distribution is meaningful. "What does drawing them cost?" is
//! a property of a submitted frame and needs a real swap chain, so it is measured against a
//! hidden window with the GPU's own timestamps.
//!
//! The scene is **deterministic**: element positions and contents come from the index, not
//! from a clock or a random number, so two runs measure the same work.
//!
//! ```text
//! cargo run --release --bin ui_bench
//! cargo run --release --bin ui_bench -- --elements 2000 --iterations 20000
//! ```
//!
//! Percentiles rather than means alone: the Phase 3 audit found a UI whose mean was
//! comfortable and whose p95 was four times its median, and a mean would have hidden it.

use std::alloc::{GlobalAlloc, Layout, System};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use engine_core::math::Vec3;
use engine_game::arena::{self, ArenaConfig};
use engine_game::command::PlayerCommand;
use engine_platform::{Window, WindowConfig};
use engine_presentation::camera::ThirdPersonCamera;
use engine_presentation::damage_numbers::FloatingNumbers;
use engine_presentation::game_ui::GameUi;
use engine_presentation::nameplate::{NameplateConfig, NameplateSet};
use engine_presentation::snapshot::UiSnapshot;
use engine_render_data::ui::{TextAlign, UiDrawList, UiRect};
use engine_render_data::{Camera, Projection};
use engine_renderer::{Renderer, RendererConfig, UiBatcher};

/// Element counts benchmarked when none are given.
const DEFAULT_SCENARIOS: [usize; 6] = [20, 50, 100, 250, 500, 1_000];

/// Measured CPU iterations per scenario.
const DEFAULT_ITERATIONS: usize = 5_000;

/// Unmeasured iterations before each scenario, so every buffer has reached its high-water
/// mark and the allocation count describes the steady state rather than the warm-up.
const WARMUP_ITERATIONS: usize = 500;

/// Frames rendered per scenario in the GPU phase.
/// One frame of real time, for the nameplate layout's smoothing.
const FRAME: f32 = 1.0 / 60.0;

const GPU_FRAMES: usize = 600;

/// Frames rendered before measurement starts.
const GPU_WARMUP_FRAMES: usize = 60;

/// Client size the GPU phase renders at, matching the project's target resolution.
const WIDTH: u32 = 1920;
/// Client height.
const HEIGHT: u32 = 1080;

/// Counts every allocation the process makes.
///
/// The same wrapper `game_bench` uses: a relaxed atomic add, far cheaper than the
/// allocation it counts.
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

/// Mean and percentiles over a set of samples, in microseconds.
struct Stats {
    mean: f64,
    p50: f64,
    p95: f64,
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
                p95: 0.0,
                p99: 0.0,
                max: 0.0,
            };
        }
        let sum: f64 = samples.iter().sum();
        samples.sort_by(f64::total_cmp);
        Self {
            mean: sum / samples.len() as f64,
            p50: engine_core::time::percentile_of_sorted(samples, 0.50),
            p95: engine_core::time::percentile_of_sorted(samples, 0.95),
            p99: engine_core::time::percentile_of_sorted(samples, 0.99),
            max: samples[samples.len() - 1],
        }
    }
}

/// What one scenario measured.
struct Measurement {
    elements: usize,
    commands: usize,
    quads: usize,
    vertices: usize,
    /// Layout: building the draw list.
    layout: Stats,
    /// Batching: turning commands into vertices.
    batch: Stats,
    /// Layout plus batching, which is the whole CPU cost of the interface.
    total: Stats,
    allocations: usize,
    /// GPU milliseconds for a frame containing this UI, and the same frame without it.
    gpu: Option<GpuMeasurement>,
}

/// What the GPU phase measured, in milliseconds.
struct GpuMeasurement {
    with_ui: Stats,
    without_ui: Stats,
    draw_calls_with_ui: u32,
    draw_calls_without_ui: u32,
}

/// Builds a deterministic interface of `elements` labelled bars.
///
/// A labelled bar — panel, bar background, bar fill, name, value — is what the real HUD is
/// made of, so the benchmark scales the thing the game actually draws rather than a
/// rectangle nobody would submit.
fn build_ui(list: &mut UiDrawList, elements: usize, frame: usize) {
    list.clear();
    for index in 0..elements {
        // Position and fill derive from the index and the frame number, so the work is
        // identical between runs but not constant-folded away.
        let column = (index % 8) as f32;
        let row = (index / 8) as f32;
        let x = 20.0 + column * 230.0;
        let y = 20.0 + row * 22.0;
        let fraction = ((index + frame) % 100) as f32 / 100.0;

        let panel = UiRect::new(x, y, 220.0, 20.0);
        list.rect(panel, [0.05, 0.06, 0.08, 0.72]);
        list.bar(
            UiRect::new(x + 2.0, y + 2.0, 216.0, 8.0),
            fraction,
            [0.10, 0.10, 0.12, 0.85],
            [0.80, 0.25, 0.22, 1.0],
        );
        list.text_at(
            (x + 4.0, y + 11.0),
            "TRAINING DUMMY",
            [0.95, 0.95, 0.85, 1.0],
            1.0,
        );
        list.text_aligned(
            (x + 216.0, y + 11.0),
            "100%",
            [0.95, 0.95, 0.85, 1.0],
            1.0,
            TextAlign::Right,
        );
        if index % 4 == 0 {
            list.outline(panel, 1.0, [1.0, 1.0, 1.0, 1.0]);
        }
    }
}

/// Measures the *real* combat HUD: the arena, its nameplates, the target frame, the player
/// bars, the ability bar, the cast bar and a full set of floating damage numbers.
///
/// The synthetic scenarios above scale one element type; this one measures what the game
/// actually submits, which is the number an acceptance criterion should be stated against.
fn measure_combat(dummies: usize, iterations: usize) {
    let mut simulation = arena::training_arena(&ArenaConfig::with_group(dummies));
    let mut controller = ThirdPersonCamera::following(simulation.player_position());
    let mut camera = match Camera::new(Projection::perspective(WIDTH as f32 / HEIGHT as f32)) {
        Ok(camera) => camera,
        Err(error) => {
            eprintln!("SKIP: camera: {error}");
            return;
        }
    };
    controller.follow(simulation.player_position(), 0.016);
    if controller
        .apply(&mut camera, WIDTH as f32 / HEIGHT as f32)
        .is_err()
    {
        eprintln!("SKIP: the camera could not be applied");
        return;
    }

    // Fill the arena with combat: burn everything down so health bars, statuses and damage
    // numbers are all populated rather than sitting at their cheap default.
    for tick in 0..600 {
        if tick % 20 == 0 {
            simulation.push_command(PlayerCommand::CycleTarget);
        }
        if tick % 9 == 0 {
            simulation.push_command(PlayerCommand::UseAbility(tick / 9 % 3));
        }
        simulation.step_once();
    }

    let config = NameplateConfig::default();
    let mut plates = NameplateSet::with_capacity(32);
    let mut ui = GameUi::new();
    let mut numbers = FloatingNumbers::new();
    let mut snapshot = UiSnapshot::build(&simulation);
    let view_projection = camera.view_projection();

    // A full set of damage numbers: the worst case a large area hit produces.
    let now = simulation.time() as f32;
    for index in 0..engine_presentation::MAX_FLOATING_NUMBERS {
        numbers.push(
            simulation.player().entity,
            Vec3::new((index % 8) as f32, 1.0, -((index / 8) as f32)),
            (index * 7 % 400) as f32 + 1.0,
            index % 20 == 0,
            index % 3 == 0,
            now,
        );
    }

    let mut batcher = UiBatcher::new(1 << 20);
    for _ in 0..WARMUP_ITERATIONS {
        plates.build(
            &simulation,
            &view_projection,
            (WIDTH, HEIGHT),
            &config,
            FRAME,
        );
        snapshot.refresh(&simulation, None);
        ui.build(&snapshot, plates.plates(), &config, (WIDTH, HEIGHT));
        ui.push_damage_numbers(&numbers, &view_projection, (WIDTH, HEIGHT), now);
        batcher.build(ui.list());
    }

    let mut samples = Vec::with_capacity(iterations);
    let before = ALLOCATIONS.load(Ordering::Relaxed);
    for _ in 0..iterations {
        let started = Instant::now();
        plates.build(
            &simulation,
            &view_projection,
            (WIDTH, HEIGHT),
            &config,
            FRAME,
        );
        snapshot.refresh(&simulation, None);
        ui.build(&snapshot, plates.plates(), &config, (WIDTH, HEIGHT));
        ui.push_damage_numbers(&numbers, &view_projection, (WIDTH, HEIGHT), now);
        batcher.build(ui.list());
        samples.push(started.elapsed().as_secs_f64() * 1e6);
    }
    let allocations = ALLOCATIONS.load(Ordering::Relaxed) - before;
    let stats = Stats::of(&mut samples);
    let batch = batcher.stats();

    println!(
        "{dummies:>8} {:>9} {:>8} {:>9.3} {:>9.3} {:>9.3} {:>9.3} {:>9.3} {:>8}",
        batch.commands,
        batch.quads,
        stats.mean,
        stats.p50,
        stats.p95,
        stats.p99,
        stats.max,
        allocations
    );
}

fn main() -> ExitCode {
    if let Err(error) = engine_core::logging::init() {
        eprintln!("fatal: failed to install logger: {error}");
        return ExitCode::FAILURE;
    }

    let mut scenarios = Vec::from(DEFAULT_SCENARIOS);
    let mut iterations = DEFAULT_ITERATIONS;
    let mut gpu = true;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--elements" => {
                let Some(value) = arguments.next().and_then(|raw| raw.parse().ok()) else {
                    eprintln!("error: --elements needs a number");
                    return ExitCode::FAILURE;
                };
                scenarios = vec![value];
            }
            "--iterations" => {
                let Some(value) = arguments.next().and_then(|raw| raw.parse().ok()) else {
                    eprintln!("error: --iterations needs a number");
                    return ExitCode::FAILURE;
                };
                iterations = value;
            }
            "--no-gpu" => gpu = false,
            "-h" | "--help" => {
                println!(
                    "usage: ui_bench [--elements <count>] [--iterations <count>] [--no-gpu]\n\n\
                     Measures UI layout, batching and drawing. The CPU phase needs no\n\
                     device; the GPU phase opens a hidden 1920x1080 window."
                );
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("error: unknown option {other:?}");
                return ExitCode::FAILURE;
            }
        }
    }

    println!("\n=== UI BENCHMARK ===\n");
    println!("CPU phase: layout and batching, no device. {iterations} measured iterations.");
    if gpu {
        println!("GPU phase: {WIDTH}x{HEIGHT} hidden window, {GPU_FRAMES} frames, vsync off.");
    }
    println!();

    let measurements: Vec<Measurement> = scenarios
        .iter()
        .map(|&elements| measure(elements, iterations, gpu))
        .collect();

    report(&measurements);

    println!(
        "
=== COMBAT HUD ===
"
    );
    println!(
        "The real interface: nameplates, target frame, player bars, ability bar, cast bar
         and {} floating damage numbers, over an arena that has been fought in.
",
        engine_presentation::MAX_FLOATING_NUMBERS
    );
    println!(
        "{:>8} {:>9} {:>8} {:>9} {:>9} {:>9} {:>9} {:>9} {:>8}",
        "dummies", "commands", "quads", "cpu us", "p50", "p95", "p99", "max", "allocs"
    );
    for dummies in [1_usize, 20, 100, 500, 1_000] {
        measure_combat(dummies, iterations);
    }

    ExitCode::SUCCESS
}

/// Measures one scenario.
fn measure(elements: usize, iterations: usize, gpu: bool) -> Measurement {
    let mut list = UiDrawList::with_capacity(elements * 8, elements * 32);
    let mut batcher = UiBatcher::new(1 << 20);

    for frame in 0..WARMUP_ITERATIONS {
        build_ui(&mut list, elements, frame);
        batcher.build(&list);
    }

    let mut layout_samples = Vec::with_capacity(iterations);
    let mut batch_samples = Vec::with_capacity(iterations);
    let mut total_samples = Vec::with_capacity(iterations);

    let before = ALLOCATIONS.load(Ordering::Relaxed);
    for frame in 0..iterations {
        let started = Instant::now();
        build_ui(&mut list, elements, frame);
        let laid_out = Instant::now();
        batcher.build(&list);
        let finished = Instant::now();

        layout_samples.push(laid_out.duration_since(started).as_secs_f64() * 1e6);
        batch_samples.push(finished.duration_since(laid_out).as_secs_f64() * 1e6);
        total_samples.push(finished.duration_since(started).as_secs_f64() * 1e6);
    }
    let allocations = ALLOCATIONS.load(Ordering::Relaxed) - before;

    let stats = batcher.stats();
    let gpu = if gpu {
        measure_gpu(elements).unwrap_or_else(|error| {
            eprintln!("SKIP: the GPU phase could not run: {error}");
            None
        })
    } else {
        None
    };

    Measurement {
        elements,
        commands: stats.commands,
        quads: stats.quads,
        vertices: stats.vertices,
        layout: Stats::of(&mut layout_samples),
        batch: Stats::of(&mut batch_samples),
        total: Stats::of(&mut total_samples),
        allocations,
        gpu,
    }
}

/// Renders the same interface against a hidden window and reports the GPU's own timings.
///
/// Measured twice — with the UI and with it switched off — because the number that matters
/// is what the interface *adds*, and a frame that clears and presents is not free.
fn measure_gpu(elements: usize) -> Result<Option<GpuMeasurement>, engine_core::error::EngineError> {
    let window = Window::new(&WindowConfig::new("ui_bench", WIDTH, HEIGHT).with_visible(false))?;
    let mut renderer = Renderer::new(
        window.raw_handle(),
        &RendererConfig::new(WIDTH, HEIGHT).with_vsync(false),
    )?;

    let mut list = UiDrawList::with_capacity(elements * 8, elements * 32);

    let with_ui = render_pass(&mut renderer, &mut list, elements, true)?;
    let without_ui = render_pass(&mut renderer, &mut list, elements, false)?;

    Ok(Some(GpuMeasurement {
        with_ui: with_ui.0,
        without_ui: without_ui.0,
        draw_calls_with_ui: with_ui.1,
        draw_calls_without_ui: without_ui.1,
    }))
}

/// Renders `GPU_FRAMES` frames and returns their GPU time and the last frame's draw calls.
fn render_pass(
    renderer: &mut Renderer,
    list: &mut UiDrawList,
    elements: usize,
    ui: bool,
) -> Result<(Stats, u32), engine_core::error::EngineError> {
    renderer.set_ui_enabled(ui);
    let mut samples = Vec::with_capacity(GPU_FRAMES);

    for frame in 0..GPU_WARMUP_FRAMES + GPU_FRAMES {
        if ui {
            build_ui(list, elements, frame);
            renderer.set_ui(list);
        }
        renderer.render([0.02, 0.02, 0.03, 1.0])?;
        renderer.present()?;
        if frame >= GPU_WARMUP_FRAMES
            && let Some(milliseconds) = renderer.last_gpu_frame_ms()
        {
            samples.push(milliseconds);
        }
    }

    let draw_calls = renderer.draw_calls_last_frame();
    Ok((Stats::of(&mut samples), draw_calls))
}

/// Prints the table and the per-scenario detail.
fn report(measurements: &[Measurement]) {
    println!(
        "{:>9} {:>9} {:>8} {:>9} {:>9} {:>9} {:>9} {:>9} {:>8}",
        "elements", "commands", "quads", "cpu us", "p50", "p95", "p99", "max", "allocs"
    );
    for measurement in measurements {
        println!(
            "{:>9} {:>9} {:>8} {:>9.3} {:>9.3} {:>9.3} {:>9.3} {:>9.3} {:>8}",
            measurement.elements,
            measurement.commands,
            measurement.quads,
            measurement.total.mean,
            measurement.total.p50,
            measurement.total.p95,
            measurement.total.p99,
            measurement.total.max,
            measurement.allocations,
        );
    }

    for measurement in measurements {
        println!("\nScenario: {} elements", measurement.elements);
        println!(
            "  {} commands -> {} quads -> {} vertices",
            measurement.commands, measurement.quads, measurement.vertices
        );
        print_stats("  layout       ", &measurement.layout);
        print_stats("  batch        ", &measurement.batch);
        print_stats("  layout+batch ", &measurement.total);
        println!(
            "  allocations over the measured iterations: {}",
            measurement.allocations
        );
        if let Some(gpu) = &measurement.gpu {
            println!(
                "  GPU with UI     mean {:.3} ms  p50 {:.3}  p95 {:.3}  p99 {:.3}  max {:.3}",
                gpu.with_ui.mean,
                gpu.with_ui.p50,
                gpu.with_ui.p95,
                gpu.with_ui.p99,
                gpu.with_ui.max
            );
            println!(
                "  GPU without UI  mean {:.3} ms  p50 {:.3}  p95 {:.3}  p99 {:.3}  max {:.3}",
                gpu.without_ui.mean,
                gpu.without_ui.p50,
                gpu.without_ui.p95,
                gpu.without_ui.p99,
                gpu.without_ui.max
            );
            println!(
                "  GPU cost of the UI: {:.3} ms      draw calls {} vs {}",
                gpu.with_ui.mean - gpu.without_ui.mean,
                gpu.draw_calls_with_ui,
                gpu.draw_calls_without_ui
            );
        }
    }

    println!(
        "\nA frame at 240 FPS is 4167 us and at 480 FPS is 2083 us. The CPU column is the\n\
         whole interface: laying it out and turning it into vertices."
    );
}

/// Prints one line of statistics in microseconds.
fn print_stats(label: &str, stats: &Stats) {
    println!(
        "{label}mean {:>8.3} us  p50 {:>8.3}  p95 {:>8.3}  p99 {:>8.3}  max {:>8.3}",
        stats.mean, stats.p50, stats.p95, stats.p99, stats.max
    );
}
