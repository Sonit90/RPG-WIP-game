//! What a fight costs the CPU, and how it scales with the number of enemies.
//!
//! ```text
//! simulation tick ─┬─ AI        every brain: perceive, decide, execute
//!                  ├─ movement  every body, enemies through the character controller
//!                  ├─ abilities the player's own casts
//!                  └─ statuses  burns
//! plus, per frame:  extraction, culling, the VFX director and the audio director
//! ```
//!
//! Its own binary, no device, for the same reason `vfx_bench` and `audio_bench` are: "is the
//! AI fast enough?" is a question about arrays, and it must be answerable on a machine with
//! no GPU. The **combat** stress test (Milestone 25) is the same scenarios with the VFX and
//! audio directors attached, which is what makes "what does the fight cost end to end"
//! measurable as a difference rather than a guess.
//!
//! ```text
//! cargo run --release --bin ai_bench
//! cargo run --release --bin ai_bench -- --frames 600
//! ```
//!
//! Every scenario runs the **real** encounter: the real brains, the real threat tables, the
//! real telegraphs, the real damage path. Nothing is simulated at a distance.

use std::alloc::{GlobalAlloc, Layout, System};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use engine_audio::{AudioSystem, CHANNELS, DEFAULT_MAX_VOICES, SoundBank};
use engine_core::math::{Sphere, Vec3};
use engine_game::arena::{self, ArenaConfig};
use engine_game::command::PlayerCommand;
use engine_game::enemy::brain::AiState;
use engine_game::sim::Simulation;
use engine_presentation::AudioDirector;
use engine_presentation::camera::ThirdPersonCamera;
use engine_presentation::extract::{RenderExtractor, ViewParams, VisualBindings};
use engine_presentation::vfx::{VfxDirector, VfxVisuals};
use engine_render_data::{Camera, MaterialHandle, MeshHandle, Projection};
use engine_renderer::culling::Culler;
use engine_renderer::render_list::RenderList;
use engine_vfx::VfxShape;

/// Measured frames per scenario when none is given.
const DEFAULT_FRAMES: usize = 1_200;

/// Unmeasured frames before each scenario, so every buffer has reached its high-water mark.
const WARMUP_FRAMES: usize = 300;

/// Benchmark aspect ratio: 1920x1080, the stated target resolution.
const ASPECT: f32 = 1920.0 / 1080.0;

/// Frame delta the presentation layers are advanced by, in seconds.
const FRAME_DELTA: f32 = 1.0 / 240.0;

/// The rate the benchmark's sound bank is synthesised at.
const SAMPLE_RATE: u32 = 48_000;

/// Frames one device callback asks for.
const BLOCK_FRAMES: usize = 512;

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

/// One scenario: a number of enemies, and whether presentation is attached.
struct Scenario {
    label: &'static str,
    enemies: usize,
    /// Whether the VFX and audio directors observe the fight.
    ///
    /// The paired runs are what make "what does the whole fight cost" a difference between
    /// two runs of one binary rather than an estimate.
    presentation: bool,
}

/// Mean and percentiles over a set of samples, in microseconds.
struct Stats {
    mean: f64,
    p99: f64,
}

impl Stats {
    /// Summarizes `samples`, which is sorted in place.
    fn of(samples: &mut [f64]) -> Self {
        if samples.is_empty() {
            return Self {
                mean: 0.0,
                p99: 0.0,
            };
        }
        let sum: f64 = samples.iter().sum();
        samples.sort_by(f64::total_cmp);
        Self {
            mean: sum / samples.len() as f64,
            p99: engine_core::time::percentile_of_sorted(samples, 0.99),
        }
    }
}

