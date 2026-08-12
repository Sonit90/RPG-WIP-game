//! What combat's audio costs, on both threads, and how it scales with the size of the fight.
//!
//! ```text
//! simulation ─▶ observe events ─▶ AudioDirector::update ─▶ AudioSystem::render
//!               ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^    ^^^^^^^^^^^^^^^^^^^
//!               the game thread's share                    the device thread's share
//! ```
//!
//! Its own binary, and no device, for the same reason `vfx_bench` is: "is the audio system
//! fast enough?" is a question about arithmetic over arrays, and a machine without speakers
//! must be able to answer it. The mix is measured by asking for the same block of frames a
//! device would ask for — 512 frames at 48 kHz, about 10.7 ms of sound — so the number in the
//! table is the number the callback would spend.
//!
//! ```text
//! cargo run --release --bin audio_bench
//! cargo run --release --bin audio_bench -- --frames 500
//! ```
//!
//! Every scenario runs the **real** director against the **real** simulation: the events are
//! the ones combat emits, the sounds are the catalogue's, and the limits are the ones the
//! game ships. Nothing is synthesised, because a benchmark that made up its own events would
//! measure a system nobody ships.
//!
//! # What the two "no audio" rows are for
//!
//! Audio's cost is the *difference* between two runs of the same fight in the same binary,
//! exactly as the VFX and UI benchmarks measure theirs. A build compiled without audio would
//! differ in a dozen other ways.

use std::alloc::{GlobalAlloc, Layout, System};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use engine_audio::{AudioSystem, CHANNELS, DEFAULT_MAX_VOICES, Listener, SoundBank};
use engine_core::math::Vec3;
use engine_game::arena::{self, ArenaConfig};
use engine_game::command::PlayerCommand;
use engine_game::firemage::FireAbility;
use engine_game::sim::Simulation;
use engine_presentation::AudioDirector;

/// Measured frames per scenario when none is given.
const DEFAULT_FRAMES: usize = 1_200;

/// Unmeasured frames before each scenario, so every buffer has reached its high-water mark.
const WARMUP_FRAMES: usize = 300;

/// The rate the benchmark's bank is synthesised at.
///
/// A real device's rate, not a test's: the mix cost is per sample, so measuring at 8 kHz
/// would flatter it six-fold.
const SAMPLE_RATE: u32 = 48_000;

/// Frames the device asks for in one callback.
///
/// 512 at 48 kHz is 10.7 ms of sound, which is what WASAPI shared mode typically asks for.
const BLOCK_FRAMES: usize = 512;

/// Frame delta the director is advanced by, in seconds.
///
/// 1/240, the engine's target: sounds are advanced with *real* time, so this is the step the
/// shipped game takes.
const FRAME_DELTA: f32 = 1.0 / 240.0;

/// How many game frames pass per device callback at 240 FPS.
///
/// 10.7 ms of sound against 4.2 ms frames: the callback runs about every two and a half
/// frames, and the benchmark mixes on that schedule rather than once a frame, so the mix
/// cost per frame is the cost the machine actually pays.
const FRAMES_PER_BLOCK: usize = 2;

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
    /// Nothing at all. The baseline: a fight standing still, with only the bed playing.
    Idle,
    /// The single-target rotation: filler, lance, pyroblast, detonate.
    SingleTarget,
    /// Everything with an area: cone, meteor, firestorm, detonate.
    AreaHeavy,
}

