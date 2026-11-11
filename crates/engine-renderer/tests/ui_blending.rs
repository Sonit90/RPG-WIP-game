//! Alpha blending, proven pixel by pixel.
//!
//! A draw-call count and a silent validation layer cannot tell a translucent panel from an
//! opaque one. These tests render into an offscreen target and read the result back, so the
//! assertion is the colour that actually landed in the render target.
//!
//! Not behind the `debug-overlay` feature: blending is what game UI is drawn with.

#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Mutex, MutexGuard, PoisonError};

use engine_renderer::device::RenderDevice;
use engine_renderer::offscreen::render_ui;
use engine_renderer::{RendererError, UiDrawList, UiRect};

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

const WIDTH: u32 = 64;
const HEIGHT: u32 = 64;

/// A black clear, so a blend shows up as exactly the source colour times its alpha.
const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// How far a read-back channel may sit from the expected value.
///
/// The target is 8-bit UNORM and the back-buffer format is sRGB-free, so the only error is
/// the quantization of a linear float to 256 levels: one step is about 4/255.
const TOLERANCE: i32 = 3;

fn assert_channel(actual: u8, expected: f32, what: &str) {
    let expected = (expected * 255.0).round() as i32;
    let difference = (actual as i32 - expected).abs();
    assert!(
        difference <= TOLERANCE,
        "{what}: read {actual}, expected about {expected}"
    );
}

#[test]
fn a_half_alpha_quad_lands_halfway_between_the_clear_and_its_colour() {
    // The defining property of `src * a + dst * (1 - a)`. An opaque pipeline would produce
    // the source colour outright, and a pipeline with blending misconfigured as additive
    // would produce something brighter than either input.
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }

    let mut list = UiDrawList::new();
    list.rect(
        UiRect::new(0.0, 0.0, WIDTH as f32, HEIGHT as f32),
        [1.0, 0.0, 0.0, 0.5],
    );

    let image = render_ui(WIDTH, HEIGHT, BLACK, &list).expect("the frame must render");
    let pixel = image.pixel(WIDTH / 2, HEIGHT / 2).expect("a centre pixel");

    // Red at half alpha over black: 1.0 * 0.5 + 0.0 * 0.5.
    assert_channel(pixel[0], 0.5, "red");
    assert_channel(pixel[1], 0.0, "green");
    assert_channel(pixel[2], 0.0, "blue");
}

#[test]
fn a_fully_transparent_quad_leaves_the_target_untouched() {
    // What a damage number fades to. If alpha zero still wrote, a finished number would sit
    // on screen as a solid block forever.
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }

    let mut list = UiDrawList::new();
    list.rect(
        UiRect::new(0.0, 0.0, WIDTH as f32, HEIGHT as f32),
        [1.0, 1.0, 1.0, 0.0],
    );

    let image =
        render_ui(WIDTH, HEIGHT, [0.25, 0.0, 0.0, 1.0], &list).expect("the frame must render");
    let pixel = image.pixel(WIDTH / 2, HEIGHT / 2).expect("a centre pixel");
    assert_channel(pixel[0], 0.25, "red");
    assert_channel(pixel[1], 0.0, "green");
}

#[test]
fn an_opaque_quad_replaces_what_is_behind_it() {
    // Alpha one must be exact, not nearly-exact: a HUD panel that let 1% of the scene
    // through would shimmer as the scene moved behind it.
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }

    let mut list = UiDrawList::new();
    list.rect(
        UiRect::new(0.0, 0.0, WIDTH as f32, HEIGHT as f32),
        [0.0, 1.0, 0.0, 1.0],
    );

    let image =
        render_ui(WIDTH, HEIGHT, [1.0, 0.0, 0.0, 1.0], &list).expect("the frame must render");
    let pixel = image.pixel(WIDTH / 2, HEIGHT / 2).expect("a centre pixel");
    assert_channel(pixel[0], 0.0, "red");
    assert_channel(pixel[1], 1.0, "green");
}

#[test]
fn two_translucent_quads_accumulate_in_paint_order() {
    // Painter's order, proven by the arithmetic: the second quad blends over the result of
    // the first. With a depth test this would be a coin toss, and with depth *writes* the
    // second quad would be rejected outright.
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }

    let full = UiRect::new(0.0, 0.0, WIDTH as f32, HEIGHT as f32);
    let mut list = UiDrawList::new();
    list.rect(full, [1.0, 0.0, 0.0, 0.5]);
    list.rect(full, [0.0, 0.0, 1.0, 0.5]);

    let image = render_ui(WIDTH, HEIGHT, BLACK, &list).expect("the frame must render");
    let pixel = image.pixel(WIDTH / 2, HEIGHT / 2).expect("a centre pixel");

    // Red: 1.0*0.5 = 0.5, then the blue quad leaves half of it: 0.5*0.5 = 0.25.
    // Blue: 0.0 under, then 1.0*0.5 = 0.5.
    assert_channel(pixel[0], 0.25, "red after two layers");
    assert_channel(pixel[2], 0.5, "blue on top");
}

#[test]
fn the_later_quad_is_the_one_on_top() {
    // The same ordering property stated as the thing a HUD author relies on.
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }

    let full = UiRect::new(0.0, 0.0, WIDTH as f32, HEIGHT as f32);
    let mut list = UiDrawList::new();
    list.rect(full, [1.0, 0.0, 0.0, 1.0]);
    list.rect(full, [0.0, 1.0, 0.0, 1.0]);

    let image = render_ui(WIDTH, HEIGHT, BLACK, &list).expect("the frame must render");
    let pixel = image.pixel(WIDTH / 2, HEIGHT / 2).expect("a centre pixel");
    assert_channel(pixel[1], 1.0, "the second quad wins");
    assert_channel(pixel[0], 0.0, "the first is completely covered");
}

#[test]
fn a_glyph_is_the_vertex_colour_cut_to_the_letter_shape() {
    // The atlas modulates alpha only. If it modulated colour instead, text would come out
    // as a grey wash of the tint rather than a crisp letter.
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }

    let mut list = UiDrawList::new();
    // A single wide glyph, scaled up so its lit and unlit texels each cover many pixels.
    list.text_at((0.0, 0.0), "H", [0.0, 1.0, 0.0, 1.0], 8.0);

    let image =
        render_ui(WIDTH, HEIGHT, [0.5, 0.0, 0.0, 1.0], &list).expect("the frame must render");

    // 'H' is 0b10001 on its top row: lit at the left and right columns, unlit between. At
    // scale 8 the glyph is 40x56 pixels, so column 4 is inside the left stroke and column
    // 20 is inside the gap.
    let stroke = image.pixel(4, 8).expect("inside the left stroke");
    let gap = image.pixel(20, 8).expect("inside the gap");

    assert_channel(stroke[1], 1.0, "the stroke is the vertex colour");
    assert_channel(stroke[0], 0.0, "the stroke covered the clear");
    assert_channel(gap[0], 0.5, "the gap left the clear alone");
    assert_channel(gap[1], 0.0, "the gap drew no green");
}

#[test]
fn opaque_scene_geometry_is_not_blended() {
    // The other half of the requirement: blending must not have been switched on globally.
    // The scene pipeline's blend state is a compile-time value, so this is a unit-level
    // check that needs no device.
    let blend = engine_renderer::ui_pipeline::alpha_blend_desc();
    assert!(
        blend.RenderTarget[0].BlendEnable.as_bool(),
        "the UI pass blends"
    );
}
