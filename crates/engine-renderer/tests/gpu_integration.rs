//! Automated GPU integration test (Milestone 9).
//!
//! Follows the milestone's steps exactly, with no visible window anywhere:
//!
//! 1. create a D3D12 device,
//! 2. create an offscreen render target,
//! 3. render a known triangle,
//! 4. copy the result to a readback resource,
//! 5. read the pixels back to the CPU,
//! 6. verify that the expected pixels and colours are present.
//!
//! Steps 1, 2, 4 and 5 live in `engine_renderer::offscreen`; this file supplies the known
//! triangle and the verification.
//!
//! The expected pixel count is **derived from the triangle's own vertices**, not
//! hardcoded: the test computes the triangle's share of the clip volume and scales it by
//! the target size. Changing the triangle changes the expectation automatically, so the
//! test cannot silently rot into asserting a stale constant.
//!
//! Takes the same process-wide GPU lock as the other GPU tests; see `d3d12_init.rs`.

#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Mutex, MutexGuard, PoisonError};

use engine_renderer::device::RenderDevice;
use engine_renderer::offscreen::{CapturedImage, render_vertices};
use engine_renderer::{RendererError, Vertex};

static GPU: Mutex<()> = Mutex::new(());

fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Whether this machine can run the test at all.
///
/// A machine with no D3D12 adapter skips rather than fails: the suite must stay runnable
/// on hardware the engine does not target. Any other initialization error is a real
/// failure and panics.
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

/// Render target size for the test. Large enough that edge pixels are a small share of
/// the total, small enough to stay fast.
const SIZE: u32 = 512;

/// The clear colour: pure black, so every lit pixel came from the draw.
const CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// The triangle's colour, chosen so each channel is distinct and none is 0 or 255.
/// `R8G8B8A8_UNORM` stores what the shader wrote, so these map to exact byte values.
const TRIANGLE_COLOR: [f32; 4] = [0.25, 0.50, 0.75, 1.0];

/// The byte values `TRIANGLE_COLOR` becomes after conversion to 8-bit UNORM.
const EXPECTED_RGB: [u8; 3] = [64, 128, 191];

/// Tolerance per channel, absorbing rounding in the UNORM conversion.
const CHANNEL_TOLERANCE: i32 = 2;

/// A known triangle: uniform colour, so interpolation cannot blur the expected value.
fn known_triangle() -> [Vertex; 3] {
    [
        Vertex {
            position: [0.0, 0.6, 0.0],
            color: TRIANGLE_COLOR,
        },
        Vertex {
            position: [-0.6, -0.5, 0.0],
            color: TRIANGLE_COLOR,
        },
        Vertex {
            position: [0.6, -0.5, 0.0],
            color: TRIANGLE_COLOR,
        },
    ]
}

/// Pixels the triangle should cover, derived from its vertices.
///
/// The clip volume spans -1..1 on both axes, an area of 4. A triangle's share of that
/// area is the same share of the render target's pixels.
fn expected_pixel_count(triangle: &[Vertex; 3], width: u32, height: u32) -> f64 {
    let [a, b, c] = triangle;
    let area = ((b.position[0] - a.position[0]) * (c.position[1] - a.position[1])
        - (c.position[0] - a.position[0]) * (b.position[1] - a.position[1]))
        .abs()
        / 2.0;
    f64::from(area) / 4.0 * f64::from(width) * f64::from(height)
}

/// Whether a pixel matches the triangle's colour within tolerance.
fn matches_triangle_color(pixel: [u8; 4]) -> bool {
    (0..3).all(|channel| {
        (i32::from(pixel[channel]) - i32::from(EXPECTED_RGB[channel])).abs() <= CHANNEL_TOLERANCE
    }) && pixel[3] == 255
}

/// Whether a pixel is still the clear colour.
fn is_clear(pixel: [u8; 4]) -> bool {
    pixel[0] <= 2 && pixel[1] <= 2 && pixel[2] <= 2
}

