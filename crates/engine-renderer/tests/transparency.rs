//! The transparent rendering path, proven pixel by pixel.
//!
//! A surface class, a pipeline descriptor and a sorted list are all things a unit test can
//! check without a GPU, and `src/render_list.rs` and `src/pipeline.rs` do check them. What
//! they cannot answer is whether the frame that reaches the render target actually blends,
//! actually tests depth, and actually draws in the order the sort produced. These tests render
//! the debug scene — and smaller scenes built from its pieces — offscreen and read the result
//! back, so every assertion is about a colour that landed in a real render target.
//!
//! Each test states the arithmetic it expects rather than a captured value, because a
//! screenshot comparison tells you a frame changed and this tells you which rule broke.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Mutex, MutexGuard, PoisonError};

use engine_core::math::Vec3;
use engine_core::transform::Transform;
use engine_render_data::{Camera, Lighting, Projection};
use engine_renderer::RendererError;
use engine_renderer::device::RenderDevice;
use engine_renderer::material::{MaterialDesc, SurfaceClass};
use engine_renderer::mesh::MeshData;
use engine_renderer::offscreen::{CapturedImage, SceneObject, render_surface_scene};
use engine_renderer::transparency::{
    DEBUG_ALPHA_CUTOFF, DEBUG_OPACITY, debug_lighting, debug_scene, upright_quad,
};

/// D3D12 device creation is serialized: several tests each making a device and submitting a
/// frame at once is a way to find driver bugs rather than renderer bugs.
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

const WIDTH: u32 = 96;
const HEIGHT: u32 = 96;

/// A black clear, so a blended layer over the background is exactly `colour * alpha`.
const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// How far a read-back channel may sit from the expected value.
///
/// The target is 8-bit UNORM, so one quantization step is about 1/255. The margin is wider
/// than that because the lighting still runs — flat ambient, but a multiply nonetheless.
const TOLERANCE: i32 = 4;

fn assert_channel(actual: u8, expected: f32, what: &str) {
    let expected = (expected * 255.0).round() as i32;
    let difference = (actual as i32 - expected).abs();
    assert!(
        difference <= TOLERANCE,
        "{what}: read {actual}, expected about {expected}"
    );
}

/// A camera on `+Z` looking at the origin, so "farther" and "smaller Z" mean the same thing.
fn camera() -> Camera {
    let mut camera = Camera::new(Projection::perspective(WIDTH as f32 / HEIGHT as f32))
        .expect("a perspective projection at 1:1 is valid");
    camera
        .look_at(Vec3::new(0.0, 0.0, 6.0), Vec3::ZERO, Vec3::Y)
        .expect("the camera can look at the origin");
    camera
}

/// Flat lighting: ambient only, so a surface's colour is its base colour and the expected
/// result of a blend is a multiplication rather than a lighting calculation.
fn flat_lighting() -> Lighting {
    Lighting {
        ambient_intensity: 1.0,
        intensity: 0.0,
        ..Lighting::default()
    }
}

/// A quad of `size` at `z`, in `material`.
fn quad_at(size: f32, z: f32, material: MaterialDesc) -> (MeshData, MaterialDesc, Transform) {
    (
        upright_quad(size),
        material,
        Transform::from_translation(Vec3::new(0.0, 0.0, z)),
    )
}

/// Renders a scene of `(mesh, material, transform)` triples through the classified path.
fn render(scene: &[(MeshData, MaterialDesc, Transform)]) -> CapturedImage {
    let objects: Vec<SceneObject<'_>> = scene
        .iter()
        .map(|(mesh, material, transform)| SceneObject {
            mesh,
            material: *material,
            transform: *transform,
        })
        .collect();
    render_surface_scene(WIDTH, HEIGHT, BLACK, &objects, &camera(), flat_lighting())
        .expect("the frame must render")
}

/// The pixel at the middle of the target, where every quad in these scenes covers.
fn centre(image: &CapturedImage) -> [u8; 4] {
    image.pixel(WIDTH / 2, HEIGHT / 2).expect("a centre pixel")
}

#[test]
fn an_opaque_surface_is_unchanged_by_the_transparent_path_existing() {
    // Requirement 6, at the pixel. An opaque quad must land on exactly its own colour: no
    // blending with the clear, no partial coverage, nothing the new phase could have leaked
    // into it. If the classification ever mis-routed an opaque material, this is the first
    // thing that would move.
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }

    let image = render(&[quad_at(4.0, 0.0, MaterialDesc::colored(0.8, 0.2, 0.2))]);
    let pixel = centre(&image);

    assert_channel(pixel[0], 0.8, "red");
    assert_channel(pixel[1], 0.2, "green");
    assert_channel(pixel[2], 0.2, "blue");
}

