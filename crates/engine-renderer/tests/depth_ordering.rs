//! Depth testing, proven by pixels rather than by API calls.
//!
//! The only honest test of a depth buffer is that submission order stops deciding what is
//! visible. Two overlapping triangles are drawn in both orders; if depth is working, the
//! nearer one wins both times.

#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Mutex, MutexGuard, PoisonError};

use engine_renderer::offscreen::render_vertices;
use engine_renderer::pipeline::Vertex;

static GPU: Mutex<()> = Mutex::new(());

fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU.lock().unwrap_or_else(PoisonError::into_inner)
}

const SIZE: u32 = 64;
const CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// A full-viewport triangle at depth `z`, in the given colour.
///
/// Clip-space `z` is the depth written, so 0.0 is the near plane and 1.0 the far one. The
/// triangle is oversized so it covers the whole target and the centre pixel is unambiguous.
fn covering_triangle(z: f32, color: [f32; 4]) -> [Vertex; 3] {
    [
        Vertex {
            position: [0.0, 3.0, z],
            color,
        },
        Vertex {
            position: [-3.0, -3.0, z],
            color,
        },
        Vertex {
            position: [3.0, -3.0, z],
            color,
        },
    ]
}

/// The colour at the centre of the image, as bytes.
fn center(image: &engine_renderer::offscreen::CapturedImage) -> [u8; 4] {
    image
        .pixel(SIZE / 2, SIZE / 2)
        .expect("the centre pixel must exist")
}

#[test]
fn the_nearer_surface_wins_whatever_order_it_was_submitted_in() {
    let _guard = gpu_lock();

    let near = covering_triangle(0.25, [1.0, 0.0, 0.0, 1.0]);
    let far = covering_triangle(0.75, [0.0, 1.0, 0.0, 1.0]);

    let mut near_first = Vec::new();
    near_first.extend_from_slice(&near);
    near_first.extend_from_slice(&far);

    let mut far_first = Vec::new();
    far_first.extend_from_slice(&far);
    far_first.extend_from_slice(&near);

    let a = render_vertices(SIZE, SIZE, CLEAR, &near_first).expect("render must succeed");
    let b = render_vertices(SIZE, SIZE, CLEAR, &far_first).expect("render must succeed");

    // Without a depth buffer these two images differ: whichever triangle was recorded last
    // would be on top. That is exactly the bug this milestone removes.
    assert_eq!(
        center(&a),
        center(&b),
        "submission order still decides what is visible"
    );

    let pixel = center(&a);
    assert!(
        pixel[0] > 200 && pixel[1] < 60,
        "expected the near (red) surface, got {pixel:?}"
    );
}

#[test]
fn a_surface_behind_another_does_not_show_through() {
    let _guard = gpu_lock();

    let mut vertices = Vec::new();
    vertices.extend_from_slice(&covering_triangle(0.2, [1.0, 0.0, 0.0, 1.0]));
    vertices.extend_from_slice(&covering_triangle(0.9, [0.0, 1.0, 0.0, 1.0]));

    let image = render_vertices(SIZE, SIZE, CLEAR, &vertices).expect("render must succeed");

    // Not one green pixel anywhere: the far triangle covers the same area, so any green at
    // all means a fragment passed the depth test that should not have.
    let green = image.count_pixels(|pixel| pixel[1] > 128 && pixel[0] < 128);
    assert_eq!(
        green, 0,
        "{green} pixels of the occluded surface showed through"
    );
}

#[test]
fn coplanar_surfaces_keep_the_last_one_drawn() {
    let _guard = gpu_lock();

    // `LESS_EQUAL`, not `LESS`: a fragment at exactly the stored depth is **accepted**, so
    // the later of two coplanar surfaces wins.
    //
    // This used to be the opposite, and the change was forced rather than chosen. The depth
    // prepass the contact shadows need writes exactly the depth the camera pass then
    // computes, so under `LESS` *every fragment in the frame* fails its own test and the
    // scene renders as the clear colour. Accepting equality is the only comparison that
    // works with a prepass at all.
    //
    // What it costs: deliberately coplanar geometry now resolves by submission order rather
    // than by rejection. Nothing in this renderer draws the same surface twice, and the one
    // place two surfaces are nearly coplanar on purpose — the targeting decal on the floor —
    // already separates them by three centimetres because a tie flickers under *either*
    // comparison as the camera moves.
    let mut vertices = Vec::new();
    vertices.extend_from_slice(&covering_triangle(0.5, [1.0, 0.0, 0.0, 1.0]));
    vertices.extend_from_slice(&covering_triangle(0.5, [0.0, 1.0, 0.0, 1.0]));

    let image = render_vertices(SIZE, SIZE, CLEAR, &vertices).expect("render must succeed");
    let pixel = center(&image);
    assert!(
        pixel[1] > 200 && pixel[0] < 60,
        "expected the last-drawn surface to survive, got {pixel:?}"
    );
}

#[test]
fn nothing_beyond_the_far_plane_is_drawn() {
    let _guard = gpu_lock();

    // Clip-space z greater than w is outside the [0, 1] depth range and is clipped. A
    // renderer that mapped depth to OpenGL's [-1, 1] would draw this.
    let vertices = covering_triangle(1.5, [1.0, 0.0, 0.0, 1.0]);
    let image = render_vertices(SIZE, SIZE, CLEAR, &vertices).expect("render must succeed");

    assert_eq!(
        center(&image),
        [0, 0, 0, 255],
        "geometry beyond the far plane was drawn"
    );
}
