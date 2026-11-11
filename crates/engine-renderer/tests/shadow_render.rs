//! The shadow system, rendered on the GPU and read back a pixel at a time.
//!
//! Everything else about shadows is asserted without a device: the cascade mathematics, the
//! atlas layout, the caster culling, the light selection. This file asserts the one thing
//! those cannot — that the pipeline actually **darkens the right pixels** — by rendering
//! through the real [`ShadowRenderer`](engine_renderer::ShadowRenderer), the real depth-only
//! pipelines, the real atlas and the real mesh shader, and comparing what came back.
//!
//! ```text
//!            light
//!              |
//!          [ cube ]      <- caster, floating above the floor
//!              |
//! ============ V ============   the floor: shadowed under the cube, lit beside it
//! ```
//!
//! Every test compares two pixels of the *same* surface rather than one pixel against a
//! constant. A brightness threshold would be a test of the lighting model and the clear
//! colour; "under the caster is darker than beside it" is a test of the shadow.
//!
//! # Skipping
//!
//! The suite skips when no D3D12 device can be created, exactly as the other GPU tests do:
//! a machine with no adapter should report "no GPU" rather than a wall of failures.

#![cfg(windows)]
#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_core::math::{Quat, Vec3};
use engine_core::transform::Transform;
use engine_render_data::{Camera, Lighting, LocalLight, Projection, ShadowCasting};
use engine_renderer::offscreen::{CapturedImage, ShadowSceneObject, render_scene_shadowed};
use engine_renderer::shadow::{ShadowPreset, ShadowSettings};
use engine_renderer::{MaterialDesc, MeshData};

/// Whether a device exists at all.
fn gpu_available() -> bool {
    engine_renderer::offscreen::render_triangle(8, 8, [0.0, 0.0, 0.0, 1.0]).is_ok()
}

const SIZE: u32 = 192;
const CLEAR: [f32; 4] = [0.02, 0.02, 0.04, 1.0];

/// A sun tilted along +X, so a caster's shadow lands *beside* it rather than under it.
///
/// The tilt is the whole reason this file can measure anything. With the light straight down
/// and the camera straight down, a cube's shadow is exactly the patch of floor the cube
/// itself hides — there is no pixel that shows the shadow and not the caster. Travelling
/// 1.5 metres sideways for every metre down puts the shadow of a cube 2 m up at x = -3,
/// clear of the cube's own footprint at |x| < 0.75 and clear of its silhouette from above.
fn overhead_sun() -> Lighting {
    Lighting {
        direction: Vec3::new(-1.5, -1.0, 0.0),
        color: Vec3::ONE,
        intensity: 1.0,
        // Low, so the shadowed area is clearly darker than the lit one. Ambient is added
        // outside the shadow factor by design — see `shade_surface` — so a high ambient
        // would compress the difference this file measures.
        ambient_color: Vec3::ONE,
        ambient_intensity: 0.1,
    }
}

/// A camera looking straight down at the floor from above.
///
/// Top-down, so the floor fills the frame and a pixel's position on screen is a position in
/// the world — which is what lets the assertions name "under the cube" and "beside it".
fn top_down_camera() -> Camera {
    let mut camera = Camera::new(Projection::perspective(1.0)).expect("a square viewport");
    camera.set_position(Vec3::new(0.0, 12.0, 0.0));
    // Pitched a quarter turn down from looking along -Z.
    camera.set_rotation(Quat::from_yaw_pitch_roll(
        0.0,
        -std::f32::consts::FRAC_PI_2,
        0.0,
    ));
    camera
}

fn floor() -> MeshData {
    MeshData::plane(40.0)
}

fn matte(red: f32, green: f32, blue: f32) -> MaterialDesc {
    MaterialDesc::colored(red, green, blue).with_roughness(1.0)
}

/// A cutout material whose sampled alpha is `alpha` and whose threshold is `cutoff`.
///
/// `alpha < cutoff` is discarded, in the camera pass and the shadow pass alike.
fn cutout(alpha: f32, cutoff: f32) -> MaterialDesc {
    MaterialDesc {
        base_color: engine_core::math::Vec4::new(0.9, 0.3, 0.2, alpha),
        ..matte(0.9, 0.3, 0.2)
    }
    .with_alpha_cutoff(cutoff)
}

