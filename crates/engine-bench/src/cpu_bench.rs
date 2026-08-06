//! CPU-side engine benchmarks: allocators, job system, input, frame overhead.
//!
//! Lives in the benchmark crate, so none of it can reach the game executable —
//! `cargo build --release --bin mygame` never compiles this file.
//!
//! ```text
//! cargo run --release --bin cpu_bench
//! cargo run --release --bin cpu_bench -- --iterations 200000
//! ```
//!
//! Every number is a median of repeated samples, not a single run: a single sample on
//! Windows measures whatever else the scheduler was doing. Results belong in
//! `docs/performance.md` together with the machine they came from — comparing them across
//! machines is meaningless.

use std::hint::black_box;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use engine_core::input::{InputEvent, InputState, KeyCode};
use engine_core::jobs::JobSystem;
use engine_core::memory::{Arena, FrameAllocator};
use engine_core::profiling::Profiler;
use engine_core::threading::recommended_worker_count;

/// Iterations per sample for the micro-benchmarks.
const DEFAULT_ITERATIONS: u32 = 100_000;

/// Samples taken per benchmark; the median is reported.
const SAMPLES: u32 = 9;

/// Jobs submitted per job-system measurement.
const JOB_COUNT: usize = 1_000;

fn main() -> ExitCode {
    if let Err(error) = engine_core::logging::init() {
        eprintln!("fatal: failed to install logger: {error}");
        return ExitCode::FAILURE;
    }

    let mut iterations = DEFAULT_ITERATIONS;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--iterations" => {
                let Some(value) = arguments.next().and_then(|value| value.parse().ok()) else {
                    eprintln!("error: --iterations needs a number");
                    return ExitCode::FAILURE;
                };
                iterations = value;
            }
            "-h" | "--help" => {
                println!(
                    "usage: cpu_bench [--iterations <count>]\n\n\
                     Measures allocator, input and job-system overhead. Report the median\n\
                     figures in docs/performance.md along with the machine."
                );
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("error: unknown option {other}");
                return ExitCode::FAILURE;
            }
        }
    }

    println!("iterations per sample: {iterations}, samples: {SAMPLES}\n");

    benchmark_allocators(iterations);
    benchmark_input(iterations);
    benchmark_profiler(iterations);
    benchmark_jobs();

    ExitCode::SUCCESS
}

/// Runs `body` `SAMPLES` times and returns the median nanoseconds per iteration.
fn measure(iterations: u32, mut body: impl FnMut(u32)) -> f64 {
    let mut samples = Vec::with_capacity(SAMPLES as usize);
    for _ in 0..SAMPLES {
        let start = Instant::now();
        body(iterations);
        samples.push(start.elapsed());
    }
    samples.sort_unstable();
    samples[samples.len() / 2].as_secs_f64() * 1e9 / f64::from(iterations)
}

/// Prints one result line.
fn report(name: &str, nanoseconds: f64) {
    println!("{name:<38} {nanoseconds:>10.2} ns");
}

fn benchmark_allocators(iterations: u32) {
    println!("-- allocators --");

    // Bump allocation: one aligned allocation per iteration, resetting between samples so
    // the arena never fills.
    let mut arena = Arena::with_capacity(64 * 1024 * 1024);
    let bump = measure(iterations, |count| {
        for _ in 0..count {
            let value = arena.alloc(black_box(1_u64));
            black_box(value);
        }
        arena.reset();
    });
    report("arena alloc<u64>", bump);

    // The same allocation through the system allocator, for scale. Not a fair comparison
    // of features — Box frees individually — but it is the alternative the frame path
    // would otherwise use.
    let boxed = measure(iterations, |count| {
        for _ in 0..count {
            let value = Box::new(black_box(1_u64));
            black_box(&value);
        }
    });
    report("Box::new(u64) (for scale)", boxed);

    // A 256-byte slice, closer to a real transient allocation than a scalar.
    let mut slice_arena = Arena::with_capacity(64 * 1024 * 1024);
    let source = [7_u8; 256];
    let slice = measure(iterations, |count| {
        for _ in 0..count {
            let copy = slice_arena.alloc_slice(black_box(&source));
            black_box(copy);
        }
        slice_arena.reset();
    });
    report("arena alloc_slice(256 B)", slice);

    // The per-frame cost the engine actually pays: one reset per frame, whatever the
    // frame allocated.
    let mut frame = FrameAllocator::with_capacity(1024 * 1024);
    let reset = measure(iterations, |count| {
        for _ in 0..count {
            let _ = frame.arena().alloc(black_box(1_u32));
            frame.begin_frame();
        }
    });
    report("frame allocator alloc + reset", reset);

    println!();
}

fn benchmark_input(iterations: u32) {
    println!("-- input --");

    let mut state = InputState::new();
    let events = [
        InputEvent::KeyDown {
            key: KeyCode::W,
            repeat: false,
        },
        InputEvent::MouseMoved { x: 10, y: 20 },
        InputEvent::MouseButtonDown {
            button: engine_core::input::MouseButton::Left,
        },
        InputEvent::KeyUp { key: KeyCode::W },
    ];

    // A whole frame's input work: roll transitions, then apply four events.
    let frame = measure(iterations, |count| {
        for _ in 0..count {
            state.begin_frame();
            for event in events {
                state.apply(black_box(event));
            }
        }
    });
    report("input begin_frame + 4 events", frame);

    let query = measure(iterations, |count| {
        for _ in 0..count {
            black_box(state.is_key_down(black_box(KeyCode::W)));
            black_box(state.was_key_pressed(black_box(KeyCode::Space)));
        }
    });
    report("input 2 queries", query);

    println!();
}

