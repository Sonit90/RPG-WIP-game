//! The metallic-roughness lighting model, judged by pixels.
//!
//! Milestone 2 proved the four material descriptors reach the GPU. These tests prove the
//! shading *consumes* them, and each one is written so that only the thing it names can make
//! it pass: a normal map is asserted through a light that a flat surface cannot see at all, a
//! swapped roughness/metallic pair is caught by reading each channel back separately, and
//! occlusion is asserted to change the ambient term and to leave the sun alone.
//!
//! Everything renders through [`render_material_surface`], which draws one flat quad with a
//! known tangent frame — T along +X, N along +Z, B = cross(N, T) * w — through the real mesh
//! pipeline and the real root signature. Comparisons are between two renders rather than
//! against magic colours wherever the relationship is what matters, which is the same
//! discipline tests/lighting.rs uses.

#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Mutex, MutexGuard, PoisonError};

use engine_core::math::{Vec3, Vec4};
use engine_core::transform::Transform;
use engine_render_data::{Camera, Lighting, Projection};
use engine_renderer::material::{MATERIAL_TEXTURE_COUNT, MaterialDesc};
use engine_renderer::mesh::{MeshData, MeshVertex, SkinnedMeshData, SkinnedVertex};
use engine_renderer::offscreen::{
    CapturedImage, MaterialSurface, SURFACE_PROBE_SIZE, SceneObject, SkinnedObject,
    render_material_surface, render_scene, render_skinned_scene,
};

static GPU: Mutex<()> = Mutex::new(());

fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The four texels of a material that changes nothing: white base, flat normal, neutral ORM,
/// white emissive. Every test starts from this and replaces the one slot it is about.
const IDENTITY: [[u8; 4]; MATERIAL_TEXTURE_COUNT as usize] = [
    [255, 255, 255, 255],
    [128, 128, 255, 255],
    [255, 255, 255, 255],
    [255, 255, 255, 255],
];

/// The slot each texel of that array belongs to, named so a test reads as what it changes.
const BASE: usize = 0;
const NORMAL: usize = 1;
const ORM: usize = 2;
const EMISSIVE: usize = 3;

/// A texel set with one slot replaced.
fn with_texel(slot: usize, texel: [u8; 4]) -> [[u8; 4]; MATERIAL_TEXTURE_COUNT as usize] {
    let mut texels = IDENTITY;
    texels[slot] = texel;
    texels
}

/// Sunlight travelling in `direction`, with no ambient so one term is under test at a time.
fn sun(direction: Vec3) -> Lighting {
    Lighting {
        direction,
        ambient_intensity: 0.0,
        ..Lighting::default()
    }
}

/// Ambient only: no sun at all.
fn ambient_only(intensity: f32) -> Lighting {
    Lighting {
        intensity: 0.0,
        ambient_intensity: intensity,
        ..Lighting::default()
    }
}

/// The centre pixel of a rendered probe, or `None` when there is no adapter to render on.
fn shade(surface: MaterialSurface) -> Option<[u8; 4]> {
    match render_material_surface(surface) {
        Ok(image) => Some(center(&image)),
        Err(engine_renderer::RendererError::NoSuitableAdapter { examined }) => {
            println!("SKIP: no D3D12 adapter ({examined} examined)");
            None
        }
        Err(error) => panic!("render must succeed: {error}"),
    }
}

fn center(image: &CapturedImage) -> [u8; 4] {
    image
        .pixel(SURFACE_PROBE_SIZE / 2, SURFACE_PROBE_SIZE / 2)
        .expect("the centre pixel is inside the image")
}

// ---------------------------------------------------------------------------------------
// Normal mapping
// ---------------------------------------------------------------------------------------

#[test]
fn a_normal_map_tilts_the_surface_into_a_light_a_flat_one_cannot_see() {
    let _guard = gpu_lock();

    // The light travels towards -X, so it arrives from +X. The quad faces +Z, which is
    // exactly edge-on: a flat surface receives nothing at all. Any brightness in the second
    // render came from the normal map, through the tangent frame, and from nowhere else —
    // which is why this is the first test and not a comparison of two lit renders.
    let lighting = sun(Vec3::new(-1.0, 0.0, 0.0));

    let Some(flat) = shade(MaterialSurface {
        lighting,
        ..MaterialSurface::new(IDENTITY, 0)
    }) else {
        return;
    };
    // Tangent-space (+1, 0, ~0.55): tilted hard towards +T, which is +X in world space.
    let tilted = shade(MaterialSurface {
        lighting,
        ..MaterialSurface::new(with_texel(NORMAL, [255, 128, 200, 255]), 0)
    })
    .expect("the device rendered the first image");

    assert_eq!(
        flat[0], 0,
        "a flat surface edge-on to the light must receive nothing: {flat:?}"
    );
    assert!(
        tilted[0] > 60,
        "the normal map should tilt the surface into the light: {tilted:?}"
    );
}