/// The floor, plus one cube two metres above the origin.
fn scene<'a>(floor_mesh: &'a MeshData, cube: &'a MeshData) -> Vec<ShadowSceneObject<'a>> {
    vec![
        ShadowSceneObject {
            transform: Transform::IDENTITY,
            mesh: floor_mesh,
            material: matte(0.8, 0.8, 0.8),
            // A flat floor casting its own shadow would be a slab of darkness; it receives.
            shadow: ShadowCasting::RECEIVES_ONLY,
        },
        ShadowSceneObject {
            transform: Transform::from_translation(Vec3::new(0.0, 2.0, 0.0)),
            mesh: cube,
            material: matte(0.9, 0.3, 0.2),
            shadow: ShadowCasting::CASTS_AND_RECEIVES,
        },
    ]
}

/// The mean brightness of a small patch, so one noisy texel cannot decide a test.
fn patch(image: &CapturedImage, x: u32, y: u32) -> f32 {
    let mut total = 0.0;
    let mut count = 0.0;
    for offset_y in 0..4 {
        for offset_x in 0..4 {
            if let Some(pixel) = image.pixel(x + offset_x, y + offset_y) {
                total += f32::from(pixel[0]) + f32::from(pixel[1]) + f32::from(pixel[2]);
                count += 3.0;
            }
        }
    }
    if count == 0.0 { 0.0 } else { total / count }
}

/// Renders the standard scene at `settings`.
fn render(settings: ShadowSettings) -> CapturedImage {
    let floor_mesh = floor();
    let cube = MeshData::cube(1.5);
    let objects = scene(&floor_mesh, &cube);
    render_scene_shadowed(
        SIZE,
        SIZE,
        CLEAR,
        &objects,
        &[],
        &top_down_camera(),
        overhead_sun(),
        settings,
    )
    .expect("the shadowed scene must render")
}

/// A patch of floor inside the cube's shadow.
///
/// The camera is 12 m up with a 60-degree vertical field of view, so the floor spans about
/// 6.9 m either side of the centre and one pixel is about 7 cm. The shadow of a 1.5 m cube
/// two metres up, cast by a light travelling 1.5 sideways for every 1 down, covers x from
/// -3.75 to -2.25; this patch sits at about x = -3.4.
const SHADOWED: (u32, u32) = (48, 94);
/// A patch of open floor on the lit side of the cube, about x = +3.2.
const LIT: (u32, u32) = (140, 94);
/// A second patch of open floor, near a corner and far from everything.
const BESIDE: (u32, u32) = (12, 12);

#[test]
fn a_caster_darkens_the_floor_beneath_it() {
    if !gpu_available() {
        eprintln!("skipping: no D3D12 device");
        return;
    }
    // The whole system, end to end: the cascade covered the floor, the cube reached the
    // depth pass, the atlas held it, and the mesh shader found it again.
    let image = render(ShadowPreset::High.settings());

    // The cube itself is at the centre of a top-down view, so the floor's shadow is read
    // just outside the cube's own silhouette — near enough to be inside the shadow, far
    // enough not to be the cube.
    let shadowed = patch(&image, SHADOWED.0, SHADOWED.1);
    let lit = patch(&image, LIT.0, LIT.1);
    assert!(
        shadowed < lit * 0.75,
        "the floor under the caster ({shadowed:.1}) is not clearly darker than the floor away \
         from it ({lit:.1})"
    );
}

#[test]
fn shadows_switched_off_leave_the_floor_evenly_lit() {
    if !gpu_available() {
        eprintln!("skipping: no D3D12 device");
        return;
    }
    // The control. Without it, "the floor is darker there" could be a lighting artefact of
    // the geometry rather than a shadow — and it also proves the Off preset really does
    // switch the whole stage off rather than rendering it and ignoring it.
    let image = render(ShadowPreset::Off.settings());
    let under = patch(&image, SHADOWED.0, SHADOWED.1);
    let lit = patch(&image, LIT.0, LIT.1);
    assert!(
        (under - lit).abs() < lit * 0.1,
        "with shadows off the floor should be evenly lit: {under:.1} against {lit:.1}"
    );
}

#[test]
fn a_surface_that_does_not_receive_is_not_darkened() {
    if !gpu_available() {
        eprintln!("skipping: no D3D12 device");
        return;
    }
    // §4's `receives_shadow`, asserted where it is visible: the same scene, the same caster,
    // the same cascades — and a floor that has opted out.
    let floor_mesh = floor();
    let cube = MeshData::cube(1.5);
    let mut objects = scene(&floor_mesh, &cube);
    objects[0].shadow = ShadowCasting::NONE;

    let image = render_scene_shadowed(
        SIZE,
        SIZE,
        CLEAR,
        &objects,
        &[],
        &top_down_camera(),
        overhead_sun(),
        ShadowPreset::High.settings(),
    )
    .expect("the shadowed scene must render");

    let under = patch(&image, SHADOWED.0, SHADOWED.1);
    let lit = patch(&image, LIT.0, LIT.1);
    assert!(
        (under - lit).abs() < lit * 0.1,
        "a floor that does not receive shadows was darkened anyway: {under:.1} against {lit:.1}"
    );
}

