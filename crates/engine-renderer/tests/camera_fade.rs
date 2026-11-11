//! The dithered fade, proven pixel by pixel.
//!
//! ```text
//!   opacity 1      opacity 0.5    opacity 0
//!   ##########     # # # # #
//!   ##########      # # # # #     (nothing)
//!   ##########     # # # # #
//! ```
//!
//! A faded entity keeps a screen-door share of its pixels and discards the rest, in the
//! opaque pass it was always drawn in. These tests render through the same pipeline and
//! constant buffers the window uses and count what reached the render target: how much of a
//! surface survives a given opacity, that what is left still occludes like an opaque surface,
//! and that the camera fade touches see-through surfaces — and only those — in front of the
//! eye and between the eye and its target.

#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Mutex, MutexGuard, PoisonError};

use engine_core::math::Vec3;
use engine_core::transform::Transform;
use engine_render_data::{Camera, CameraFade, InstanceStyle, Lighting, Projection};
use engine_renderer::RendererError;
use engine_renderer::device::RenderDevice;
use engine_renderer::material::MaterialDesc;
use engine_renderer::mesh::MeshData;
use engine_renderer::offscreen::{CapturedImage, SceneObject, render_scene_styled};
use engine_renderer::transparency::upright_quad;

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

const SIZE: u32 = 64;
const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// A camera on `+Z` at `z`, looking down `-Z`.
fn camera_at(z: f32) -> Camera {
    let mut camera = Camera::new(Projection::perspective(1.0)).expect("a valid projection");
    camera
        .look_at(
            Vec3::new(0.0, 0.0, z),
            Vec3::new(0.0, 0.0, z - 1.0),
            Vec3::Y,
        )
        .expect("the camera can look down -Z");
    camera
}

fn flat_lighting() -> Lighting {
    Lighting {
        ambient_intensity: 1.0,
        intensity: 0.0,
        ..Lighting::default()
    }
}

/// Red and green, so which of two stacked quads a pixel belongs to is one channel.
fn red() -> MaterialDesc {
    MaterialDesc::colored(1.0, 0.0, 0.0)
}

fn green() -> MaterialDesc {
    MaterialDesc::colored(0.0, 1.0, 0.0)
}

/// Renders `(mesh, material, z, style)` quads, all centred on the view axis.
fn render(
    camera: &Camera,
    scene: &[(MeshData, MaterialDesc, f32, InstanceStyle)],
    fade: Option<CameraFade>,
) -> CapturedImage {
    let objects: Vec<SceneObject<'_>> = scene
        .iter()
        .map(|(mesh, material, z, _)| SceneObject {
            mesh,
            material: *material,
            transform: Transform::from_translation(Vec3::new(0.0, 0.0, *z)),
        })
        .collect();
    let styles: Vec<InstanceStyle> = scene.iter().map(|(_, _, _, style)| *style).collect();
    render_scene_styled(
        SIZE,
        SIZE,
        BLACK,
        &objects,
        &styles,
        fade,
        camera,
        flat_lighting(),
    )
    .expect("the frame must render")
}

fn is_red(pixel: [u8; 4]) -> bool {
    pixel[0] > 128 && pixel[1] < 64
}

fn is_green(pixel: [u8; 4]) -> bool {
    pixel[1] > 128 && pixel[0] < 64
}

fn total() -> usize {
    (SIZE * SIZE) as usize
}

/// A fade aimed down the view axis at `target_z`.
fn fade_towards(target_z: f32) -> CameraFade {
    CameraFade {
        target: Vec3::new(0.0, 0.0, target_z),
        near_start: 0.2,
        near_end: 1.0,
        radius: 0.9,
        min_opacity: 0.3,
    }
}

#[test]
fn opacity_keeps_that_share_of_the_pixels() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    // A quad far larger than the view, so every pixel is the quad's.
    let camera = camera_at(5.0);
    for (opacity, low, high) in [
        (1.0, 1.0, 1.0),
        (0.5, 0.45, 0.55),
        (0.25, 0.2, 0.3),
        (0.0, 0.0, 0.0),
    ] {
        let image = render(
            &camera,
            &[(
                upright_quad(40.0),
                red(),
                0.0,
                InstanceStyle::NEUTRAL.with_opacity(opacity),
            )],
            None,
        );
        let share = image.count_pixels(is_red) as f32 / total() as f32;
        assert!(
            (low..=high).contains(&share),
            "opacity {opacity} drew {share} of the pixels"
        );
    }
}