#[test]
fn the_tangent_handedness_decides_which_way_a_bitangent_perturbation_tilts() {
    let _guard = gpu_lock();

    // The same normal map both times: tangent-space (0, +1, ~0.55), a tilt along the
    // *bitangent*. The bitangent is cross(N, T) * w, so w flips which world direction that
    // is — +Y when w is +1, -Y when it is -1.
    //
    // This is the test that fails if `tangent.w` is dropped, which is the classic normal
    // mapping bug: everything looks right until a mirrored UV island shades inside out.
    let texels = with_texel(NORMAL, [128, 255, 200, 255]);
    let from_above = sun(Vec3::new(0.0, -1.0, 0.0));

    let Some(right_handed) = shade(MaterialSurface {
        lighting: from_above,
        tangent_handedness: 1.0,
        ..MaterialSurface::new(texels, 0)
    }) else {
        return;
    };
    let left_handed = shade(MaterialSurface {
        lighting: from_above,
        tangent_handedness: -1.0,
        ..MaterialSurface::new(texels, 0)
    })
    .expect("the device rendered the first image");

    assert!(
        right_handed[0] > 60,
        "w = +1 puts the bitangent along +Y, into a light from above: {right_handed:?}"
    );
    assert_eq!(
        left_handed[0], 0,
        "w = -1 reverses it, and the surface tilts away from that light: {left_handed:?}"
    );
}

#[test]
fn the_shading_normal_view_shows_the_perturbed_normal_and_not_the_texel() {
    let _guard = gpu_lock();

    // Debug view 8 returns the normal *after* the TBN transform, encoded the way a normal map
    // encodes one. For the flat fallback that is the geometric normal, +Z, which encodes to
    // (0.5, 0.5, 1). View 5 returns the raw texel instead, and the two agreeing on the
    // fallback while disagreeing on a perturbation is what says the transform happened.
    let texels = with_texel(NORMAL, [255, 128, 200, 255]);

    let Some(flat_shaded) = shade(MaterialSurface::new(IDENTITY, 8)) else {
        return;
    };
    let tilted_shaded = shade(MaterialSurface::new(texels, 8)).expect("the device rendered");
    let tilted_raw = shade(MaterialSurface::new(texels, 5)).expect("the device rendered");

    assert!(
        flat_shaded[2] > 245 && flat_shaded[0].abs_diff(128) <= 2,
        "an unmapped surface's shading normal is its geometric one: {flat_shaded:?}"
    );
    assert!(
        tilted_shaded[0] > flat_shaded[0] + 40,
        "the perturbed normal must lean towards +X in world space: {tilted_shaded:?}"
    );
    assert_ne!(
        tilted_shaded, tilted_raw,
        "view 8 is the transformed normal, view 5 the texel; identical means no transform"
    );
}

// ---------------------------------------------------------------------------------------
// The ORM packing
// ---------------------------------------------------------------------------------------

#[test]
fn roughness_comes_from_green_and_metallic_from_blue() {
    let _guard = gpu_lock();

    // The one test that catches a swapped ORM channel, which is otherwise invisible until a
    // scene of plastic-looking metal shows up. Debug views 9 and 10 return the roughness and
    // the metallic the shading resolved, so a swap makes each read the other's number.
    //
    // ORM = (255, 26, 255): occluded not at all, nearly smooth, fully metal.
    let texels = with_texel(ORM, [255, 26, 255, 255]);
    let material = MaterialDesc::default()
        .with_roughness(1.0)
        .with_metallic(1.0);

    let Some(roughness) = shade(MaterialSurface {
        material,
        ..MaterialSurface::new(texels, 9)
    }) else {
        return;
    };
    let metallic = shade(MaterialSurface {
        material,
        ..MaterialSurface::new(texels, 10)
    })
    .expect("the device rendered");

    assert!(
        roughness[0] < 40,
        "green is roughness: 26/255 scaled by a factor of 1: {roughness:?}"
    );
    assert!(
        metallic[0] > 245,
        "blue is metallic: 255/255 scaled by a factor of 1: {metallic:?}"
    );
}