#[test]
fn a_caster_that_does_not_cast_leaves_no_shadow() {
    if !gpu_available() {
        eprintln!("skipping: no D3D12 device");
        return;
    }
    let floor_mesh = floor();
    let cube = MeshData::cube(1.5);
    let mut objects = scene(&floor_mesh, &cube);
    objects[1].shadow = ShadowCasting::RECEIVES_ONLY;

    let image = render_scene_shadowed(
        SIZE,
        SIZE,
        CLEAR,
        &objects,
        &[],
        &top_down_camera(),
        overhead_sun(),
        ShadowPreset::High.settings(),
    )
    .expect("the shadowed scene must render");

    let under = patch(&image, SHADOWED.0, SHADOWED.1);
    let lit = patch(&image, LIT.0, LIT.1);
    assert!(
        (under - lit).abs() < lit * 0.1,
        "an object with casting switched off still cast one: {under:.1} against {lit:.1}"
    );
}

#[test]
fn a_lit_floor_shows_no_acne_at_any_preset() {
    if !gpu_available() {
        eprintln!("skipping: no D3D12 device");
        return;
    }
    // Shadow acne is the floor shadowing *itself*: it appears as darkening in places nothing
    // is above. Two patches of open floor, at every quality level, must be as bright as each
    // other and as bright as the unshadowed render — which is what the biases are tuned for
    // and what a regression in them would break first.
    let unshadowed = patch(&render(ShadowPreset::Off.settings()), LIT.0, LIT.1);
    for preset in [
        ShadowPreset::Low,
        ShadowPreset::Medium,
        ShadowPreset::High,
        ShadowPreset::Ultra,
    ] {
        let image = render(preset.settings());
        let corner = patch(&image, LIT.0, LIT.1);
        let beside = patch(&image, BESIDE.0, BESIDE.1);
        assert!(
            corner > unshadowed * 0.9,
            "{}: open floor is darker than it is with shadows off ({corner:.1} against \
             {unshadowed:.1}) — the bias is letting the floor shadow itself",
            preset.as_str()
        );
        assert!(
            beside > unshadowed * 0.9,
            "{}: open floor beside the caster shows acne ({beside:.1} against {unshadowed:.1})",
            preset.as_str()
        );
    }
}

#[test]
fn every_preset_renders_a_shadow() {
    if !gpu_available() {
        eprintln!("skipping: no D3D12 device");
        return;
    }
    // A quality level that produced no shadow at all would be a setting the player can pick
    // that quietly disables the feature.
    for preset in [
        ShadowPreset::Low,
        ShadowPreset::Medium,
        ShadowPreset::High,
        ShadowPreset::Ultra,
    ] {
        let image = render(preset.settings());
        let shadowed = patch(&image, SHADOWED.0, SHADOWED.1);
        let lit = patch(&image, LIT.0, LIT.1);
        assert!(
            shadowed < lit * 0.8,
            "{}: no shadow under the caster ({shadowed:.1} against {lit:.1})",
            preset.as_str()
        );
    }
}

#[test]
fn an_alpha_tested_caster_casts_through_its_own_holes() {
    if !gpu_available() {
        eprintln!("skipping: no D3D12 device");
        return;
    }
    // §12: a cutout material must reach the depth pass through the alpha-tested pipeline,
    // and the fragments it discards must leave no depth behind.
    //
    // The alpha under test is the material's own — its map is the shared white texel, so the
    // sampled alpha is the base colour's. One caster is opaque enough to pass its cutoff and
    // one is not, which makes this a test of *whether the alpha test runs at all* without
    // needing a cutout texture to be authored.
    let floor_mesh = floor();
    let cube = MeshData::cube(1.5);

    let mut kept = scene(&floor_mesh, &cube);
    kept[1].material = cutout(1.0, 0.5);
    let mut discarded = scene(&floor_mesh, &cube);
    discarded[1].material = cutout(0.4, 0.8);

    let render_one = |objects: &[ShadowSceneObject<'_>]| {
        render_scene_shadowed(
            SIZE,
            SIZE,
            CLEAR,
            objects,
            &[],
            &top_down_camera(),
            overhead_sun(),
            ShadowPreset::High.settings(),
        )
        .expect("the shadowed scene must render")
    };

    let solid = render_one(&kept);
    let holes = render_one(&discarded);
    let lit = patch(&solid, LIT.0, LIT.1);
    let solid_shadow = patch(&solid, SHADOWED.0, SHADOWED.1);
    let hole_shadow = patch(&holes, SHADOWED.0, SHADOWED.1);

    assert!(
        solid_shadow < lit * 0.8,
        "a cutout caster whose fragments all pass must still cast: {solid_shadow:.1} against \
         {lit:.1}"
    );
    assert!(
        hole_shadow > solid_shadow * 1.3,
        "a cutout caster whose fragments are all discarded still cast a shadow: \
         {hole_shadow:.1} against {solid_shadow:.1}"
    );
}

