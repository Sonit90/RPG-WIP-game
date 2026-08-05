//! Automated proof that the triangle is actually rasterized.
//!
//! Renders offscreen and reads the pixels back, so it needs no window and no desktop.
//! This exists because "the draw call was recorded and the validation layer was silent"
//! is not evidence: a back-face-culled triangle produces exactly those signals and an
//! empty image, which is what happened during Milestone 4.
//!
//! Takes the same process-wide GPU lock as `d3d12_init.rs` — see that file for why.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Mutex, MutexGuard, PoisonError};

use engine_renderer::RendererError;
use engine_renderer::device::GpuContext;
use engine_renderer::offscreen::{CapturedImage, render_triangle};

/// Serializes GPU work across test threads and, through it, across test binaries in the
/// same process. Separate binaries run as separate processes, which is fine: the crash
/// this guards against needs concurrent devices inside one process.
static GPU: Mutex<()> = Mutex::new(());

fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Whether this machine has a usable D3D12 adapter.
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

/// The clear colour used by these tests: pure black, so any lit pixel came from the draw.
const CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// True if a pixel is not the cleared background.
fn is_lit(pixel: [u8; 4]) -> bool {
    pixel[0] > 8 || pixel[1] > 8 || pixel[2] > 8
}

#[test]
fn triangle_covers_the_expected_fraction_of_the_image() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }

    let width = 256;
    let height = 256;
    let image: CapturedImage =
        render_triangle(width, height, CLEAR).expect("offscreen render must succeed");

    assert_eq!(image.width, width);
    assert_eq!(image.height, height);
    assert_eq!(image.pixels.len(), (width * height * 4) as usize);

    let lit = image.count_pixels(is_lit);
    let total = (width * height) as usize;
    let fraction = lit as f64 / total as f64;

    // The triangle spans x in [-0.6, 0.6] and y in [-0.5, 0.6] of a clip volume of area 4,
    // so it covers 0.5 * 1.2 * 1.1 / 4 = 16.5% of the image. Allow for rasterization
    // rules and rounding, but stay tight enough to fail if the triangle is culled (0%) or
    // if the whole target is filled (100%).
    assert!(
        (0.13..=0.20).contains(&fraction),
        "triangle should cover ~16.5% of the image, got {:.1}% ({lit} of {total} pixels)",
        fraction * 100.0
    );
}

#[test]
fn triangle_shows_the_interpolated_vertex_colours() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }

    let size = 256;
    let image = render_triangle(size, size, CLEAR).expect("offscreen render must succeed");

    // Clip space (x right, y up) maps to pixels with y flipped.
    let to_pixel = |x: f32, y: f32| {
        (
            ((x + 1.0) * 0.5 * size as f32) as u32,
            ((1.0 - y) * 0.5 * size as f32) as u32,
        )
    };

    // Near the top vertex, which is red.
    let (x, y) = to_pixel(0.0, 0.45);
    let apex = image.pixel(x, y).expect("apex sample must be inside");
    assert!(
        apex[0] > apex[1] && apex[0] > apex[2],
        "top vertex is red, sampled {apex:?} at ({x}, {y})"
    );

    // Near the bottom-left vertex, which is green.
    let (x, y) = to_pixel(-0.45, -0.42);
    let left = image.pixel(x, y).expect("left sample must be inside");
    assert!(
        left[1] > left[0] && left[1] > left[2],
        "bottom-left vertex is green, sampled {left:?} at ({x}, {y})"
    );

    // Near the bottom-right vertex, which is blue.
    let (x, y) = to_pixel(0.45, -0.42);
    let right = image.pixel(x, y).expect("right sample must be inside");
    assert!(
        right[2] > right[0] && right[2] > right[1],
        "bottom-right vertex is blue, sampled {right:?} at ({x}, {y})"
    );

    // The centre is a blend of all three, so no channel dominates completely.
    let centre = image.pixel(size / 2, size / 2).expect("centre is inside");
    assert!(
        is_lit(centre) && centre.iter().take(3).all(|&c| c > 20),
        "the centre should be an interpolated blend, sampled {centre:?}"
    );
}

#[test]
fn corners_keep_the_clear_colour() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }

    let size = 128;
    let image = render_triangle(size, size, CLEAR).expect("offscreen render must succeed");

    for (x, y) in [(0, 0), (size - 1, 0), (0, size - 1), (size - 1, size - 1)] {
        let pixel = image.pixel(x, y).expect("corner is inside the image");
        assert!(
            !is_lit(pixel),
            "corner ({x}, {y}) must stay the clear colour, got {pixel:?}"
        );
    }
}

#[test]
fn the_clear_colour_reaches_the_render_target() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }

    // A distinctive clear colour, checked in a corner the triangle cannot reach.
    let image = render_triangle(64, 64, [0.25, 0.5, 0.75, 1.0]).expect("offscreen render");
    let corner = image.pixel(0, 0).expect("corner is inside the image");

    // R8G8B8A8_UNORM stores the value as written, so 0.25 -> 64, 0.5 -> 128, 0.75 -> 191.
    assert!((corner[0] as i32 - 64).abs() <= 2, "red {}", corner[0]);
    assert!((corner[1] as i32 - 128).abs() <= 2, "green {}", corner[1]);
    assert!((corner[2] as i32 - 191).abs() <= 2, "blue {}", corner[2]);
}