#[test]
fn the_orm_map_scales_the_material_factors_rather_than_replacing_them() {
    let _guard = gpu_lock();

    // glTF's convention, and what makes the white fallback the identity: a material with a
    // roughness of 0.5 and a map holding 0.5 is rougher than either alone would say.
    let texels = with_texel(ORM, [255, 128, 255, 255]);
    let material = MaterialDesc::default().with_roughness(0.5);

    let Some(scaled) = shade(MaterialSurface {
        material,
        ..MaterialSurface::new(texels, 9)
    }) else {
        return;
    };
    let unmapped = shade(MaterialSurface {
        material,
        ..MaterialSurface::new(IDENTITY, 9)
    })
    .expect("the device rendered");

    assert!(
        unmapped[0].abs_diff(128) <= 3,
        "with the neutral fallback the roughness is the factor alone: {unmapped:?}"
    );
    assert!(
        scaled[0].abs_diff(64) <= 4,
        "0.5 factor times a 0.5 texel is 0.25: {scaled:?}"
    );
}

#[test]
fn a_smoother_surface_returns_a_brighter_highlight_than_a_rough_one() {
    let _guard = gpu_lock();

    // Light and camera both on the surface's axis, so the half vector is the normal and the
    // specular lobe is at its peak. A black base colour removes the diffuse term entirely, so
    // every photon in these two images went through the GGX lobe: a smooth surface
    // concentrates them and a rough one spreads them out.
    let lighting = sun(Vec3::new(0.0, 0.0, -1.0));
    let black = MaterialDesc::colored(0.0, 0.0, 0.0);

    let Some(rough) = shade(MaterialSurface {
        material: black.with_roughness(1.0),
        lighting,
        ..MaterialSurface::new(IDENTITY, 0)
    }) else {
        return;
    };
    let smooth = shade(MaterialSurface {
        material: black.with_roughness(0.1),
        lighting,
        ..MaterialSurface::new(IDENTITY, 0)
    })
    .expect("the device rendered");

    assert!(
        rough[0] < 12,
        "a fully rough dielectric barely reflects: {rough:?}"
    );
    assert!(
        smooth[0] > rough[0] + 20,
        "a smooth one concentrates the same energy: {smooth:?} against {rough:?}"
    );
}

#[test]
fn a_metal_loses_its_diffuse_lobe_and_tints_what_it_reflects() {
    let _guard = gpu_lock();

    // The metallic workflow in one assertion, and the light is deliberately *off* the mirror
    // direction: at the specular peak a smooth metal is brighter than any dielectric, because
    // it is showing the light back at the camera. Away from that peak it is nearly black,
    // because it has no diffuse lobe to fall back on — which is the property under test.
    //
    // The light arrives from up and to the left at roughly 60 degrees; the camera is on the
    // surface's axis, so the reflected lobe goes off past it.
    let lighting = sun(Vec3::new(-0.75, -0.75, -0.5));
    let red = MaterialDesc::colored(0.9, 0.1, 0.1).with_roughness(0.4);

    let Some(dielectric) = shade(MaterialSurface {
        material: red.with_metallic(0.0),
        lighting,
        ..MaterialSurface::new(IDENTITY, 0)
    }) else {
        return;
    };
    let metal = shade(MaterialSurface {
        material: red.with_metallic(1.0),
        lighting,
        ..MaterialSurface::new(IDENTITY, 0)
    })
    .expect("the device rendered");

    assert!(
        dielectric[0] > 80,
        "a red dielectric off-axis still diffuses: {dielectric:?}"
    );
    assert!(
        metal[0] < dielectric[0] / 2,
        "a metal has no diffuse lobe to fall back on: {metal:?} against {dielectric:?}"
    );
    assert!(
        metal[0] > metal[1],
        "and what it does reflect is tinted by its base colour: {metal:?}"
    );
}

// ---------------------------------------------------------------------------------------
// Occlusion, emission, and the two colour-space slots
// ---------------------------------------------------------------------------------------

