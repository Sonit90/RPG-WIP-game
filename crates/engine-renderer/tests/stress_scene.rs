//! GPU integration tests for the stress-test scene.
//!
//! Takes the same process-wide GPU lock as the other GPU tests; see `d3d12_init.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Mutex, MutexGuard, PoisonError};

use engine_platform::{Window, WindowConfig};
use engine_renderer::device::GpuContext;
use engine_renderer::offscreen::render_vertices;
use engine_renderer::stress::{STRESS_LEVELS, build_grid};
use engine_renderer::{Renderer, RendererConfig, RendererError};

static GPU: Mutex<()> = Mutex::new(());

fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU.lock().unwrap_or_else(PoisonError::into_inner)
}

fn gpu_available() -> bool {
    match GpuContext::new(false, false) {
        Ok(_) => true,
        Err(RendererError::NoSuitableAdapter { examined }) => {
            eprintln!("SKIP: no D3D12 adapter on this machine ({examined} examined)");
            false
        }
        Err(error) => panic!("unexpected D3D12 initialization failure: {error}"),
    }
}

fn renderer_for(title: &str) -> (Window, Renderer) {
    let window = Window::new(&WindowConfig::new(title, 640, 480).with_visible(false))
        .expect("hidden window creation must succeed");
    let config = RendererConfig::new(640, 480)
        .with_debug_layer(true)
        .with_break_on_validation_error(false);
    let renderer =
        Renderer::new(window.raw_handle(), &config).expect("renderer creation must succeed");
    (window, renderer)
}

#[test]
fn every_stress_level_loads_and_renders() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    let (_window, mut renderer) = renderer_for("stress level test");

    for level in STRESS_LEVELS {
        renderer
            .set_scene_triangles(level)
            .unwrap_or_else(|error| panic!("level {level} must load: {error}"));
        assert_eq!(renderer.scene_triangle_count(), level);

        // More frames than slots, so every frame slot draws the new scene.
        for _ in 0..(engine_renderer::FRAME_COUNT + 1) {
            renderer
                .render([0.0, 0.0, 0.0, 1.0])
                .expect("frame records");
            renderer.present().expect("frame presents");
        }

        assert_eq!(
            renderer.triangles_last_frame(),
            u64::from(level),
            "the frame must submit exactly the scene's triangles"
        );
    }

    renderer.wait_for_idle().expect("GPU must go idle");
}

#[test]
fn switching_levels_back_and_forth_is_stable() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    let (_window, mut renderer) = renderer_for("stress toggle test");

    for level in [1, 10_000, 1, 100_000, 100, 1] {
        renderer.set_scene_triangles(level).expect("level loads");
        renderer.render([0.0; 4]).expect("frame records");
        renderer.present().expect("frame presents");
        assert_eq!(renderer.scene_triangle_count(), level);
    }

    renderer.wait_for_idle().expect("GPU must go idle");
}

#[test]
fn reloading_the_same_level_is_a_no_op() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    let (_window, mut renderer) = renderer_for("stress reload test");

    renderer.set_scene_triangles(100).expect("level loads");
    renderer.render([0.0; 4]).expect("frame records");
    renderer.present().expect("frame presents");
    let presented = renderer.frames_presented();

    // A repeat request must not disturb a frame in flight or change the scene.
    renderer.set_scene_triangles(100).expect("repeat is fine");
    assert_eq!(renderer.scene_triangle_count(), 100);
    assert_eq!(renderer.frames_presented(), presented);
}

#[test]
fn a_zero_triangle_scene_is_rejected() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    let (_window, mut renderer) = renderer_for("stress zero test");

    assert!(renderer.set_scene_triangles(0).is_err());
    assert_eq!(
        renderer.scene_triangle_count(),
        1,
        "a rejected request must leave the scene alone"
    );
}

/// Every level must cover a similar fraction of the screen.
///
/// This is the property that makes the levels comparable: the grid keeps total coverage
/// roughly constant and shrinks the triangles instead, so a jump from 100 to 100,000
/// triangles measures geometry throughput rather than a change in how many pixels are
/// shaded. A level that lit almost nothing, or one that filled the screen while others
/// did not, would invalidate the comparison.
#[test]
fn every_level_covers_a_similar_fraction_of_the_screen() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }

    let size = 256;
    let mut vertices = Vec::new();

    for level in [1_u32, 100, 1_000] {
        build_grid(level, &mut vertices);
        let image = render_vertices(size, size, [0.0, 0.0, 0.0, 1.0], &vertices)
            .unwrap_or_else(|error| panic!("level {level} must render: {error}"));
        let lit = image.count_pixels(|p| p[0] > 8 || p[1] > 8 || p[2] > 8);
        let fraction = lit as f64 / f64::from(size * size);
        eprintln!("level {level}: {:.1}% of pixels lit", fraction * 100.0);

        // A triangle fills half of its cell, scaled by CELL_FILL^2 (0.85^2), so the
        // ideal coverage is ~36%. Small triangles lose a little to rasterization rules,
        // which is why the lower bound has room.
        assert!(
            (0.20..=0.45).contains(&fraction),
            "level {level} lit {:.1}% of the image, outside the comparable band",
            fraction * 100.0
        );
    }
}