#[test]
fn what_a_fade_leaves_still_hides_what_is_behind_it() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    // A half-faded red quad in front of a whole green one: every pixel is one or the other,
    // half each, and none is black. The red pixels that are drawn still write depth.
    let camera = camera_at(5.0);
    let image = render(
        &camera,
        &[
            (upright_quad(40.0), green(), -1.0, InstanceStyle::NEUTRAL),
            (
                upright_quad(40.0),
                red(),
                0.0,
                InstanceStyle::NEUTRAL.with_opacity(0.5),
            ),
        ],
        None,
    );
    let reds = image.count_pixels(is_red);
    let greens = image.count_pixels(is_green);
    assert_eq!(reds + greens, total(), "a pixel showed neither quad");
    let share = reds as f32 / total() as f32;
    assert!(
        (0.45..=0.55).contains(&share),
        "{share} of the pixels were the front quad"
    );
}

#[test]
fn a_see_through_surface_between_the_eye_and_the_target_fades() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    // Eye at z = 5, target at z = -3, a red wall of decoration at z = 1 across the line.
    let camera = camera_at(5.0);
    let fade = Some(fade_towards(-3.0));
    let centre = |image: &CapturedImage| {
        let mut drawn = 0;
        for y in SIZE / 2 - 4..SIZE / 2 + 4 {
            for x in SIZE / 2 - 4..SIZE / 2 + 4 {
                if is_red(image.pixel(x, y).expect("inside the image")) {
                    drawn += 1;
                }
            }
        }
        drawn as f32 / 64.0
    };

    let solid = render(
        &camera,
        &[(upright_quad(40.0), red(), 1.0, InstanceStyle::NEUTRAL)],
        fade,
    );
    assert_eq!(centre(&solid), 1.0, "a solid surface is never faded");

    let decoration = render(
        &camera,
        &[(
            upright_quad(40.0),
            red(),
            1.0,
            InstanceStyle::NEUTRAL.see_through(),
        )],
        fade,
    );
    let share = centre(&decoration);
    assert!(
        (0.2..=0.4).contains(&share),
        "the occluder kept {share} at the centre"
    );
    // Outside the tube it is whole again: the corners are far off the line.
    assert!(is_red(decoration.pixel(1, 1).expect("a corner")));
    assert!(is_red(
        decoration.pixel(SIZE - 2, SIZE - 2).expect("a corner")
    ));

    // With no fade this frame, a see-through surface is just a surface.
    let off = render(
        &camera,
        &[(
            upright_quad(40.0),
            red(),
            1.0,
            InstanceStyle::NEUTRAL.see_through(),
        )],
        None,
    );
    assert_eq!(off.count_pixels(is_red), total());
}

#[test]
fn a_see_through_surface_behind_the_target_is_left_alone() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    // The same decoration, but past the character: it is not in the way of anything.
    let camera = camera_at(5.0);
    let image = render(
        &camera,
        &[(
            upright_quad(40.0),
            red(),
            -6.0,
            InstanceStyle::NEUTRAL.see_through(),
        )],
        Some(fade_towards(-3.0)),
    );
    assert_eq!(image.count_pixels(is_red), total());
}

#[test]
fn a_see_through_surface_at_the_eye_is_gone_before_the_near_plane_cuts_it() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    // A leaf 0.15 m in front of the eye, and the target well off to one side so only the
    // near zone applies: nothing of it is drawn.
    let camera = camera_at(5.0);
    let fade = Some(CameraFade {
        target: Vec3::new(30.0, 0.0, -30.0),
        ..fade_towards(0.0)
    });
    let leaf = render(
        &camera,
        &[(
            upright_quad(40.0),
            red(),
            4.85,
            InstanceStyle::NEUTRAL.see_through(),
        )],
        fade,
    );
    assert_eq!(
        leaf.count_pixels(is_red),
        0,
        "the leaf was drawn at the eye"
    );
    // Half a metre away it is partly there; two metres away, whole.
    let half = render(
        &camera,
        &[(
            upright_quad(40.0),
            red(),
            4.4,
            InstanceStyle::NEUTRAL.see_through(),
        )],
        fade,
    );
    let share = half.count_pixels(is_red) as f32 / total() as f32;
    assert!(share > 0.05 && share < 0.95, "{share}");
    let far = render(
        &camera,
        &[(
            upright_quad(40.0),
            red(),
            3.0,
            InstanceStyle::NEUTRAL.see_through(),
        )],
        fade,
    );
    assert_eq!(far.count_pixels(is_red), total());
}