/// One scenario: a number of dummies and what the player does to them.
struct Scenario {
    label: &'static str,
    dummies: usize,
    rotation: Rotation,
    /// Whether audio runs at all. The paired "off" run is what makes the audio cost a
    /// difference between two runs of one binary rather than a guess.
    audio: bool,
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
    audio: bool,
    /// Microseconds per frame, by pass.
    simulation: Stats,
    observe: Stats,
    update: Stats,
    /// Microseconds per device callback.
    mix: Stats,
    /// The game thread's total: simulation, observation and the director's update.
    total: Stats,
    peak_voices: usize,
    mean_voices: f64,
    started: u64,
    thinned: u64,
    dropped: u64,
    limited: u64,
    bank_bytes: usize,
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
                    "usage: audio_bench [--frames <count>]\n\n\
                     Measures what combat's audio costs: event observation, the director's\n\
                     update, and the mix itself, at 1 to 100 targets and with the area\n\
                     rotation on top. No device is opened."
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
            label: "idle, no audio",
            dummies: 10,
            rotation: Rotation::Idle,
            audio: false,
        },
        Scenario {
            label: "idle",
            dummies: 10,
            rotation: Rotation::Idle,
            audio: true,
        },
        Scenario {
            label: "1 target, no audio",
            dummies: 0,
            rotation: Rotation::SingleTarget,
            audio: false,
        },
        Scenario {
            label: "1 target",
            dummies: 0,
            rotation: Rotation::SingleTarget,
            audio: true,
        },
        Scenario {
            label: "10 targets",
            dummies: 9,
            rotation: Rotation::SingleTarget,
            audio: true,
        },
        Scenario {
            label: "50 targets",
            dummies: 49,
            rotation: Rotation::SingleTarget,
            audio: true,
        },
        Scenario {
            label: "100 targets",
            dummies: 99,
            rotation: Rotation::SingleTarget,
            audio: true,
        },
        Scenario {
            label: "aoe-heavy, 50, no audio",
            dummies: 49,
            rotation: Rotation::AreaHeavy,
            audio: false,
        },
        Scenario {
            label: "aoe-heavy, 50",
            dummies: 49,
            rotation: Rotation::AreaHeavy,
            audio: true,
        },
        Scenario {
            label: "worst case, 100",
            dummies: 99,
            rotation: Rotation::AreaHeavy,
            audio: true,
        },
    ];

    println!(
        "audio benchmark: {frames} measured frames per scenario, {WARMUP_FRAMES} warm-up, \
         {SAMPLE_RATE} Hz, {BLOCK_FRAMES}-frame blocks, no device"
    );
    println!(
        "{:<24} {:>5} {:>8} {:>8} {:>8} {:>9} {:>9} {:>9} {:>7} {:>7} {:>7} {:>7}",
        "scenario",
        "audio",
        "sim us",
        "obs us",
        "upd us",
        "game us",
        "p99 us",
        "mix us",
        "voices",
        "peak",
        "thinned",
        "allocs"
    );

    let mut results = Vec::with_capacity(scenarios.len());
    for scenario in &scenarios {
        let measurement = measure(scenario, frames);
        report(&measurement);
        results.push(measurement);
    }

    println!();
    println!("what audio costs, as the difference between paired runs:");
    compare(&results, "idle, no audio", "idle");
    compare(&results, "1 target, no audio", "1 target");
    compare(&results, "aoe-heavy, 50, no audio", "aoe-heavy, 50");

    println!();
    println!("what each scenario asked for, and what the limiter did about it:");
    for result in &results {
        println!(
            "  {:<24} started {:>6} | thinned {:>6} | dropped {:>4} | limited samples {:>7} | \
             bank {:.1} MiB",
            result.label,
            result.started,
            result.thinned,
            result.dropped,
            result.limited,
            result.bank_bytes as f64 / (1024.0 * 1024.0),
        );
    }
}

/// Prints the difference audio makes between two runs of the same fight.
fn compare(results: &[Measurement], without: &str, with: &str) {
    let find = |label: &str| results.iter().find(|result| result.label == label);
    let (Some(off), Some(on)) = (find(without), find(with)) else {
        return;
    };
    println!(
        "  {:<24} {:>8.1} us -> {:>8.1} us  (+{:.1} us mean, +{:.1} us p99 on the game thread; \
         {:.1} us per mix)",
        with,
        off.total.mean,
        on.total.mean,
        on.total.mean - off.total.mean,
        on.total.p99 - off.total.p99,
        on.mix.mean,
    );
}

