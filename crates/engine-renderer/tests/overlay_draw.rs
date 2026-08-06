//! Debug overlay integration tests.
//!
//! Only compiled when the `debug-overlay` feature is on; a shipping build has no overlay
//! to test. Takes the same process-wide GPU lock as the other GPU tests.

#![cfg(feature = "debug-overlay")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Mutex, MutexGuard, PoisonError};

use engine_platform::{Window, WindowConfig};
use engine_renderer::device::RenderDevice;
use engine_renderer::{Renderer, RendererConfig, RendererError};

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
fn overlay_text_produces_geometry_and_an_extra_draw_call() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    let (_window, mut renderer) = renderer_for("overlay draw test");

    // Without text, only the scene is drawn.
    renderer.render([0.0; 4]).expect("frame must record");
    renderer.present().expect("frame must present");
    assert_eq!(renderer.draw_calls_last_frame(), 1);
    assert_eq!(renderer.overlay_vertex_count(), 0);

    renderer.set_overlay_text(&["FPS 1234", "CPU 0.166 MS"]);
    renderer.render([0.0; 4]).expect("frame must record");
    renderer.present().expect("frame must present");

    assert_eq!(
        renderer.draw_calls_last_frame(),
        2,
        "the overlay adds exactly one draw call"
    );
    assert!(
        renderer.overlay_vertex_count() > 0,
        "overlay text must produce vertices"
    );
    assert_eq!(
        renderer.overlay_vertex_count() % 6,
        0,
        "overlay geometry is whole quads"
    );

    renderer.wait_for_idle().expect("GPU must go idle");
}

#[test]
fn disabling_the_overlay_removes_its_draw_call() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    let (_window, mut renderer) = renderer_for("overlay toggle test");

    renderer.set_overlay_text(&["FPS 1234"]);
    renderer.render([0.0; 4]).expect("frame must record");
    renderer.present().expect("frame must present");
    assert_eq!(renderer.draw_calls_last_frame(), 2);

    renderer.set_overlay_enabled(false);
    assert!(!renderer.overlay_enabled());
    renderer.render([0.0; 4]).expect("frame must record");
    renderer.present().expect("frame must present");

    assert_eq!(
        renderer.draw_calls_last_frame(),
        1,
        "a disabled overlay must not be drawn"
    );
    assert_eq!(renderer.overlay_vertex_count(), 0);

    renderer.wait_for_idle().expect("GPU must go idle");
}

#[test]
fn overlay_survives_many_frames_and_a_resize() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    let (_window, mut renderer) = renderer_for("overlay resize test");

    renderer.set_overlay_text(&["FPS 5861", "CPU 0.168 MS", "GPU 0.045 MS", "1920X1080"]);

    // More frames than slots, so every per-slot upload buffer is exercised and reused.
    for _ in 0..(engine_renderer::FRAME_COUNT * 3) {
        renderer.render([0.0; 4]).expect("frame must record");
        renderer.present().expect("frame must present");
        assert!(renderer.overlay_vertex_count() > 0);
    }

    renderer.resize(1024, 768).expect("resize must succeed");
    renderer.set_overlay_text(&["FPS 5861", "1024X768"]);
    renderer.render([0.0; 4]).expect("frame must record");
    renderer.present().expect("frame must present");
    assert!(renderer.overlay_vertex_count() > 0);

    renderer.wait_for_idle().expect("GPU must go idle");
}

/// Proves the overlay geometry actually rasterizes, by rendering it offscreen and
/// inspecting pixels — the same standard the triangle is held to.
#[test]
fn overlay_geometry_rasterizes_in_the_top_left_corner() {
    use engine_renderer::offscreen::render_vertices;
    use engine_renderer::overlay::OverlayGeometry;

    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }

    let size = 256;
    let mut geometry = OverlayGeometry::new();
    geometry.set_cell_pixels(3.0);
    geometry.begin();
    geometry.push_line(0, "TEST", size, size);
    assert!(!geometry.is_empty(), "text must generate geometry");

    let image = render_vertices(size, size, [0.0, 0.0, 0.0, 1.0], geometry.vertices())
        .expect("offscreen render must succeed");

    let lit = image.count_pixels(|p| p[0] > 8 || p[1] > 8 || p[2] > 8);
    assert!(lit > 0, "overlay text must produce visible pixels");

    // The text sits near the top-left, so the opposite corner must stay clear.
    let far_corner = image.pixel(size - 1, size - 1).expect("corner is inside");
    assert!(
        far_corner[0] <= 8 && far_corner[1] <= 8 && far_corner[2] <= 8,
        "overlay must not cover the whole target, corner was {far_corner:?}"
    );

    // Every lit pixel should be in the upper-left region for a single short line.
    let stray = (0..size)
        .flat_map(|y| (0..size).map(move |x| (x, y)))
        .filter(|&(x, y)| x > size / 2 || y > size / 4)
        .filter(|&(x, y)| {
            let p = image.pixel(x, y).unwrap_or([0; 4]);
            p[0] > 8 || p[1] > 8 || p[2] > 8
        })
        .count();
    assert_eq!(stray, 0, "one short line must stay in the top-left region");

    eprintln!("overlay lit pixels: {lit}");
}

#[test]
fn empty_text_draws_nothing() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    let (_window, mut renderer) = renderer_for("overlay empty test");

    renderer.set_overlay_text::<&str>(&[]);
    renderer.render([0.0; 4]).expect("frame must record");
    renderer.present().expect("frame must present");
    assert_eq!(renderer.draw_calls_last_frame(), 1);

    renderer.set_overlay_text(&["   "]);
    renderer.render([0.0; 4]).expect("frame must record");
    renderer.present().expect("frame must present");
    assert_eq!(
        renderer.draw_calls_last_frame(),
        1,
        "whitespace has no lit cells, so there is nothing to draw"
    );

    renderer.wait_for_idle().expect("GPU must go idle");
}