fn benchmark_profiler(iterations: u32) {
    println!("-- profiler --");
    println!(
        "instrumentation compiled in: {}",
        engine_core::profiling::PROFILING_ENABLED
    );

    // The engine profiler as the build has it: enabled in a debug or `profiling` build,
    // inert in a plain release build. This is the number that matters, because it is what
    // the shipped frame actually pays.
    let active = Profiler::default();
    active.begin_frame(0);
    let enabled = measure(iterations, |count| {
        for _ in 0..count {
            let _scope = active.scope(black_box("Scope"));
        }
        active.begin_frame(0);
    });
    report("profile scope (this build)", enabled);

    // A profiler forced off, for the floor: what an instrumented call site costs when
    // recording is disabled.
    let inert = Profiler::disabled();
    let disabled = measure(iterations, |count| {
        for _ in 0..count {
            let _scope = inert.scope(black_box("Scope"));
        }
    });
    report("profile scope (recording off)", disabled);

    println!();
}

fn benchmark_jobs() {
    println!("-- job system --");
    println!("workload: {JOB_COUNT} jobs, each a fixed integer mix");

    for job_work in [100_u64, 10_000] {
        println!("\nwork per job: {job_work} iterations");

        let sequential = median_duration(|| {
            let mut total = 0_u64;
            for index in 0..JOB_COUNT {
                total += synthetic_work(index as u64, job_work);
            }
            black_box(total);
        });
        report_duration("sequential", sequential, sequential);

        for workers in worker_counts() {
            let Ok(jobs) = JobSystem::with_workers(workers) else {
                eprintln!("warning: could not start {workers} workers");
                continue;
            };

            let elapsed = median_duration(|| {
                let handle = jobs.submit_batch((0..JOB_COUNT).map(move |index| {
                    move || {
                        black_box(synthetic_work(index as u64, job_work));
                    }
                }));
                if handle.wait().is_err() {
                    eprintln!("warning: a job panicked");
                }
            });
            report_duration(&format!("{workers} worker(s)"), elapsed, sequential);

            if let Err(error) = jobs.shutdown() {
                eprintln!("warning: {}", error.chain());
            }
        }
    }

    // Submission overhead on its own: empty jobs, so the number is the cost of the queue,
    // the allocation for the boxed closure, and the wake-up.
    let Ok(jobs) = JobSystem::new() else {
        return;
    };
    let empty = median_duration(|| {
        let handle = jobs.submit_batch((0..JOB_COUNT).map(|_| || {}));
        let _ = handle.wait();
    });
    println!(
        "\nsubmit + wait, {JOB_COUNT} empty jobs: {:.3} ms total, {:.0} ns per job",
        empty.as_secs_f64() * 1e3,
        empty.as_secs_f64() * 1e9 / JOB_COUNT as f64
    );

    let single = median_duration(|| {
        let handle = jobs.submit(|| {});
        let _ = handle.wait();
    });
    println!(
        "submit + wait, 1 empty job:          {:.0} ns",
        single.as_secs_f64() * 1e9
    );

    if let Err(error) = jobs.shutdown() {
        eprintln!("warning: {}", error.chain());
    }
}

/// Worker counts worth measuring on this machine.
fn worker_counts() -> Vec<usize> {
    let recommended = recommended_worker_count();
    let mut counts = vec![1];
    if recommended >= 4 {
        counts.push(recommended / 2);
    }
    if recommended > 1 {
        counts.push(recommended);
    }
    counts.dedup();
    counts
}

/// Median wall-clock duration of `body` over `SAMPLES` runs.
fn median_duration(mut body: impl FnMut()) -> Duration {
    let mut samples = Vec::with_capacity(SAMPLES as usize);
    for _ in 0..SAMPLES {
        let start = Instant::now();
        body();
        samples.push(start.elapsed());
    }
    samples.sort_unstable();
    samples[samples.len() / 2]
}

/// Prints a job-system line with its speed-up against sequential execution.
fn report_duration(name: &str, elapsed: Duration, sequential: Duration) {
    let speedup = sequential.as_secs_f64() / elapsed.as_secs_f64().max(f64::MIN_POSITIVE);
    println!(
        "  {name:<20} {:>9.3} ms   {speedup:>5.2}x",
        elapsed.as_secs_f64() * 1e3
    );
}

/// A deterministic integer workload with no allocation and no memory traffic.
///
/// Deliberately not a sleep: a sleeping "job" measures the scheduler, not the job system.
fn synthetic_work(seed: u64, iterations: u64) -> u64 {
    let mut value = seed | 1;
    for _ in 0..iterations {
        // A cheap mixing step the optimizer cannot fold away.
        value = value
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        value ^= value >> 33;
    }
    value
}
