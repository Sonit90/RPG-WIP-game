//! Renderer benchmark executable.
//!
//! Lives in its own crate with its own binary, so no benchmark code can reach the game
//! executable — `cargo build --release --bin mygame` never compiles this.
//!
//! Everything is rendered offscreen: no window, no swap chain, no `Present`. Milestone 7
//! measured presentation costing three to four times as much CPU as recording a frame and
//! not tracking renderer cost, so presenting here would drown the numbers this benchmark
//! exists to produce.
//!
//! ```text
//! cargo run --release --bin performance_test
//! cargo run --release --bin performance_test -- --instances 1000 --frames 500
//! ```

use std::process::ExitCode;

use engine_core::time::FrameStats;
use engine_renderer::bench::{BenchHarness, BenchResult, DrawMode};

/// Instance counts benchmarked when none are given.
const DEFAULT_INSTANCE_COUNTS: [u32; 5] = [1, 100, 1_000, 10_000, 100_000];

/// Measured frames per configuration.
const DEFAULT_FRAMES: u32 = 300;

/// Unmeasured frames before each configuration, to absorb driver warm-up.
const WARMUP_FRAMES: u32 = 30;

/// Benchmark resolution.
const WIDTH: u32 = 1920;
const HEIGHT: u32 = 1080;

fn main() -> ExitCode {
    if let Err(error) = engine_core::logging::init() {
        eprintln!("fatal: failed to install logger: {error}");
        return ExitCode::FAILURE;
    }

    let mut instance_counts = Vec::from(DEFAULT_INSTANCE_COUNTS);
    let mut frames = DEFAULT_FRAMES;

    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--instances" => {
                let Some(value) = arguments.next().and_then(|v| v.parse().ok()) else {
                    eprintln!("error: --instances needs a number");
                    return ExitCode::FAILURE;
                };
                instance_counts = vec![value];
            }
            "--frames" => {
                let Some(value) = arguments.next().and_then(|v| v.parse().ok()) else {
                    eprintln!("error: --frames needs a number");
                    return ExitCode::FAILURE;
                };
                frames = value;
            }
            "-h" | "--help" => {
                println!(
                    "usage: performance_test [--instances <count>] [--frames <count>]\n\n\
                     Renders offscreen and compares one draw call per instance against a\n\
                     single instanced draw."
                );
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("error: unknown option {other:?}");
                return ExitCode::FAILURE;
            }
        }
    }

    match run(&instance_counts, frames) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fatal: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(instance_counts: &[u32], frames: u32) -> Result<(), engine_renderer::RendererError> {
    let mut harness = BenchHarness::new(WIDTH, HEIGHT)?;

    println!("=== RENDERER BENCHMARK ===");
    println!();
    println!("Resolution: {WIDTH}x{HEIGHT}");
    println!("GPU: {}", harness.adapter_name());
    println!("Mode: offscreen, no swap chain, no present");
    println!("Frames per configuration: {frames} (plus {WARMUP_FRAMES} warm-up)");
    println!();

    let mut summary = Vec::new();

    for &instances in instance_counts {
        harness.set_instances(instances)?;

        // The two modes are interleaved per instance count rather than run as two
        // separate passes, so machine drift cannot land entirely on one mode. This rule
        // exists because ignoring it produced two retracted results earlier in Phase 0.
        let a = harness.run(DrawMode::PerInstanceDrawCalls, frames, WARMUP_FRAMES)?;
        let b = harness.run(DrawMode::SingleInstancedDraw, frames, WARMUP_FRAMES)?;

        report(&a);
        report(&b);
        summary.push((instances, a, b));
    }

    println!("=== SUMMARY: draw calls per frame ===");
    println!();
    println!(
        "{:>10} | {:>10} | {:>10} | {:>10} | {:>10} | {:>8}",
        "instances", "A cpu ms", "B cpu ms", "A gpu ms", "B gpu ms", "cpu x"
    );
    for (instances, a, b) in &summary {
        let ratio = if b.cpu.mean_ms > 0.0 {
            a.cpu.mean_ms / b.cpu.mean_ms
        } else {
            0.0
        };
        println!(
            "{instances:>10} | {:>10.4} | {:>10.4} | {:>10.4} | {:>10.4} | {ratio:>7.1}x",
            a.cpu.mean_ms, b.cpu.mean_ms, a.gpu.mean_ms, b.gpu.mean_ms
        );
    }
    println!();
    println!(
        "cpu x is how much more CPU time mode A spends recording than mode B for the same picture."
    );

    Ok(())
}

/// Prints one configuration's results.
fn report(result: &BenchResult) {
    println!(
        "Scene: {} instances - {}",
        result.instances,
        result.mode.label()
    );
    println!();
    println!("Draw calls: {}", result.draw_calls);
    println!("Triangles:  {}", result.triangles);
    println!();
    println!("CPU (record + submit):");
    print_stats(&result.cpu);
    println!("GPU (timestamp queries):");
    print_stats(&result.gpu);
    println!("Serialized frame (record + submit + wait):");
    print_stats(&result.serialized);
    println!(
        "  implied fps: {:.0} (lower bound: no frame pipelining, no present)",
        result.serialized_fps()
    );
    println!();
}

/// Prints one set of statistics.
fn print_stats(stats: &FrameStats) {
    if stats.count == 0 {
        println!("  no samples");
        return;
    }
    println!(
        "  mean: {:.4} ms   p50: {:.4} ms   p95: {:.4} ms   p99: {:.4} ms   min: {:.4}   max: {:.4}   n={}",
        stats.mean_ms,
        stats.p50_ms,
        stats.p95_ms,
        stats.p99_ms,
        stats.min_ms,
        stats.max_ms,
        stats.count
    );
}
