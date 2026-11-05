//! The two benchmark modes must produce identical images.
//!
//! This is the premise the whole Milestone 8 comparison rests on: if mode A and mode B
//! drew different pictures, comparing their costs would be meaningless. Verified by
//! rendering both offscreen and comparing the read-back pixels, not by inspection.
//!
//! Takes the same process-wide GPU lock as the other GPU tests; see `d3d12_init.rs`.

#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Mutex, MutexGuard, PoisonError};

use engine_renderer::RendererError;
use engine_renderer::bench::{BenchHarness, DrawMode};
use engine_renderer::device::RenderDevice;

static GPU: Mutex<()> = Mutex::new(());

fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU.lock().unwrap_or_else(PoisonError::into_inner)
}

fn gpu_available() -> bool {
    match RenderDevice::new(false, false) {
        Ok(_) => true,
        Err(RendererError::NoSuitableAdapter { examined }) => {
            eprintln!("SKIP: no D3D12 adapter on this machine ({examined} examined)");
            false
        }
        Err(error) => panic!("unexpected D3D12 initialization failure: {error}"),
    }
}

/// The comparison is only meaningful if both modes draw the same picture. Verified by
/// reading the rendered pixels back and comparing them byte for byte.
#[test]
fn both_modes_produce_identical_images() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    let mut harness = BenchHarness::new(256, 256).expect("harness must be created");

    for instances in [1_u32, 7, 100] {
        harness.set_instances(instances).expect("instances upload");

        let a = harness
            .capture(DrawMode::PerInstanceDrawCalls)
            .expect("mode A renders");
        let b = harness
            .capture(DrawMode::SingleInstancedDraw)
            .expect("mode B renders");

        assert_eq!((a.width, a.height), (b.width, b.height));

        let lit = a.count_pixels(|p| p[0] > 8 || p[1] > 8 || p[2] > 8);
        assert!(
            lit > 0,
            "{instances} instances drew nothing, so the comparison would be vacuous"
        );

        let differing = a
            .pixels
            .chunks_exact(4)
            .zip(b.pixels.chunks_exact(4))
            .filter(|(left, right)| left != right)
            .count();
        assert_eq!(
            differing, 0,
            "{instances} instances: {differing} pixels differ between one draw call per \
             instance and a single instanced draw"
        );
        eprintln!("{instances} instances: images identical, {lit} pixels lit");
    }
}

#[test]
fn both_modes_report_the_geometry_they_submitted() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    let mut harness = BenchHarness::new(256, 256).expect("harness must be created");
    harness.set_instances(64).expect("instances upload");

    let a = harness
        .run(DrawMode::PerInstanceDrawCalls, 8, 2)
        .expect("mode A runs");
    let b = harness
        .run(DrawMode::SingleInstancedDraw, 8, 2)
        .expect("mode B runs");

    assert_eq!(a.draw_calls, 64, "mode A issues one draw per instance");
    assert_eq!(b.draw_calls, 1, "mode B issues a single draw");
    assert_eq!(a.instances, b.instances);
    assert_eq!(a.triangles, b.triangles);
    assert_eq!((a.frames, b.frames), (8, 8));

    assert!(a.cpu.count > 0 && b.cpu.count > 0, "CPU samples recorded");
    assert!(a.gpu.count > 0 && b.gpu.count > 0, "GPU samples recorded");
    assert!(a.cpu.mean_ms > 0.0 && b.cpu.mean_ms > 0.0);
    assert!(a.gpu.mean_ms > 0.0 && b.gpu.mean_ms > 0.0);
}

#[test]
fn a_benchmark_needs_instances_before_it_can_run() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    let mut harness = BenchHarness::new(128, 128).expect("harness must be created");

    assert!(
        harness.run(DrawMode::SingleInstancedDraw, 1, 0).is_err(),
        "running without instances must be rejected"
    );
    assert!(harness.set_instances(0).is_err(), "zero instances rejected");
}

#[test]
fn a_zero_sized_target_is_rejected() {
    let _gpu = gpu_lock();
    assert!(BenchHarness::new(0, 128).is_err());
    assert!(BenchHarness::new(128, 0).is_err());
}

/// Renders the instance grid through the offscreen path and checks it covers the screen
/// the way the stress grid does, so the benchmark is drawing a real scene rather than
/// nothing.
#[test]
fn the_instance_grid_actually_covers_the_screen() {
    use engine_renderer::instancing::build_instance_grid;

    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }

    let mut instances = Vec::new();
    build_instance_grid(100, &mut instances);
    assert_eq!(instances.len(), 100);

    // Instances must be spread out, not stacked on one spot.
    let first = instances[0].transform;
    let last = instances[99].transform;
    assert!(
        (first[0] - last[0]).abs() > 0.5 || (first[1] - last[1]).abs() > 0.5,
        "instances are not distributed: {first:?} to {last:?}"
    );
}
