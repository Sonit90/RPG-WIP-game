//! What the first dungeon costs: one pack, several packs, the whole thing, and the boss.
//!
//! ```text
//! simulation tick ─┬─ AI        every brain in the dungeon, pulled or idle
//!                  ├─ hazards   burning ground the Weavers and the boss leave
//!                  ├─ movement  every body, enemies through the character controller
//!                  └─ combat    telegraphs, damage, deaths
//! plus, per frame:  extraction, culling, the VFX director, the audio director, the mix
//! ```
//!
//! Its own binary, no device, for the same reason `ai_bench` is: "is the dungeon fast
//! enough?" is a question about arrays, and it must be answerable on a machine with no GPU.
//!
//! ```text
//! cargo run --release --bin dungeon_bench
//! cargo run --release --bin dungeon_bench -- --frames 600
//! ```
//!
//! Every scenario runs the **real** dungeon: the real level, the real collision, the real
//! brains, the real hazards, the real damage path. The only thing the harness does that
//! gameplay would not is keep the player alive — without it the measurement describes a
//! corpse on the floor and a pack walking home, which is not what the fight costs.

use std::alloc::{GlobalAlloc, Layout, System};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use engine_audio::{AudioSystem, CHANNELS, DEFAULT_MAX_VOICES, SoundBank};
use engine_core::math::{Sphere, Vec3};
use engine_game::command::PlayerCommand;
use engine_game::dungeon::{BOSS_PACK, PACKS, build_dungeon};
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

/// The frame time 240 FPS allows, in microseconds. The stated target.
const BUDGET_US: f64 = 1e6 / 240.0;

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

/// One scenario: which packs are pulled, and how the boss is set up.
struct Scenario {
    label: &'static str,
    /// Which packs the player is dragged through and left fighting.
    packs: &'static [usize],
    /// Whether the monsters are drawn with their animated models, as the game draws them,
    /// rather than with the rigid stand-in. The difference is the cost of posing them.
    animated: bool,
    /// Health fraction the boss is put at before measuring, if the boss is in the scenario.
    ///
    /// This is how the *worst case* is reached deterministically: phase three is a health
    /// threshold, and waiting for a benchmark's player to do four thousand damage would make
    /// the measurement depend on the player's rotation.
    boss_health: Option<f32>,
}

/// Mean and percentiles over a set of samples, in microseconds.
struct Stats {
    mean: f64,
    p99: f64,
    max: f64,
}

impl Stats {
    /// Summarizes `samples`, which is sorted in place.
    fn of(samples: &mut [f64]) -> Self {
        if samples.is_empty() {
            return Self {
                mean: 0.0,
                p99: 0.0,
                max: 0.0,
            };
        }
        let sum: f64 = samples.iter().sum();
        samples.sort_by(f64::total_cmp);
        Self {
            mean: sum / samples.len() as f64,
            p99: engine_core::time::percentile_of_sorted(samples, 0.99),
            max: samples[samples.len() - 1],
        }
    }
}

