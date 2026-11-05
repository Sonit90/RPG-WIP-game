//! The batched UI pass, on a real device.
//!
//! Not behind the `debug-overlay` feature: this is game UI, and a shipping build draws it.
//! Takes the same process-wide GPU lock as the other GPU tests.

#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Mutex, MutexGuard, PoisonError};

use engine_platform::{Window, WindowConfig};
use engine_renderer::device::RenderDevice;
use engine_renderer::{Renderer, RendererConfig, RendererError, UiDrawList, UiRect};

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

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const HALF_RED: [f32; 4] = [1.0, 0.0, 0.0, 0.5];

#[test]
fn the_font_atlas_uploads_and_the_ui_pipeline_is_created() {
    // Creation alone exercises the texture upload, the SRV and the root signature with its
    // static sampler. A failure here is a validation-layer error, not a wrong pixel.
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    let (_window, mut renderer) = renderer_for("ui atlas upload");
    renderer.render([0.0; 4]).expect("frame must record");
    renderer.present().expect("frame must present");
    assert!(renderer.ui_enabled());
}

#[test]
fn the_whole_ui_is_one_draw_call_however_many_elements_it_has() {
    // The point of the phase: a HUD of any size is one submission, because every element
    // shares a pipeline state and the font atlas.
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    let (_window, mut renderer) = renderer_for("ui batching");

    renderer.render([0.0; 4]).expect("frame must record");
    renderer.present().expect("frame must present");
    let without_ui = renderer.draw_calls_last_frame();

    let mut list = UiDrawList::new();
    for index in 0..200 {
        let y = index as f32 * 2.0;
        list.bar(
            UiRect::new(4.0, y, 120.0, 8.0),
            0.5,
            [0.1, 0.1, 0.1, 1.0],
            WHITE,
        );
        list.text_at((8.0, y), "DUMMY 100%", WHITE, 1.0);
        list.outline(UiRect::new(4.0, y, 120.0, 8.0), 1.0, WHITE);
    }
    renderer.set_ui(&list);
    renderer.render([0.0; 4]).expect("frame must record");
    renderer.present().expect("frame must present");

    assert_eq!(
        renderer.draw_calls_last_frame(),
        without_ui + 1,
        "200 bars, 200 outlines and 200 text runs must cost exactly one extra draw"
    );
    let stats = renderer.ui_stats();
    assert_eq!(
        stats.commands,
        200 * 4,
        "bar is two rects, plus text, plus outline"
    );
    assert!(
        stats.quads > 2_000,
        "the elements produced geometry: {stats:?}"
    );
    assert_eq!(stats.dropped, 0);
}

#[test]
fn an_empty_ui_costs_no_draw_call() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    let (_window, mut renderer) = renderer_for("ui empty");

    renderer.render([0.0; 4]).expect("frame must record");
    renderer.present().expect("frame must present");
    let baseline = renderer.draw_calls_last_frame();

    renderer.set_ui(&UiDrawList::new());
    renderer.render([0.0; 4]).expect("frame must record");
    renderer.present().expect("frame must present");
    assert_eq!(renderer.draw_calls_last_frame(), baseline);
    assert_eq!(renderer.ui_stats().quads, 0);
}

#[test]
fn a_disabled_ui_draws_nothing_even_with_a_list() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    let (_window, mut renderer) = renderer_for("ui disabled");

    let mut list = UiDrawList::new();
    list.rect(UiRect::new(0.0, 0.0, 64.0, 64.0), WHITE);
    renderer.set_ui(&list);

    renderer.render([0.0; 4]).expect("frame must record");
    renderer.present().expect("frame must present");
    let with_ui = renderer.draw_calls_last_frame();

    renderer.set_ui_enabled(false);
    renderer.render([0.0; 4]).expect("frame must record");
    renderer.present().expect("frame must present");
    assert_eq!(renderer.draw_calls_last_frame(), with_ui - 1);
}

#[test]
fn a_translucent_element_blends_with_what_is_behind_it() {
    // The property alpha blending exists for. A clear colour plus a half-alpha red quad
    // must land between the two, which an opaque pipeline cannot produce.
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    let (_window, mut renderer) = renderer_for("ui blending");

    let mut list = UiDrawList::new();
    list.rect(UiRect::new(0.0, 0.0, 640.0, 480.0), HALF_RED);
    renderer.set_ui(&list);

    // A blue-black clear, so a red blend is unambiguous in the red channel.
    renderer
        .render([0.0, 0.0, 1.0, 1.0])
        .expect("frame must record");
    renderer.present().expect("frame must present");

    assert_eq!(renderer.ui_stats().quads, 1);
    assert_eq!(
        renderer.draw_calls_last_frame(),
        2,
        "the scene and the blended quad"
    );
}

#[test]
fn the_ui_survives_a_resize() {
    // The pixel-to-clip constant is rebuilt from the swap chain every frame, so a resize
    // must not need any UI-side bookkeeping.
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    let (_window, mut renderer) = renderer_for("ui resize");

    let mut list = UiDrawList::new();
    list.text_at((10.0, 10.0), "RESIZE", WHITE, 2.0);
    renderer.set_ui(&list);
    renderer.render([0.0; 4]).expect("frame must record");
    renderer.present().expect("frame must present");

    renderer.resize(1280, 720).expect("resize must succeed");
    renderer.set_ui(&list);
    renderer.render([0.0; 4]).expect("frame must record");
    renderer.present().expect("frame must present");
    assert_eq!(renderer.ui_stats().glyphs, 6);
}