/// What one scenario measured.
struct Measurement {
    label: &'static str,
    enemies: usize,
    presentation: bool,
    /// Microseconds per frame, by pass.
    ai: Stats,
    simulation: Stats,
    extraction: Stats,
    culling: Stats,
    vfx: Stats,
    audio: Stats,
    mix: Stats,
    total: Stats,
    fighting: usize,
    casts: u64,
    telegraphs: u64,
    damage_taken: f64,
    allocations: usize,
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
                    "usage: ai_bench [--frames <count>]\n\n\
                     Measures what a fight costs: every enemy's brain, the simulation around\n\
                     it, and - in the paired runs - extraction, culling, effects and audio.\n\
                     1 to 100 enemies. No device is created."
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

/// Runs every scenario and prints the tables.
fn run(frames: usize) {
    let scenarios = [
        Scenario {
            label: "1 enemy",
            enemies: 1,
            presentation: false,
        },
        Scenario {
            label: "10 enemies",
            enemies: 10,
            presentation: false,
        },
        Scenario {
            label: "50 enemies",
            enemies: 50,
            presentation: false,
        },
        Scenario {
            label: "100 enemies",
            enemies: 100,
            presentation: false,
        },
        Scenario {
            label: "10, full frame",
            enemies: 10,
            presentation: true,
        },
        Scenario {
            label: "50, full frame",
            enemies: 50,
            presentation: true,
        },
        Scenario {
            label: "100, full frame",
            enemies: 100,
            presentation: true,
        },
    ];

    println!(
        "ai benchmark: {frames} measured frames per scenario, {WARMUP_FRAMES} warm-up, no device"
    );
    println!(
        "{:<18} {:>6} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>9} {:>9} {:>6} {:>7}",
        "scenario",
        "enemy",
        "ai us",
        "sim us",
        "extr us",
        "cull us",
        "vfx us",
        "audio us",
        "total us",
        "p99 us",
        "fight",
        "allocs"
    );

    let mut results = Vec::with_capacity(scenarios.len());
    for scenario in &scenarios {
        let measurement = measure(scenario, frames);
        report(&measurement);
        results.push(measurement);
    }

    println!();
    println!("what the fight produced:");
    for result in &results {
        println!(
            "  {:<18} {:>3} enemies | {:>3} fighting | {} casts | {} telegraphs | player took {:.0}",
            result.label,
            result.enemies,
            result.fighting,
            result.casts,
            result.telegraphs,
            result.damage_taken,
        );
    }

    println!();
    println!("what presentation adds on top of the same fight:");
    compare(&results, "10 enemies", "10, full frame");
    compare(&results, "50 enemies", "50, full frame");
    compare(&results, "100 enemies", "100, full frame");
}

/// Prints the difference presentation makes between two runs of the same fight.
fn compare(results: &[Measurement], bare: &str, full: &str) {
    let find = |label: &str| results.iter().find(|result| result.label == label);
    let (Some(off), Some(on)) = (find(bare), find(full)) else {
        return;
    };
    println!(
        "  {:<18} {:>8.1} us -> {:>8.1} us  (+{:.1} us mean, +{:.1} us p99)",
        full,
        off.total.mean,
        on.total.mean,
        on.total.mean - off.total.mean,
        on.total.p99 - off.total.p99,
    );
}

/// Runs one scenario and measures it.
fn measure(scenario: &Scenario, frames: usize) -> Measurement {
    let config = ArenaConfig::with_group(0);
    let mut simulation =
        arena::encounter_arena(&config, arena::pack_encounter(&config, scenario.enemies));

    let mut extractor = RenderExtractor::new(bindings(), 240.0);
    let mut culler = Culler::new();
    let mut list = RenderList::new();
    let mut controller = ThirdPersonCamera::following(simulation.player_position());
    let Ok(mut camera) = Camera::new(Projection::perspective(ASPECT)) else {
        panic!("a 16:9 perspective is valid");
    };
    let mut vfx = VfxDirector::new(vfx_visuals());
    let mut audio = AudioSystem::with_bank(
        SoundBank::synthesised(SAMPLE_RATE),
        SAMPLE_RATE,
        DEFAULT_MAX_VOICES,
    );
    let mut director = AudioDirector::new();
    let mut block = vec![0.0_f32; BLOCK_FRAMES * CHANNELS];
    let mut worn: Vec<engine_game::EntityId> = Vec::with_capacity(128);
    if !scenario.presentation {
        vfx.set_enabled(false);
        director.set_enabled(false, &mut audio);
        audio.set_enabled(false);
    }

    // The player walks into the middle of the pack and stays there: every enemy is pulled,
    // every enemy is in range, and the worst case is the steady state rather than a moment.
    let centre = simulation.player_position() + Vec3::new(0.0, 0.0, -20.0);
    simulation.push_command(PlayerCommand::Teleport { position: centre });

    let mut frame = |simulation: &mut Simulation,
                     extractor: &mut RenderExtractor,
                     camera: &mut Camera,
                     controller: &mut ThirdPersonCamera,
                     culler: &mut Culler,
                     list: &mut RenderList,
                     vfx: &mut VfxDirector,
                     audio: &mut AudioSystem,
                     director: &mut AudioDirector,
                     worn: &mut Vec<engine_game::EntityId>,
                     index: usize|
     -> [f64; 7] {
        // A rotation into the pack, so the fight runs in both directions.
        if index % 8 == 0 {
            simulation.push_command(PlayerCommand::UseAbility((index / 8) % 10));
        }
        // The measurement harness keeps the fight *going*: without this the player dies in
        // about eight seconds against fifty enemies and the pack walks home, so the numbers
        // would describe a field of idle enemies rather than a battle. Health is restored
        // through the world, not through gameplay, precisely so it is visible here as a
        // harness and cannot be mistaken for a rule.
        sustain(simulation, worn);

        let started = Instant::now();
        simulation.step_once();
        let total_sim = started.elapsed().as_secs_f64() * 1e6;
        let ai = simulation.last_tick_stats().ai.as_secs_f64() * 1e6;

        controller.follow(simulation.player_position(), FRAME_DELTA);
        let _ = controller.apply(camera, ASPECT);

        let started = Instant::now();
        extractor.set_view(ViewParams {
            camera_position: camera.position(),
            delta_seconds: FRAME_DELTA,
            lod_bias: 1.0,
        });
        extractor.extract(simulation);
        let extraction = started.elapsed().as_secs_f64() * 1e6;

        let started = Instant::now();
        vfx.observe(simulation);
        vfx.update(
            simulation,
            FRAME_DELTA,
            extractor.render_world_mut(),
            &engine_presentation::ui::ground_decal::GroundSurface::Plane(0.0),
        );
        let vfx_time = started.elapsed().as_secs_f64() * 1e6;

        let started = Instant::now();
        director.set_listener(camera, audio);
        director.observe(simulation, audio);
        director.update(simulation, FRAME_DELTA, audio);
        let audio_time = started.elapsed().as_secs_f64() * 1e6;

        let mut mix = 0.0;
        if index % 2 == 0 {
            let started = Instant::now();
            audio.render(&mut block);
            mix = started.elapsed().as_secs_f64() * 1e6;
        }

        let frustum = engine_core::math::Frustum::from_view_projection(&camera.view_projection());
        let started = Instant::now();
        culler.cull(extractor.render_world(), &frustum, list);
        let culling = started.elapsed().as_secs_f64() * 1e6;

        [
            ai,
            (total_sim - ai).max(0.0),
            extraction,
            culling,
            vfx_time,
            audio_time,
            mix,
        ]
    };

    for index in 0..WARMUP_FRAMES {
        frame(
            &mut simulation,
            &mut extractor,
            &mut camera,
            &mut controller,
            &mut culler,
            &mut list,
            &mut vfx,
            &mut audio,
            &mut director,
            &mut worn,
            index,
        );
    }

    let mut ai_samples = Vec::with_capacity(frames);
    let mut sim_samples = Vec::with_capacity(frames);
    let mut extraction_samples = Vec::with_capacity(frames);
    let mut culling_samples = Vec::with_capacity(frames);
    let mut vfx_samples = Vec::with_capacity(frames);
    let mut audio_samples = Vec::with_capacity(frames);
    let mut mix_samples = Vec::with_capacity(frames / 2 + 1);
    let mut total_samples = Vec::with_capacity(frames);
    let mut casts = 0_u64;
    let mut telegraphs = 0_u64;

    // Counted over the measured frames only: building the arena and warming up are expected
    // to allocate, and the question is whether the steady state does.
    let before = ALLOCATIONS.load(Ordering::Relaxed);

    for index in 0..frames {
        let [ai, sim, extraction, culling, vfx_time, audio_time, mix] = frame(
            &mut simulation,
            &mut extractor,
            &mut camera,
            &mut controller,
            &mut culler,
            &mut list,
            &mut vfx,
            &mut audio,
            &mut director,
            &mut worn,
            index + WARMUP_FRAMES,
        );
        for event in simulation.events() {
            match event {
                engine_game::CombatEvent::CastStarted { ability, .. } if ability.is_enemy() => {
                    casts += 1;
                }
                engine_game::CombatEvent::TelegraphStarted { .. } => telegraphs += 1,
                _ => {}
            }
        }

        ai_samples.push(ai);
        sim_samples.push(sim);
        extraction_samples.push(extraction);
        culling_samples.push(culling);
        vfx_samples.push(vfx_time);
        audio_samples.push(audio_time);
        if mix > 0.0 {
            mix_samples.push(mix);
        }
        total_samples.push(ai + sim + extraction + culling + vfx_time + audio_time);
    }

    let allocations = ALLOCATIONS.load(Ordering::Relaxed) - before;
    let fighting = simulation
        .enemies()
        .filter(|(_, brain)| brain.state().is_combat() || brain.state() == AiState::Alert)
        .count();

    Measurement {
        label: scenario.label,
        enemies: scenario.enemies,
        presentation: scenario.presentation,
        ai: Stats::of(&mut ai_samples),
        simulation: Stats::of(&mut sim_samples),
        extraction: Stats::of(&mut extraction_samples),
        culling: Stats::of(&mut culling_samples),
        vfx: Stats::of(&mut vfx_samples),
        audio: Stats::of(&mut audio_samples),
        mix: Stats::of(&mut mix_samples),
        total: Stats::of(&mut total_samples),
        fighting,
        casts,
        telegraphs,
        damage_taken: simulation.meter().total_damage_taken(),
        allocations,
    }
}

/// Prints one row of the table.
fn report(result: &Measurement) {
    println!(
        "{:<18} {:>6} {:>8.1} {:>8.1} {:>8.1} {:>8.1} {:>8.1} {:>8.1} {:>9.1} {:>9.1} {:>6} {:>7}",
        result.label,
        result.enemies,
        result.ai.mean,
        result.simulation.mean,
        result.extraction.mean,
        result.culling.mean,
        result.vfx.mean,
        result.audio.mean,
        result.total.mean,
        result.total.p99,
        result.fighting,
        result.allocations,
    );
    let _ = result.presentation;
    let _ = result.mix.p99;
}

/// Keeps everybody on their feet, so the stress scenario measures a fight in progress.
///
/// The player is topped up every frame and any enemy that has been worn down is restored.
/// Neither is a gameplay rule: this is the equivalent of the VFX benchmark's stress spawner,
/// and it exists so the worst case is a *steady state* rather than a moment before everything
/// is dead.
fn sustain(simulation: &mut Simulation, worn: &mut Vec<engine_game::EntityId>) {
    let player = simulation.player().entity;
    if simulation
        .world()
        .health(player)
        .is_some_and(|health| health.fraction() < 0.9)
    {
        simulation.world_mut().restore(player);
    }
    // Into a buffer the caller owns, so the harness itself allocates nothing and the
    // allocation column measures the engine rather than this function.
    worn.clear();
    worn.extend(simulation.enemies().filter_map(|(entity, _)| {
        let health = simulation.world().health(entity)?;
        (health.fraction() < 0.4 || !simulation.world().is_alive(entity)).then_some(entity)
    }));
    for entity in worn.iter() {
        simulation.world_mut().restore(*entity);
    }
}

/// The handles the extractor draws gameplay entities with.
///
/// Nothing is uploaded and no pass here dereferences a handle; what matters is that two
/// entities sharing a model share their handles, so batching behaves as it would in a frame.
fn bindings() -> VisualBindings {
    let dummy =
        engine_assets::procedural::training_dummy(engine_assets::procedural::DummyVariant::Single);
    let group =
        engine_assets::procedural::training_dummy(engine_assets::procedural::DummyVariant::Group);
    VisualBindings::unbound(&dummy, &group)
}

/// A VFX binding set with every shape bound to a fabricated handle.
fn vfx_visuals() -> VfxVisuals {
    let mut visuals = VfxVisuals::new(MaterialHandle::new(0, 1));
    for (index, shape) in VfxShape::ALL.into_iter().enumerate() {
        visuals.bind(
            shape,
            MeshHandle::new(index as u32, 1),
            Sphere::new(Vec3::ZERO, 1.0),
        );
    }
    visuals
}