/// Runs one scenario and measures it.
fn measure(scenario: &Scenario, frames: usize) -> Measurement {
    let mut simulation = arena::training_arena(&ArenaConfig::with_group(scenario.dummies));
    simulation.step_once();
    simulation.select_nearest_target();

    let bank = SoundBank::synthesised(SAMPLE_RATE);
    let bank_bytes = bank.bytes();
    let mut audio = AudioSystem::with_bank(bank, SAMPLE_RATE, DEFAULT_MAX_VOICES);
    let mut director = AudioDirector::new();
    if !scenario.audio {
        director.set_enabled(false, &mut audio);
        audio.set_enabled(false);
    }
    let mut block = vec![0.0_f32; BLOCK_FRAMES * CHANNELS];

    let mut peak_voices = 0;
    let mut voice_sum = 0.0;

    // The listener stands where the player does, a head above the ground and facing the
    // fight — the same place the third-person camera puts it.
    let place_listener = |audio: &mut AudioSystem, simulation: &Simulation| {
        audio.set_listener(Listener::new(
            simulation.player_position() + Vec3::new(0.0, 1.7, 4.0),
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(0.0, 1.0, 0.0),
        ));
    };

    for index in 0..WARMUP_FRAMES {
        drive(&mut simulation, scenario.rotation, index);
        simulation.step_once();
        place_listener(&mut audio, &simulation);
        director.observe(&simulation, &mut audio);
        director.update(&simulation, FRAME_DELTA, &mut audio);
        if index % FRAMES_PER_BLOCK == 0 {
            audio.render(&mut block);
        }
    }
    audio.reset_stats();

    let mut simulation_samples = Vec::with_capacity(frames);
    let mut observe_samples = Vec::with_capacity(frames);
    let mut update_samples = Vec::with_capacity(frames);
    let mut mix_samples = Vec::with_capacity(frames / FRAMES_PER_BLOCK + 1);
    let mut total_samples = Vec::with_capacity(frames);

    // Counted over the measured frames only: building the arena, synthesising the bank and
    // warming up are expected to allocate, and the question is whether the steady state does.
    let before = ALLOCATIONS.load(Ordering::Relaxed);

    for index in 0..frames {
        drive(&mut simulation, scenario.rotation, index + WARMUP_FRAMES);

        let started = Instant::now();
        simulation.step_once();
        let sim = started.elapsed().as_secs_f64() * 1e6;

        place_listener(&mut audio, &simulation);

        let started = Instant::now();
        director.observe(&simulation, &mut audio);
        let observe = started.elapsed().as_secs_f64() * 1e6;

        let started = Instant::now();
        director.update(&simulation, FRAME_DELTA, &mut audio);
        let update = started.elapsed().as_secs_f64() * 1e6;

        // The device's own thread, on the device's own schedule.
        if index % FRAMES_PER_BLOCK == 0 {
            let started = Instant::now();
            audio.render(&mut block);
            mix_samples.push(started.elapsed().as_secs_f64() * 1e6);
        }

        let stats = audio.stats();
        peak_voices = peak_voices.max(stats.active);
        voice_sum += stats.active as f64;

        simulation_samples.push(sim);
        observe_samples.push(observe);
        update_samples.push(update);
        total_samples.push(sim + observe + update);
    }

    let allocations = ALLOCATIONS.load(Ordering::Relaxed) - before;
    let stats = audio.stats();

    Measurement {
        label: scenario.label,
        audio: scenario.audio,
        simulation: Stats::of(&mut simulation_samples),
        observe: Stats::of(&mut observe_samples),
        update: Stats::of(&mut update_samples),
        mix: Stats::of(&mut mix_samples),
        total: Stats::of(&mut total_samples),
        peak_voices,
        mean_voices: voice_sum / frames.max(1) as f64,
        started: stats.started,
        thinned: stats.suppressed_interval + stats.suppressed_limit,
        dropped: stats.dropped,
        limited: stats.limited,
        bank_bytes,
        allocations,
    }
}

/// Prints one row of the table.
fn report(result: &Measurement) {
    println!(
        "{:<24} {:>5} {:>8.1} {:>8.1} {:>8.1} {:>9.1} {:>9.1} {:>9.1} {:>7.1} {:>7} {:>7} {:>7}",
        result.label,
        if result.audio { "on" } else { "off" },
        result.simulation.mean,
        result.observe.mean,
        result.update.mean,
        result.total.mean,
        result.total.p99,
        result.mix.mean,
        result.mean_voices,
        result.peak_voices,
        result.thinned,
        result.allocations,
    );
    let _ = result.mix.max;
}

/// Presses what the scenario's rotation presses.
///
/// Frame-indexed rather than random, so two runs of one scenario do the same fight and a
/// difference between them is the code rather than the dice.
fn drive(simulation: &mut Simulation, rotation: Rotation, frame: usize) {
    if matches!(rotation, Rotation::Idle) || frame % 8 != 0 {
        return;
    }
    let target = simulation
        .player()
        .target
        .and_then(|target| simulation.world().position(target));

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
        Rotation::AreaHeavy => {
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
        }
    }
}