#[test]
fn occlusion_darkens_the_ambient_term_and_leaves_the_sun_alone() {
    let _guard = gpu_lock();

    // The red channel of the ORM map is how much *sky* reaches a point. Applying it to the
    // sun as well is the common shortcut, and it produces a surface that goes dark when a
    // light moves onto it — so this asserts both halves: half occlusion halves the ambient,
    // and changes the sunlit render by nothing at all.
    let occluded = with_texel(ORM, [128, 255, 255, 255]);

    let Some(lit_ambient) = shade(MaterialSurface {
        lighting: ambient_only(0.5),
        ..MaterialSurface::new(IDENTITY, 0)
    }) else {
        return;
    };
    let dark_ambient = shade(MaterialSurface {
        lighting: ambient_only(0.5),
        ..MaterialSurface::new(occluded, 0)
    })
    .expect("the device rendered");

    let sunlit = shade(MaterialSurface {
        lighting: sun(Vec3::new(0.0, 0.0, -1.0)),
        ..MaterialSurface::new(IDENTITY, 0)
    })
    .expect("the device rendered");
    let sunlit_occluded = shade(MaterialSurface {
        lighting: sun(Vec3::new(0.0, 0.0, -1.0)),
        ..MaterialSurface::new(occluded, 0)
    })
    .expect("the device rendered");

    let ratio = f32::from(dark_ambient[0]) / f32::from(lit_ambient[0]);
    assert!(
        (ratio - 0.5).abs() < 0.05,
        "half occlusion should halve the ambient: {dark_ambient:?} against {lit_ambient:?}"
    );
    assert_eq!(
        sunlit, sunlit_occluded,
        "occlusion must not touch direct light"
    );
}

#[test]
fn the_normal_scale_dials_a_normal_map_between_flat_and_exaggerated() {
    let _guard = gpu_lock();

    // glTF's `normalTexture.scale`. The light arrives edge-on to the geometric surface, so
    // the only thing that can light this quad is the normal map — and how much of it lands is
    // exactly what the scale controls. Zero must put the surface back where the geometry says
    // it is, which is the assertion that catches a scale applied to all three components
    // (where it would be undone by the renormalize and do nothing at all).
    let lighting = sun(Vec3::new(-1.0, 0.0, 0.0));
    let texels = with_texel(NORMAL, [255, 128, 200, 255]);

    let Some(full) = shade(MaterialSurface {
        material: MaterialDesc::default().with_normal_scale(1.0),
        lighting,
        ..MaterialSurface::new(texels, 0)
    }) else {
        return;
    };
    let half = shade(MaterialSurface {
        material: MaterialDesc::default().with_normal_scale(0.35),
        lighting,
        ..MaterialSurface::new(texels, 0)
    })
    .expect("the device rendered");
    let flat = shade(MaterialSurface {
        material: MaterialDesc::default().with_normal_scale(0.0),
        lighting,
        ..MaterialSurface::new(texels, 0)
    })
    .expect("the device rendered");

    assert!(
        full[0] > 60,
        "the map at full strength tilts into the light: {full:?}"
    );
    assert!(
        half[0] < full[0] && half[0] > 0,
        "a smaller scale must tilt it less, not differently: {half:?} against {full:?}"
    );
    assert_eq!(
        flat[0], 0,
        "a scale of zero is the geometric normal, which this light cannot see: {flat:?}"
    );
}

#[test]
fn the_occlusion_strength_dials_the_ambient_darkening() {
    let _guard = gpu_lock();

    // glTF's `occlusionTexture.strength`, and glTF's own formula: `1 + strength * (map - 1)`.
    // A strength of zero must mean *no occlusion*, not "occlusion of zero" — the difference
    // between an artist dialling a map back and the scene going black.
    let occluded = with_texel(ORM, [64, 255, 255, 255]);
    let lighting = ambient_only(0.5);

    let Some(applied) = shade(MaterialSurface {
        material: MaterialDesc::default().with_occlusion_strength(1.0),
        lighting,
        ..MaterialSurface::new(occluded, 0)
    }) else {
        return;
    };
    let ignored = shade(MaterialSurface {
        material: MaterialDesc::default().with_occlusion_strength(0.0),
        lighting,
        ..MaterialSurface::new(occluded, 0)
    })
    .expect("the device rendered");
    let unoccluded = shade(MaterialSurface {
        material: MaterialDesc::default().with_occlusion_strength(1.0),
        lighting,
        ..MaterialSurface::new(IDENTITY, 0)
    })
    .expect("the device rendered");

    assert!(
        applied[0] < unoccluded[0] / 2,
        "the map at full strength must darken the ambient: {applied:?} against {unoccluded:?}"
    );
    assert_eq!(
        ignored, unoccluded,
        "a strength of zero must be identical to having no occlusion map at all"
    );

    // And the occlusion debug view reports the value the shading used, strength included.
    let view = shade(MaterialSurface {
        material: MaterialDesc::default().with_occlusion_strength(0.5),
        ..MaterialSurface::new(occluded, 11)
    })
    .expect("the device rendered");
    // 1 + 0.5 * (64/255 - 1) = 0.626, which is 160 of 255.
    assert!(
        view[0].abs_diff(160) <= 3,
        "half strength on a 64 texel should read about 160, got {view:?}"
    );
}