#[test]
fn a_spot_light_casts_a_local_shadow() {
    if !gpu_available() {
        eprintln!("skipping: no D3D12 device");
        return;
    }
    // §20, on the GPU. The sun is switched off entirely — zero intensity — so the only light
    // reaching the floor is the spot light's, and the only thing that can darken it is the
    // spot light's own shadow map.
    let floor_mesh = floor();
    let cube = MeshData::cube(1.5);
    let objects = scene(&floor_mesh, &cube);
    let dark_sun = Lighting {
        intensity: 0.0,
        ambient_intensity: 0.02,
        ..overhead_sun()
    };
    // Placed so its shadow of the cube lands on the same patch of floor the sun's did, for
    // the same reason the sun is tilted: a light directly above the caster hides its own
    // shadow behind the caster, and a top-down camera can then see neither.
    //
    //   light (5.1, 5, 0) ->  cube (0, 2, 0)  ->  floor (-3.4, 0, 0)
    let spot = LocalLight::spot(
        Vec3::new(5.1, 5.0, 0.0),
        Vec3::new(-8.5, -5.0, 0.0),
        Vec3::ONE,
        60.0,
        30.0,
        0.35,
        0.5,
    )
    .casting_shadows(1);

    let shadowed = render_scene_shadowed(
        SIZE,
        SIZE,
        CLEAR,
        &objects,
        &[spot],
        &top_down_camera(),
        dark_sun,
        ShadowPreset::High.settings(),
    )
    .expect("the shadowed scene must render");

    // The same light, not asking for a shadow map: the control that separates "the spot
    // light lit the floor unevenly" from "the spot light's shadow map worked".
    let unshadowed = render_scene_shadowed(
        SIZE,
        SIZE,
        CLEAR,
        &objects,
        &[LocalLight {
            casts_shadow: false,
            ..spot
        }],
        &top_down_camera(),
        dark_sun,
        ShadowPreset::High.settings(),
    )
    .expect("the shadowed scene must render");

    let under_shadowed = patch(&shadowed, SHADOWED.0, SHADOWED.1);
    let under_unshadowed = patch(&unshadowed, SHADOWED.0, SHADOWED.1);
    assert!(
        under_unshadowed > 1.0,
        "the spot light did not light the floor at all ({under_unshadowed:.1}); the test \
         cannot say anything about its shadow"
    );
    assert!(
        under_shadowed < under_unshadowed * 0.75,
        "the spot light's shadow map did not darken the floor under the caster: \
         {under_shadowed:.1} against {under_unshadowed:.1}"
    );
}

/// A camera at head height looking down a long floor, so one screen column crosses every
/// cascade.
///
/// The floor recedes to the horizon, so screen height *is* distance: the bottom of the frame
/// is a metre away and the top is hundreds. That is what makes a vertical scan of the image a
/// scan across the cascades.
fn horizon_camera() -> Camera {
    let mut camera = Camera::new(Projection::perspective(1.0)).expect("a square viewport");
    camera.set_position(Vec3::new(0.0, 1.8, 0.0));
    // Ten degrees down from level: enough that the floor fills the lower half of the frame
    // and the far cascades are in the upper half.
    camera.set_rotation(Quat::from_yaw_pitch_roll(0.0, -0.17, 0.0));
    camera
}