/// What one scenario measured.
struct Measurement {
    label: &'static str,
    ai: Stats,
    simulation: Stats,
    extraction: Stats,
    culling: Stats,
    vfx: Stats,
    audio: Stats,
    mix: Stats,
    total: Stats,
    entities: usize,
    fighting: usize,
    hazards: usize,
    effects: usize,
    voices: usize,
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
                    "usage: dungeon_bench [--frames <count>]\n\n\
                     Measures the first dungeon: one pack, three, five, the whole thing, the\n\
                     boss encounter, and the worst case the boss can produce. No device is\n\
                     created."
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

/// Every scenario the phase asked for.
static SCENARIOS: [Scenario; 8] = [
    Scenario {
        label: "1 pack",
        packs: &[0],
        animated: false,
        boss_health: None,
    },
    Scenario {
        label: "3 packs",
        packs: &[0, 1, 2],
        animated: false,
        boss_health: None,
    },
    Scenario {
        label: "5 packs",
        packs: &[0, 1, 2, 3, 4],
        animated: false,
        boss_health: None,
    },
    Scenario {
        label: "full dungeon",
        packs: &[0, 1, 2, 3, 4, 5, 6, 7],
        animated: false,
        boss_health: None,
    },
    Scenario {
        label: "boss encounter",
        packs: &[BOSS_PACK],
        animated: false,
        boss_health: None,
    },
    // Phase three, with every trash pack in the dungeon also pulled and still alive. Nothing
    // in the game produces this, which is the point: it is the ceiling, not the fight.
    Scenario {
        label: "boss, worst case",
        packs: &[0, 1, 2, 3, 4, 5, 6, 7, BOSS_PACK],
        animated: false,
        boss_health: Some(0.3),
    },
    // The two above again, with every monster posed by its own skeleton as the game draws
    // it. The extraction column's difference is what animation costs; it was never measured
    // before the performance audit (docs/PERFORMANCE_AUDIT.md), which found it at ~0.75 us a
    // monster in the game.
    Scenario {
        label: "full, animated",
        packs: &[0, 1, 2, 3, 4, 5, 6, 7],
        animated: true,
        boss_health: None,
    },
    Scenario {
        label: "worst, animated",
        packs: &[0, 1, 2, 3, 4, 5, 6, 7, BOSS_PACK],
        animated: true,
        boss_health: Some(0.3),
    },
];

/// Runs every scenario and prints the tables.
fn run(frames: usize) {
    println!(
        "dungeon benchmark: {frames} measured frames per scenario, {WARMUP_FRAMES} warm-up, no device"
    );
    println!("target: 240 FPS is {BUDGET_US:.0} us a frame at 1920x1080");
    println!(
        "{:<18} {:>7} {:>8} {:>8} {:>8} {:>8} {:>8} {:>9} {:>9} {:>9} {:>7}",
        "scenario",
        "ai us",
        "sim us",
        "extr us",
        "cull us",
        "vfx us",
        "audio us",
        "total us",
        "p99 us",
        "max us",
        "allocs"
    );

    let mut results = Vec::with_capacity(SCENARIOS.len());
    for scenario in &SCENARIOS {
        let measurement = measure(scenario, frames);
        report(&measurement);
        results.push(measurement);
    }

    println!();
    println!("what was actually running:");
    for result in &results {
        println!(
            "  {:<18} {:>3} entities | {:>3} fighting | {:>2} hazards | {:>3} effects | {:>2} voices | mix {:.1} us",
            result.label,
            result.entities,
            result.fighting,
            result.hazards,
            result.effects,
            result.voices,
            result.mix.mean,
        );
    }

    println!();
    println!("against the budget:");
    for result in &results {
        let headroom = BUDGET_US - result.total.p99;
        println!(
            "  {:<18} p99 {:>8.1} us of {BUDGET_US:.0} us  ({}{:.1} us, {:.0}% of budget)",
            result.label,
            result.total.p99,
            if headroom >= 0.0 { "+" } else { "" },
            headroom,
            result.total.p99 / BUDGET_US * 100.0,
        );
    }
}

/// Prints one row.
fn report(measurement: &Measurement) {
    println!(
        "{:<18} {:>7.1} {:>8.1} {:>8.1} {:>8.1} {:>8.1} {:>8.1} {:>9.1} {:>9.1} {:>9.1} {:>7}",
        measurement.label,
        measurement.ai.mean,
        measurement.simulation.mean,
        measurement.extraction.mean,
        measurement.culling.mean,
        measurement.vfx.mean,
        measurement.audio.mean,
        measurement.total.mean,
        measurement.total.p99,
        measurement.total.max,
        measurement.allocations,
    );
}

/// Keeps the player and every enemy alive, so a scenario measures a fight rather than a
/// field of corpses.
///
/// Through the world rather than through gameplay, precisely so it is visible here as a
/// harness and cannot be mistaken for a rule.
fn sustain(simulation: &mut Simulation) {
    let player = simulation.player().entity;
    simulation.world_mut().heal(player, 1_000.0);
    let entities: Vec<_> = simulation.world().entities().collect();
    for entity in entities {
        if entity == player {
            continue;
        }
        simulation.world_mut().heal(entity, 1_000.0);
    }
}

/// Drags the player to each pack in turn so all of them are pulled, then parks them on the
/// last one.
fn pull(simulation: &mut Simulation, packs: &[usize]) {
    for pack in packs {
        let Some(target) = PACKS.get(*pack).and_then(|pack| {
            pack.encounter()
                .spawns()
                .first()
                .map(|spawn| spawn.position)
        }) else {
            continue;
        };
        simulation.push_command(PlayerCommand::Teleport {
            position: target + Vec3::new(0.0, 0.0, 3.0),
        });
        // Long enough for every member to notice and reach its preferred range.
        for _ in 0..240 {
            sustain(simulation);
            simulation.step_once();
        }
    }
}

/// Runs one scenario and measures it.
fn measure(scenario: &Scenario, frames: usize) -> Measurement {
    let (mut simulation, dungeon) = build_dungeon();

    let mut extractor = RenderExtractor::new(bindings(), 240.0);
    if scenario.animated {
        extractor.set_monster_models(
            engine_assets::procedural::MONSTER_MODELS
                .iter()
                .filter_map(|name| engine_presentation::MonsterModel::unbound(name))
                .collect(),
        );
    }
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

    pull(&mut simulation, scenario.packs);
    if let (Some(fraction), Some(boss)) = (scenario.boss_health, dungeon.boss(&simulation)) {
        let max = simulation
            .world()
            .health(boss)
            .map_or(0.0, engine_game::Health::max);
        let current = simulation
            .world()
            .health(boss)
            .map_or(0.0, engine_game::Health::current);
        let wanted = max * fraction;
        if wanted < current {
            simulation.world_mut().damage(boss, current - wanted);
        }
        // And it stays there: `sustain` would heal it straight back out of phase three.
        for _ in 0..60 {
            simulation.step_once();
        }
    }

    let hold_boss = scenario.boss_health;
    let mut frame = |simulation: &mut Simulation,
                     extractor: &mut RenderExtractor,
                     camera: &mut Camera,
                     controller: &mut ThirdPersonCamera,
                     culler: &mut Culler,
                     list: &mut RenderList,
                     vfx: &mut VfxDirector,
                     audio: &mut AudioSystem,
                     director: &mut AudioDirector,
                     index: usize|
     -> [f64; 7] {
        // A rotation, so the player's own casts, zones and burns are in the measurement.
        if index % 8 == 0 {
            simulation.push_command(PlayerCommand::UseAbility((index / 8) % 10));
        }
        sustain(simulation);
        if let (Some(fraction), Some(boss)) = (hold_boss, dungeon.boss(simulation)) {
            // Held at its threshold: `sustain` just healed it, and a boss that drifted out of
            // phase three mid-scenario would measure two different fights.
            let max = simulation
                .world()
                .health(boss)
                .map_or(0.0, engine_game::Health::max);
            let current = simulation
                .world()
                .health(boss)
                .map_or(0.0, engine_game::Health::current);
            let wanted = max * fraction;
            if wanted < current {
                simulation.world_mut().damage(boss, current - wanted);
            }
        }

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
    let mut hazards = 0_usize;
    let mut effects = 0_usize;
    let mut voices = 0_usize;

    // Counted over the measured frames only: building the dungeon and warming up are expected
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
            index + WARMUP_FRAMES,
        );

        hazards = hazards.max(simulation.enemy_hazards().len());
        effects = effects.max(vfx.stats().entities);
        voices = voices.max(audio.stats().active);

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
        .filter(|(_, brain)| brain.state().is_combat())
        .count();

    Measurement {
        label: scenario.label,
        ai: Stats::of(&mut ai_samples),
        simulation: Stats::of(&mut sim_samples),
        extraction: Stats::of(&mut extraction_samples),
        culling: Stats::of(&mut culling_samples),
        vfx: Stats::of(&mut vfx_samples),
        audio: Stats::of(&mut audio_samples),
        mix: Stats::of(&mut mix_samples),
        total: Stats::of(&mut total_samples),
        entities: simulation.world().len(),
        fighting,
        hazards,
        effects,
        voices,
        allocations,
    }
}

/// Placeholder handles: the benchmark creates no device, so nothing is ever uploaded.
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