#[test]
fn the_emissive_map_scales_the_emissive_factor() {
    let _guard = gpu_lock();

    // glTF multiplies the two, and so does the shader. Lit from behind and with a black base
    // colour, everything in these images is the emissive term.
    let behind = sun(Vec3::new(0.0, 0.0, 1.0));
    let glowing = MaterialDesc::colored(0.0, 0.0, 0.0).with_emissive(Vec3::new(0.0, 1.0, 0.0));

    let Some(full) = shade(MaterialSurface {
        material: glowing,
        lighting: behind,
        ..MaterialSurface::new(IDENTITY, 0)
    }) else {
        return;
    };
    // An sRGB view: 188 decodes to about half in linear, which is the point of asserting the
    // *ratio* rather than the texel. A map read as raw data instead would halve at 128.
    let half = shade(MaterialSurface {
        material: glowing,
        lighting: behind,
        ..MaterialSurface::new(with_texel(EMISSIVE, [0, 188, 0, 255]), 0)
    })
    .expect("the device rendered");
    let no_factor = shade(MaterialSurface {
        material: MaterialDesc::colored(0.0, 0.0, 0.0),
        lighting: behind,
        ..MaterialSurface::new(IDENTITY, 0)
    })
    .expect("the device rendered");

    assert!(
        full[1] > 245,
        "a white emissive map passes the factor through: {full:?}"
    );
    let ratio = f32::from(half[1]) / f32::from(full[1]);
    assert!(
        (ratio - 0.5).abs() < 0.08,
        "a half-bright map should halve the emission: {half:?} against {full:?}"
    );
    assert_eq!(
        no_factor[1], 0,
        "and with no emissive factor the map emits nothing: {no_factor:?}"
    );
}

#[test]
fn the_base_colour_map_is_read_as_colour_and_the_orm_map_as_data() {
    let _guard = gpu_lock();

    // The colour-space half of the material ABI. A texel of 128 in the base colour map is
    // sRGB and decodes to about 0.216 linear; the same 128 in the ORM map is data and means
    // 0.5. Reading either the way the other is read is the classic washed-out or muddy
    // import, and it has no symptom other than this.
    // A base colour factor of exactly white, so the number below is the decoded texel and
    // nothing else — `MaterialDesc::default` is 0.8 grey, which would scale it.
    let Some(base) = shade(MaterialSurface {
        material: MaterialDesc::colored(1.0, 1.0, 1.0),
        lighting: ambient_only(1.0),
        ..MaterialSurface::new(with_texel(BASE, [128, 128, 128, 255]), 0)
    }) else {
        return;
    };
    let orm_channel = shade(MaterialSurface {
        material: MaterialDesc::default().with_roughness(1.0),
        ..MaterialSurface::new(with_texel(ORM, [255, 128, 255, 255]), 9)
    })
    .expect("the device rendered");

    assert!(
        base[0].abs_diff(55) <= 4,
        "an sRGB 128 is about 0.216 linear, which is 55 of 255: {base:?}"
    );
    assert!(
        orm_channel[0].abs_diff(128) <= 3,
        "a data 128 is 0.5, and stays 0.5: {orm_channel:?}"
    );
}

#[test]
fn a_material_with_no_maps_at_all_still_shades_from_its_constants() {
    let _guard = gpu_lock();

    // Backward compatibility, asserted rather than assumed: every existing asset in this
    // engine has exactly this material — constants and four shared fallbacks. The identity
    // texels above are what those fallbacks hold, so an explicit set and the fallback set
    // must produce the same pixel to the bit.
    let material = MaterialDesc::colored(0.6, 0.4, 0.2)
        .with_roughness(0.35)
        .with_metallic(0.25);
    let lighting = sun(Vec3::new(-0.4, -1.0, -0.6));

    let Some(explicit) = shade(MaterialSurface {
        material,
        lighting,
        ..MaterialSurface::new(IDENTITY, 0)
    }) else {
        return;
    };
    assert!(
        explicit[0] > explicit[1] && explicit[1] > explicit[2],
        "the constants alone must still colour the surface: {explicit:?}"
    );
    assert!(explicit[0] > 20, "and light it: {explicit:?}");
}