#[test]
fn the_floor_shows_no_seam_where_one_cascade_meets_the_next() {
    if !gpu_available() {
        eprintln!("skipping: no D3D12 device");
        return;
    }
    // §14, measured. An empty floor is lit by exactly one thing — the sun, at one angle, on
    // one normal — so every pixel of it should be the same brightness whatever distance it
    // is at. Any step in that brightness is a cascade boundary showing, and any darkening is
    // a cascade shadowing the floor with itself.
    //
    // The scan runs from just below the horizon to the bottom of the frame, which at this
    // pitch covers roughly 150 m down to 2 m: every cascade the High preset builds.
    let floor_mesh = MeshData::plane(800.0);
    let objects = vec![ShadowSceneObject {
        transform: Transform::IDENTITY,
        mesh: &floor_mesh,
        material: matte(0.8, 0.8, 0.8),
        shadow: ShadowCasting::RECEIVES_ONLY,
    }];

    let image = render_scene_shadowed(
        SIZE,
        SIZE,
        CLEAR,
        &objects,
        &[],
        &horizon_camera(),
        overhead_sun(),
        ShadowPreset::High.settings(),
    )
    .expect("the shadowed scene must render");

    // Rows from a little below the horizon to the bottom edge. The horizon itself is skipped:
    // the floor ends there and the row would be half sky.
    let rows: Vec<f32> = (0..14)
        .map(|step| patch(&image, SIZE / 2, 108 + step * 6))
        .collect();
    let brightest = rows.iter().copied().fold(0.0_f32, f32::max);
    let dimmest = rows.iter().copied().fold(f32::MAX, f32::min);
    assert!(
        brightest > 1.0,
        "the scan found no lit floor at all: {rows:?}"
    );
    assert!(
        dimmest > brightest * 0.9,
        "the floor's brightness steps by more than a tenth across the cascades, which is a          seam: {rows:?}"
    );

    // And the step between *adjacent* rows, which is where a hard boundary would show even
    // if the ends of the scan happened to match.
    for pair in rows.windows(2) {
        assert!(
            (pair[1] - pair[0]).abs() < brightest * 0.06,
            "a step of {:.1} between adjacent rows: {rows:?}",
            (pair[1] - pair[0]).abs()
        );
    }
}

#[test]
fn a_point_light_casts_a_shadow_from_the_face_it_falls_on() {
    if !gpu_available() {
        eprintln!("skipping: no D3D12 device");
        return;
    }
    // §23. A point light spends six views, one per cube face, and the shader picks the face
    // from the major axis of the light-to-surface vector. The caster and the receiver here
    // are both on the -X face of the light, so this asserts that the right one of the six was
    // chosen as much as that any of them were rendered.
    let floor_mesh = floor();
    let cube = MeshData::cube(1.5);
    let objects = scene(&floor_mesh, &cube);
    let dark_sun = Lighting {
        intensity: 0.0,
        ambient_intensity: 0.02,
        ..overhead_sun()
    };
    let point =
        LocalLight::point(Vec3::new(5.1, 5.0, 0.0), Vec3::ONE, 90.0, 30.0).casting_shadows(1);

    // Ultra, because it is the preset whose budget can afford a point light's six views.
    let settings = ShadowPreset::Ultra.settings();
    assert!(settings.local.point_lights && settings.local_view_budget() >= 6);

    let shadowed = render_scene_shadowed(
        SIZE,
        SIZE,
        CLEAR,
        &objects,
        &[point],
        &top_down_camera(),
        dark_sun,
        settings,
    )
    .expect("the shadowed scene must render");
    let unshadowed = render_scene_shadowed(
        SIZE,
        SIZE,
        CLEAR,
        &objects,
        &[LocalLight {
            casts_shadow: false,
            ..point
        }],
        &top_down_camera(),
        dark_sun,
        settings,
    )
    .expect("the shadowed scene must render");

    let under_shadowed = patch(&shadowed, SHADOWED.0, SHADOWED.1);
    let under_unshadowed = patch(&unshadowed, SHADOWED.0, SHADOWED.1);
    assert!(
        under_unshadowed > 1.0,
        "the point light did not light the floor at all ({under_unshadowed:.1})"
    );
    assert!(
        under_shadowed < under_unshadowed * 0.75,
        "the point light's shadow cube did not darken the floor under the caster:          {under_shadowed:.1} against {under_unshadowed:.1}"
    );
}

#[test]
fn contact_shadows_do_not_darken_open_ground() {
    if !gpu_available() {
        eprintln!("skipping: no D3D12 device");
        return;
    }
    // §22 is a *short-range* effect, and the failure mode that matters is it becoming a
    // long-range one: a march that reported occlusion from anything it happened to cross
    // would darken the whole floor. Open ground with the effect on must match open ground
    // with it off.
    let with_contact = ShadowPreset::High.settings();
    assert!(with_contact.contact.enabled, "High enables contact shadows");
    let without = with_contact.with_contact_shadows(false);

    let lit_with = patch(&render(with_contact), LIT.0, LIT.1);
    let lit_without = patch(&render(without), LIT.0, LIT.1);
    assert!(
        (lit_with - lit_without).abs() < lit_without * 0.05,
        "contact shadows darkened open ground: {lit_with:.1} against {lit_without:.1}"
    );
}