#[test]
fn an_opaque_surface_hides_what_is_behind_it_whatever_the_draw_order() {
    // The other half of "opaque is unchanged": depth writes still happen, so a nearer opaque
    // surface occludes a farther one even when the farther one is drawn second. Losing the
    // depth write on the opaque pipeline would show up here and nowhere else in this file.
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }

    let image = render(&[
        // Near first, far second: only the depth test can produce the right answer.
        quad_at(4.0, 2.0, MaterialDesc::colored(0.0, 0.9, 0.0)),
        quad_at(4.0, 0.0, MaterialDesc::colored(0.9, 0.0, 0.0)),
    ]);
    let pixel = centre(&image);

    assert_channel(pixel[0], 0.0, "red: the far quad must be hidden");
    assert_channel(pixel[1], 0.9, "green: the near quad survives");
}

#[test]
fn an_alpha_tested_surface_above_its_cutoff_draws_as_opaque() {
    // A cutout that passes its test is an opaque surface in every way that matters: full
    // intensity, not blended with the background. An alpha of 0.9 blended rather than tested
    // would read 0.72 here, which is a difference this tolerance cannot swallow.
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }

    let mut material = MaterialDesc::colored(0.2, 0.8, 0.2).with_alpha_cutoff(DEBUG_ALPHA_CUTOFF);
    material.base_color.w = 0.9;
    assert_eq!(material.surface, SurfaceClass::AlphaTest);

    let image = render(&[quad_at(4.0, 0.0, material)]);
    let pixel = centre(&image);

    assert_channel(pixel[0], 0.2, "red");
    assert_channel(pixel[1], 0.8, "green: full intensity, not 0.8 * 0.9");
    assert_channel(pixel[2], 0.2, "blue");
}

#[test]
fn an_alpha_tested_surface_below_its_cutoff_is_discarded_entirely() {
    // The test really fires. This is the assertion that would catch a cutout material that
    // was accidentally routed into the transparent phase, where a `discard` never happens and
    // the quad would appear as a faint wash instead of not at all.
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }

    let mut material = MaterialDesc::colored(1.0, 0.0, 0.0).with_alpha_cutoff(DEBUG_ALPHA_CUTOFF);
    material.base_color.w = 0.25;

    let image = render(&[quad_at(4.0, 0.0, material)]);
    let pixel = centre(&image);

    assert_channel(pixel[0], 0.0, "red: every fragment must have discarded");
    assert_eq!(
        image.count_pixels(|pixel| pixel[0] > 8),
        0,
        "a sub-cutoff cutout must leave the target untouched everywhere"
    );
}

#[test]
fn a_transparent_surface_blends_with_what_is_behind_it() {
    // The defining property: `src * a + dst * (1 - a)`. Over a black clear at half opacity
    // that is exactly half the surface colour. An unblended pipeline would read the full
    // colour; a premultiplied blend factor of ONE would read the full colour too, which is
    // why the background here is black and the next test uses a lit one.
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }

    let image = render(&[quad_at(
        4.0,
        0.0,
        MaterialDesc::colored(1.0, 0.0, 0.0).with_opacity(DEBUG_OPACITY),
    )]);
    let pixel = centre(&image);

    assert_channel(pixel[0], 0.5, "red: 1.0 * 0.5 over black");
    assert_channel(pixel[1], 0.0, "green");
    assert_channel(pixel[2], 0.0, "blue");
}

#[test]
fn a_transparent_surface_over_opaque_geometry_shows_both() {
    // Half a red surface over a green one: `1.0 * 0.5` red and `0.8 * 0.5` green. This is the
    // one that distinguishes straight source-alpha blending from the particle pass's
    // premultiplied blend - with `ONE` as the source factor the red would read 1.0.
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }

    let image = render(&[
        quad_at(4.0, 0.0, MaterialDesc::colored(0.0, 0.8, 0.0)),
        quad_at(
            2.0,
            1.0,
            MaterialDesc::colored(1.0, 0.0, 0.0).with_opacity(0.5),
        ),
    ]);
    let pixel = centre(&image);

    assert_channel(
        pixel[0],
        0.5,
        "red: the transparent surface at half opacity",
    );
    assert_channel(
        pixel[1],
        0.4,
        "green: the opaque surface, halved by the blend",
    );
}

#[test]
fn a_transparent_surface_is_hidden_by_nearer_opaque_geometry() {
    // Depth testing, which is the half of the depth state the transparent phase keeps. A
    // pipeline with `DepthEnable` off would draw the effect straight through the wall, which
    // is the single most visible way this feature can be wrong.
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }

    let image = render(&[
        // The opaque wall is nearer the camera than the transparent quad behind it.
        quad_at(4.0, 2.0, MaterialDesc::colored(0.0, 0.0, 0.9)),
        quad_at(
            2.0,
            0.0,
            MaterialDesc::colored(1.0, 0.0, 0.0).with_opacity(0.5),
        ),
    ]);
    let pixel = centre(&image);

    assert_channel(pixel[0], 0.0, "red: the effect is behind the wall");
    assert_channel(pixel[2], 0.9, "blue: the wall, undimmed");
}