/// Steps 1-6 of the milestone, end to end.
#[test]
fn offscreen_triangle_produces_the_expected_pixels() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }

    let triangle = known_triangle();

    // Steps 1-5: device, offscreen target, draw, copy to readback, map to CPU memory.
    let image: CapturedImage =
        render_vertices(SIZE, SIZE, CLEAR, &triangle).expect("offscreen render must succeed");

    assert_eq!((image.width, image.height), (SIZE, SIZE));
    assert_eq!(
        image.pixels.len(),
        (SIZE * SIZE * 4) as usize,
        "readback must produce a tightly packed RGBA image"
    );

    // Step 6: verification.
    let expected = expected_pixel_count(&triangle, SIZE, SIZE);
    let actual = image.count_pixels(matches_triangle_color) as f64;
    let error = (actual - expected).abs() / expected;

    eprintln!("=== GPU INTEGRATION TEST ===");
    eprintln!("Target:   {SIZE}x{SIZE} offscreen, no window");
    eprintln!(
        "Expected: ~{expected:.0} pixels of colour rgb({}, {}, {})",
        EXPECTED_RGB[0], EXPECTED_RGB[1], EXPECTED_RGB[2]
    );
    eprintln!(
        "Actual:   {actual:.0} matching pixels ({:.2}% error)",
        error * 100.0
    );
    eprintln!("Result:   {}", if error <= 0.05 { "PASS" } else { "FAIL" });

    // 5% absorbs rasterization edge rules: a pixel is covered when its centre is inside
    // the triangle, so the boundary gains or loses roughly a perimeter's worth of pixels.
    assert!(
        error <= 0.05,
        "expected ~{expected:.0} pixels of the triangle colour, found {actual:.0} \
         ({:.2}% error, tolerance 5%)",
        error * 100.0
    );
}

/// Every pixel must be either the triangle's colour or the clear colour.
///
/// This catches a whole class of failures the count alone would miss: wrong colour
/// channels, a swapped colour order, uninitialized memory in the readback, or row padding
/// leaking into the image.
#[test]
fn no_pixel_is_neither_triangle_nor_background() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }

    let image = render_vertices(SIZE, SIZE, CLEAR, &known_triangle())
        .expect("offscreen render must succeed");

    let unexpected = image.count_pixels(|pixel| !matches_triangle_color(pixel) && !is_clear(pixel));

    // Anti-aliasing is off and the colour is uniform, so there should be no blend pixels
    // at all. A small allowance would hide real corruption, so this is exact.
    assert_eq!(
        unexpected, 0,
        "found {unexpected} pixels that are neither the triangle colour nor the clear colour"
    );
}

/// The corners are outside the triangle, so they prove the draw did not fill the target.
#[test]
fn the_target_is_not_simply_filled() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }

    let image = render_vertices(SIZE, SIZE, CLEAR, &known_triangle())
        .expect("offscreen render must succeed");

    for (x, y) in [(0, 0), (SIZE - 1, 0), (0, SIZE - 1), (SIZE - 1, SIZE - 1)] {
        let pixel = image.pixel(x, y).expect("corner is inside the image");
        assert!(
            is_clear(pixel),
            "corner ({x}, {y}) should be the clear colour, found {pixel:?}"
        );
    }

    // The centroid is inside the triangle.
    let centre = image
        .pixel(SIZE / 2, SIZE / 2)
        .expect("centre is inside the image");
    assert!(
        matches_triangle_color(centre),
        "the centre should be the triangle colour, found {centre:?}"
    );
}

/// The same input must produce the same pixels every time, or no measurement built on
/// this path is reproducible.
#[test]
fn rendering_is_deterministic() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }

    let first = render_vertices(256, 256, CLEAR, &known_triangle()).expect("first render");
    let second = render_vertices(256, 256, CLEAR, &known_triangle()).expect("second render");

    assert_eq!(
        first.pixels, second.pixels,
        "two identical renders produced different images"
    );
}

/// The clear colour must arrive at the render target unchanged, so a wrong background
/// cannot be mistaken for correct geometry.
#[test]
fn the_clear_colour_is_written_exactly() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }

    // Distinct per-channel values, none of them 0 or 255.
    let image = render_vertices(64, 64, [0.25, 0.50, 0.75, 1.0], &known_triangle())
        .expect("offscreen render must succeed");
    let corner = image.pixel(0, 0).expect("corner is inside the image");

    for (channel, expected) in EXPECTED_RGB.iter().enumerate() {
        assert!(
            (i32::from(corner[channel]) - i32::from(*expected)).abs() <= CHANNEL_TOLERANCE,
            "clear colour channel {channel} was {} not ~{expected}",
            corner[channel]
        );
    }
    assert_eq!(corner[3], 255, "alpha must be opaque");
}

/// A degenerate target must fail cleanly rather than crash or return garbage.
#[test]
fn invalid_targets_are_rejected() {
    assert!(render_vertices(0, 64, CLEAR, &known_triangle()).is_err());
    assert!(render_vertices(64, 0, CLEAR, &known_triangle()).is_err());
}