// ---------------------------------------------------------------------------------------
// The other two pipelines
// ---------------------------------------------------------------------------------------

/// A camera two metres back, looking at the origin.
fn camera() -> Camera {
    let mut camera = Camera::new(Projection::perspective(1.0)).expect("a valid projection");
    camera.set_position(Vec3::new(0.0, 0.0, 2.0));
    camera
}

const SIZE: u32 = 64;
const CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

#[test]
fn the_skinned_pipeline_shades_a_surface_the_way_the_static_one_does() {
    let _guard = gpu_lock();

    // Both pipelines call the same `shade_surface`, so what this actually checks is the
    // vertex half: that the skinned shader carries a tangent through the bone blend at all.
    // A skinned pipeline that dropped it would shade with a zero tangent, take the guard in
    // `apply_normal_map`, and — because these materials have no normal map — still look
    // identical. So the material here is lit off-axis with a low roughness, where the
    // specular lobe makes any difference in the interpolated frame visible.
    let material = MaterialDesc::colored(0.8, 0.8, 0.8)
        .with_roughness(0.25)
        .with_metallic(0.5);
    let lighting = sun(Vec3::new(-0.4, -0.6, -1.0));

    let corners = [
        (
            Vec3::new(-1.0, -1.0, 0.0),
            engine_core::math::Vec2::new(0.0, 1.0),
        ),
        (
            Vec3::new(1.0, -1.0, 0.0),
            engine_core::math::Vec2::new(1.0, 1.0),
        ),
        (
            Vec3::new(1.0, 1.0, 0.0),
            engine_core::math::Vec2::new(1.0, 0.0),
        ),
        (
            Vec3::new(-1.0, 1.0, 0.0),
            engine_core::math::Vec2::new(0.0, 0.0),
        ),
    ];
    let tangent = Vec4::new(1.0, 0.0, 0.0, 1.0);
    let indices = vec![0, 1, 2, 0, 2, 3];

    let static_mesh = MeshData::new(
        corners
            .iter()
            .map(|(position, uv)| MeshVertex::new(*position, Vec3::Z, *uv).with_tangent(tangent))
            .collect(),
        indices.clone(),
    )
    .expect("the quad is well formed");
    let skinned_mesh = SkinnedMeshData::new(
        corners
            .iter()
            .map(|(position, uv)| {
                SkinnedVertex::new(*position, Vec3::Z, *uv, [0; 4], [1.0, 0.0, 0.0, 0.0])
                    .with_tangent(tangent)
            })
            .collect(),
        indices,
    )
    .expect("the quad is well formed");

    let statically = match render_scene(
        SIZE,
        SIZE,
        CLEAR,
        &[SceneObject {
            mesh: &static_mesh,
            material,
            transform: Transform::IDENTITY,
        }],
        &camera(),
        lighting,
    ) {
        Ok(image) => image,
        Err(engine_renderer::RendererError::NoSuitableAdapter { examined }) => {
            println!("SKIP: no D3D12 adapter ({examined} examined)");
            return;
        }
        Err(error) => panic!("render must succeed: {error}"),
    };
    let skinned = render_skinned_scene(
        SIZE,
        SIZE,
        CLEAR,
        &[SkinnedObject {
            mesh: &skinned_mesh,
            material,
            transform: Transform::IDENTITY,
            // One identity bone: the pose is the bind pose, so any difference between the
            // two images is the shading and not the skinning.
            palette: &[engine_core::math::Mat4::IDENTITY],
        }],
        &camera(),
        lighting,
    )
    .expect("the device rendered the first image");

    let a = statically.pixel(SIZE / 2, SIZE / 2).expect("centre pixel");
    let b = skinned.pixel(SIZE / 2, SIZE / 2).expect("centre pixel");
    assert!(a[0] > 20, "the static quad must be lit at all: {a:?}");
    for channel in 0..4 {
        assert!(
            a[channel].abs_diff(b[channel]) <= 1,
            "the two pipelines must shade the same surface the same way: {a:?} against {b:?}"
        );
    }
}