#[test]
fn a_transparent_surface_does_not_occlude_the_transparent_surfaces_behind_it() {
    // The depth *write* being off. Two overlapping layers must both contribute; if the nearer
    // one wrote depth, the farther one would be rejected outright and the result would be a
    // single layer - which reads as an effect that lost half its geometry rather than as a
    // depth setting.
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }

    let image = render(&[
        quad_at(
            2.0,
            0.0,
            MaterialDesc::colored(0.0, 0.0, 1.0).with_opacity(0.5),
        ),
        quad_at(
            2.0,
            1.0,
            MaterialDesc::colored(0.0, 1.0, 0.0).with_opacity(0.5),
        ),
    ]);
    let pixel = centre(&image);

    // Far blue first at half over black gives 0.5 blue; near green at half over that gives
    // 0.5 green and 0.25 blue. Both channels present is the assertion; their exact values are
    // what the ordering test below turns into a statement about which came first.
    assert_channel(pixel[1], 0.5, "green: the near layer");
    assert_channel(
        pixel[2],
        0.25,
        "blue: the far layer, still visible through it",
    );
}

#[test]
fn overlapping_transparent_surfaces_are_composited_farthest_first() {
    // The ordering rule, and the reason the phase sorts at all. The same two quads are handed
    // over in both possible authoring orders and must produce the same picture, because the
    // sort - not the list order - decides which is drawn first.
    //
    // The expected values also pin down *which* order is correct: far-then-near gives
    // `0.25` blue and `0.5` green, and near-then-far would give `0.5` blue and `0.25` green.
    // A phase that drew in list order would pass a "both layers present" test and fail this.
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }

    let far = || {
        quad_at(
            2.0,
            0.0,
            MaterialDesc::colored(0.0, 0.0, 1.0).with_opacity(0.5),
        )
    };
    let near = || {
        quad_at(
            2.0,
            1.0,
            MaterialDesc::colored(0.0, 1.0, 0.0).with_opacity(0.5),
        )
    };

    let authored_far_first = centre(&render(&[far(), near()]));
    let authored_near_first = centre(&render(&[near(), far()]));

    for pixel in [authored_far_first, authored_near_first] {
        assert_channel(pixel[1], 0.5, "green: the near layer is composited last");
        assert_channel(pixel[2], 0.25, "blue: the far layer went down first");
    }
    assert_eq!(
        authored_far_first, authored_near_first,
        "the authoring order must not change the picture; the sort decides"
    );
}

#[test]
fn the_transparent_order_is_the_same_every_run() {
    // Determinism, at the pixel rather than at the permutation. Ten renders of one scene must
    // be ten identical images: a sort whose comparator was not a total order could reorder
    // equal-distance entries between runs and produce a frame that flickers.
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }

    let scene = || {
        vec![
            quad_at(
                2.0,
                0.0,
                MaterialDesc::colored(0.0, 0.0, 1.0).with_opacity(0.5),
            ),
            quad_at(
                2.0,
                1.0,
                MaterialDesc::colored(0.0, 1.0, 0.0).with_opacity(0.5),
            ),
            quad_at(
                2.0,
                0.5,
                MaterialDesc::colored(1.0, 0.0, 0.0).with_opacity(0.5),
            ),
        ]
    };

    let first = render(&scene());
    for run in 1..6 {
        let again = render(&scene());
        assert_eq!(
            first.pixels, again.pixels,
            "run {run} produced a different frame from run 0"
        );
    }
}

#[test]
fn the_debug_scene_renders_every_class_at_once() {
    // The scene from `engine_renderer::transparency`, drawn through the same classified path
    // the renderer uses. It is the integration case: an opaque cube, cutouts either side of
    // one threshold, and overlapping blended quads all in one frame, none of them interfering
    // with any other.
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }

    let scene = debug_scene();
    let objects: Vec<SceneObject<'_>> = scene
        .iter()
        .map(|object| SceneObject {
            mesh: &object.mesh,
            material: object.material,
            transform: object.transform,
        })
        .collect();

    let image = render_surface_scene(
        256,
        256,
        BLACK,
        &objects,
        &engine_renderer::transparency::debug_camera(256, 256),
        debug_lighting(),
    )
    .expect("the debug scene must render");

    // Something was drawn, and it was not the whole screen: a frame that came back entirely
    // black would mean nothing rendered, and one with no black at all would mean a quad
    // covered everything and the individual assertions below are measuring the wrong thing.
    let lit = image.count_pixels(|pixel| pixel[0] > 8 || pixel[1] > 8 || pixel[2] > 8);
    assert!(
        lit > 1_000,
        "the debug scene drew almost nothing: {lit} lit pixels"
    );
    assert!(
        lit < 256 * 256,
        "the debug scene covered the whole target, so nothing can be told apart"
    );

    // The discarded cutout is the only red thing on the cube's side of the scene apart from
    // the transparent quad, and the transparent quad is half-strength. A fully saturated red
    // pixel anywhere means the sub-cutoff quad drew.
    assert_eq!(
        image.count_pixels(|pixel| pixel[0] > 200 && pixel[1] < 40 && pixel[2] < 40),
        0,
        "the sub-cutoff cutout must not have drawn at full intensity"
    );

    // The green channel of the cutout cross is above the cutoff and draws unblended, so the
    // scene must contain strongly green pixels.
    assert!(
        image.count_pixels(|pixel| pixel[1] > 120 && pixel[0] < 100) > 100,
        "the alpha-tested foliage cross must be visible"
    );
}
