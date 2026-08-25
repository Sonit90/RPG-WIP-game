//! The transparent rendering contract, and the debug scene that demonstrates it.
//!
//! # The three surface classes
//!
//! Every material declares exactly one [`SurfaceClass`], and that declaration is the whole of
//! the renderer's transparency vocabulary. There is no material graph, no per-object override
//! and no inference from an alpha value.
//!
//! | class | phase | depth test | depth write | blend | order |
//! |---|---|---|---|---|---|
//! | [`SurfaceClass::Opaque`] | opaque | `LESS_EQUAL` | yes | none | by `(material, mesh)` |
//! | [`SurfaceClass::AlphaTest`] | opaque | `LESS_EQUAL` | yes | none | by `(material, mesh)` |
//! | [`SurfaceClass::Transparent`] | transparent | `LESS_EQUAL` | **no** | `src.a` over `1 - src.a` | back to front |
//!
//! Alpha testing lives in the *opaque* phase deliberately. A cutout fragment either survives
//! whole or does not exist, so it needs no sorting and can write depth like anything else —
//! which is what keeps foliage cheap and, more importantly, keeps it in the depth buffer the
//! transparent phase later tests against.
//!
//! # Where the phase sits in the frame
//!
//! ```text
//! shadow stage -> opaque batches -> skinned -> TRANSPARENT -> particles -> overlay -> UI
//! ```
//!
//! After every opaque draw, because a blended surface has to test against a finished depth
//! buffer; before the particles, because both are blended and they share a screen; before the
//! interface, because a health bar drawn under a spell effect is a health bar nobody can read.
//!
//! # What the renderer does, step by step
//!
//! 1. The cull produces one visible list, exactly as before.
//! 2. [`RenderList::split_transparent`](crate::render_list::RenderList::split_transparent)
//!    moves every entry whose material is [`SurfaceClass::Transparent`] into a second list.
//!    What is left is byte-identical to the list the opaque phase always received.
//! 3. The opaque list is grouped by `(material, mesh)` and recorded unchanged.
//! 4. The transparent list is ordered by
//!    [`sort_back_to_front`](crate::render_list::RenderList::sort_back_to_front): farthest
//!    first by squared distance from the camera, ties broken by position in the list.
//! 5. Each transparent object is recorded as its own draw, in that order, with a pipeline
//!    state whose blend is enabled and whose depth write mask is zero.
//!
//! # What is deliberately not here
//!
//! * **No order-independent transparency.** No weighted-blended OIT, no per-pixel lists, no
//!   depth peeling. Back-to-front sorting of whole objects is the simplest thing that is
//!   correct for the cases the game has.
//! * **No refraction and no screen-space transparency.** Nothing samples the colour target.
//! * **No new shader permutation.** A transparent surface is shaded by the identical pixel
//!   shader an opaque one is; `shade_surface` already returns `float4(color, albedo.a *
//!   tint.a)` and always did. Only the two pipeline descriptors differ.
//! * **No HDR, bloom, fog or stylized lighting.** None of them is a prerequisite for blending
//!   and none is implemented here.
//!
//! # Known limitations, named rather than discovered
//!
//! * **Sorting is per object, by origin.** Two transparent surfaces that intersect have no
//!   correct back-to-front order at all, and a large or concave transparent mesh is answered
//!   for by its origin. See
//!   [`sort_back_to_front`](crate::render_list::RenderList::sort_back_to_front).
//! * **Transparent surfaces are single-sided.** They are rasterized with the same back-face
//!   culling opaque geometry uses, so a transparent quad seen from behind is invisible. Author
//!   effect geometry facing the player, or build it as two quads.
//! * **Shadows are unchanged.** A transparent material casts an opaque shadow if its entity
//!   says it casts one. That is the entity's decision, not the material's:
//!   [`ShadowCasting::RECEIVES_ONLY`](engine_render_data::ShadowCasting::RECEIVES_ONLY) is what
//!   a ghost wants, and it existed before this phase did.
//! * **Skinned characters are drawn in the opaque phase.** A skinned material that declares
//!   itself transparent does get the blended pipeline state, but it is recorded with the
//!   characters rather than sorted among the rigid transparent objects.
//! * **Blending is not commutative and the sort is per frame.** Two transparent objects at the
//!   same distance are drawn in list order, which is stable but arbitrary. Give them distinct
//!   depths if the layering matters.
//!
//! # The debug scene
//!
//! [`debug_scene`] is the smallest scene that exercises all of it: an opaque cube, an
//! alpha-tested foliage cross, an alpha-tested quad below its own cutoff (which must vanish),
//! a transparent quad in front of the cube, and two overlapping transparent quads at different
//! depths. It is plain data — meshes, materials and transforms — so the pixel tests and any
//! interactive viewer draw the same objects rather than two similar sets.

use engine_core::math::{Vec2, Vec3};
use engine_core::transform::Transform;
use engine_render_data::{Camera, Lighting, Projection};

use crate::material::MaterialDesc;
#[cfg(test)]
use crate::material::SurfaceClass;
use crate::mesh::{MeshData, MeshVertex};

/// One object in the debug scene: what to draw, how it looks, and where it is.
///
/// Owns its mesh rather than referencing one, because the scene is built once by a test or a
/// viewer and handed straight to whatever uploads it.
pub struct DebugObject {
    /// What this object is there to demonstrate, for test failure messages.
    pub name: &'static str,
    /// Its geometry.
    pub mesh: MeshData,
    /// Its surface, including the [`SurfaceClass`] that decides its phase.
    pub material: MaterialDesc,
    /// Where it is in the world.
    pub transform: Transform,
}

/// The alpha threshold every alpha-tested material in the debug scene uses.
///
/// One constant so the two cutout objects — the one above it and the one below it — cannot
/// drift apart and turn a failing test into a confusing one.
pub const DEBUG_ALPHA_CUTOFF: f32 = 0.5;

/// The opacity of every transparent material in the debug scene.
///
/// A half exactly, because it makes the expected blend arithmetic something a test can state
/// in one line: one layer lands halfway between the background and the surface colour, and two
/// stacked layers land three quarters of the way.
pub const DEBUG_OPACITY: f32 = 0.5;

/// An upright quad of `size`, centred on the origin, facing `+Z`.
///
/// The shape a spell card, a ghost plane and a foliage card all are. Facing `+Z` and wound
/// counter-clockwise seen from `+Z`, which is what the rasterizer calls front-facing — a quad
/// wound the other way is culled and looks like a missing object rather than like a winding
/// mistake.
#[must_use]
pub fn upright_quad(size: f32) -> MeshData {
    let half = size * 0.5;
    let normal = Vec3::Z;
    let vertices = vec![
        MeshVertex::new(Vec3::new(-half, -half, 0.0), normal, Vec2::new(0.0, 1.0)),
        MeshVertex::new(Vec3::new(half, -half, 0.0), normal, Vec2::new(1.0, 1.0)),
        MeshVertex::new(Vec3::new(half, half, 0.0), normal, Vec2::new(1.0, 0.0)),
        MeshVertex::new(Vec3::new(-half, half, 0.0), normal, Vec2::new(0.0, 0.0)),
    ];
    match MeshData::new(vertices, vec![0, 1, 2, 0, 2, 3]) {
        Ok(mesh) => mesh,
        // Four vertices and six in-range indices. If this ever fails the constructor's
        // contract changed, and a quad that silently became something else would be worse
        // than a panic naming the reason.
        Err(error) => panic!("the debug quad must be valid mesh data: {error}"),
    }
}

/// The camera the debug scene is authored for, at `width` by `height`.
///
/// Looks down `-Z` at the origin from `+Z`, so "farther from the camera" and "smaller Z" are
/// the same statement and a test can reason about the ordering without composing a matrix.
///
/// # Panics
///
/// Panics if `width` or `height` is zero, which is not a scene the camera can be built for.
#[must_use]
pub fn debug_camera(width: u32, height: u32) -> Camera {
    let aspect = width as f32 / height as f32;
    let mut camera = match Camera::new(Projection::perspective(aspect)) {
        Ok(camera) => camera,
        Err(error) => panic!("the debug camera's projection must be valid: {error}"),
    };
    if let Err(error) = camera.look_at(Vec3::new(0.0, 0.0, 6.0), Vec3::ZERO, Vec3::Y) {
        panic!("the debug camera must be able to look at the origin: {error}");
    }
    camera
}

/// Flat, bright lighting for the debug scene.
///
/// Strong ambient and a weak sun on purpose: the scene is about blending arithmetic, and a
/// surface whose brightness depends on which way it happens to face makes the expected colour
/// of a blend a lighting calculation rather than a one-line multiplication.
#[must_use]
pub fn debug_lighting() -> Lighting {
    Lighting {
        ambient_intensity: 1.0,
        intensity: 0.0,
        ..Lighting::default()
    }
}

/// The debug scene: one object per thing the transparent path has to get right.
///
/// In the order they are listed, and *not* in the order they are drawn — the renderer decides
/// that, which is the point. A viewer or a test hands the whole list over and the phases sort
/// themselves out.
///
/// | object | class | what it proves |
/// |---|---|---|
/// | `opaque_cube` | [`SurfaceClass::Opaque`] | the opaque path is untouched, and it is what the transparent quads blend over |
/// | `foliage_cross` | [`SurfaceClass::AlphaTest`] | a cutout above its threshold draws, writes depth and is not blended |
/// | `foliage_discarded` | [`SurfaceClass::AlphaTest`] | a cutout below its threshold vanishes entirely — the test really fires |
/// | `transparent_quad` | [`SurfaceClass::Transparent`] | one blended layer over opaque geometry |
/// | `transparent_far` / `transparent_near` | [`SurfaceClass::Transparent`] | two overlapping layers, which only look right if they are drawn farthest first |
///
/// The two overlapping quads are placed off to one side of the cube so that the region where
/// they overlap each other contains nothing else, which is what lets a test read one pixel and
/// know exactly which two surfaces produced it.
#[must_use]
pub fn debug_scene() -> Vec<DebugObject> {
    let cutout = |red: f32, green: f32, blue: f32, alpha: f32| {
        let mut material =
            MaterialDesc::colored(red, green, blue).with_alpha_cutoff(DEBUG_ALPHA_CUTOFF);
        material.base_color.w = alpha;
        material
    };

    vec![
        DebugObject {
            name: "opaque_cube",
            mesh: MeshData::cube(1.5),
            material: MaterialDesc::colored(0.9, 0.9, 0.9),
            transform: Transform::from_translation(Vec3::new(-1.6, 0.0, 0.0)),
        },
        // Two quads crossed at a right angle: the classic foliage card. Its alpha is above the
        // cutoff, so every fragment survives - with a base colour map it would be the map that
        // carves the leaf shape, and the constant stands in for one here.
        DebugObject {
            name: "foliage_cross",
            mesh: upright_quad(1.4),
            material: cutout(0.2, 0.7, 0.2, 0.9),
            transform: Transform::from_translation(Vec3::new(1.6, 0.0, 0.0)),
        },
        DebugObject {
            name: "foliage_cross_crossbar",
            mesh: upright_quad(1.4),
            material: cutout(0.2, 0.7, 0.2, 0.9),
            transform: Transform::from_translation(Vec3::new(1.6, 0.0, 0.0)).with_rotation(
                engine_core::math::Quat::from_axis_angle(Vec3::Y, std::f32::consts::FRAC_PI_2),
            ),
        },
        // Below the cutoff: every fragment discards and nothing is drawn. It sits directly in
        // front of the cube, so if the alpha test ever stopped firing this object would hide
        // the cube and the failure would be unmissable.
        DebugObject {
            name: "foliage_discarded",
            mesh: upright_quad(1.0),
            material: cutout(1.0, 0.0, 0.0, 0.25),
            transform: Transform::from_translation(Vec3::new(-1.6, 0.0, 1.5)),
        },
        // One blended layer in front of the cube.
        DebugObject {
            name: "transparent_quad",
            mesh: upright_quad(1.0),
            material: MaterialDesc::colored(1.0, 0.0, 0.0).with_opacity(DEBUG_OPACITY),
            transform: Transform::from_translation(Vec3::new(-1.6, 0.0, 2.0)),
        },
        // Two blended layers over the background, listed far-then-near but deliberately not
        // adjacent in the list: the sort has to do the work, not the authoring order.
        DebugObject {
            name: "transparent_far",
            mesh: upright_quad(1.0),
            material: MaterialDesc::colored(0.0, 0.0, 1.0).with_opacity(DEBUG_OPACITY),
            transform: Transform::from_translation(Vec3::new(1.6, 1.4, 1.0)),
        },
        DebugObject {
            name: "transparent_near",
            mesh: upright_quad(1.0),
            material: MaterialDesc::colored(0.0, 1.0, 0.0).with_opacity(DEBUG_OPACITY),
            transform: Transform::from_translation(Vec3::new(1.6, 1.4, 2.0)),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_debug_scene_covers_every_surface_class() {
        // The scene's whole purpose. If a class ever stopped being represented, the pixel
        // tests built on it would keep passing while covering less.
        let scene = debug_scene();
        for class in SurfaceClass::ALL {
            assert!(
                scene.iter().any(|object| object.material.surface == class),
                "the debug scene must contain a {} object",
                class.name()
            );
        }
    }

    #[test]
    fn the_scene_contains_two_overlapping_transparent_objects() {
        // Overlapping in X and Y and separated in Z is what makes the ordering observable at
        // all: two transparent quads that do not overlap produce the same picture in either
        // order, and a test built on them would prove nothing.
        let scene = debug_scene();
        let Some(far) = scene.iter().find(|object| object.name == "transparent_far") else {
            panic!("the scene names a far transparent quad");
        };
        let Some(near) = scene
            .iter()
            .find(|object| object.name == "transparent_near")
        else {
            panic!("the scene names a near transparent quad");
        };

        assert_eq!(far.transform.translation.x, near.transform.translation.x);
        assert_eq!(far.transform.translation.y, near.transform.translation.y);
        assert!(
            near.transform.translation.z > far.transform.translation.z,
            "the near quad must be nearer the camera, which looks down -Z from +Z"
        );
    }

    #[test]
    fn every_material_in_the_scene_is_valid() {
        // The class and the cutoff have to agree or `Material::new` rejects them, and a debug
        // scene that cannot be uploaded is worse than no debug scene.
        for object in debug_scene() {
            let desc = object.material;
            match desc.surface {
                SurfaceClass::AlphaTest => assert!(
                    desc.alpha_cutoff > 0.0,
                    "{} is alpha tested and needs a threshold",
                    object.name
                ),
                SurfaceClass::Opaque | SurfaceClass::Transparent => assert_eq!(
                    desc.alpha_cutoff,
                    0.0,
                    "{} is {} and must carry no threshold",
                    object.name,
                    desc.surface.name()
                ),
            }
        }
    }

    #[test]
    fn the_cutout_pair_sits_either_side_of_one_threshold() {
        // One object above the cutoff and one below it, so the scene demonstrates both halves
        // of the test rather than only the half that draws.
        let scene = debug_scene();
        let Some(visible) = scene.iter().find(|object| object.name == "foliage_cross") else {
            panic!("the scene names a visible cutout");
        };
        let Some(discarded) = scene
            .iter()
            .find(|object| object.name == "foliage_discarded")
        else {
            panic!("the scene names a discarded cutout");
        };

        assert!(visible.material.base_color.w > DEBUG_ALPHA_CUTOFF);
        assert!(discarded.material.base_color.w < DEBUG_ALPHA_CUTOFF);
    }

    #[test]
    fn the_upright_quad_faces_the_camera() {
        // Wound counter-clockwise seen from +Z, where the debug camera is. The other winding
        // is culled, and every transparent test would read the background and report that
        // blending is broken.
        let quad = upright_quad(2.0);
        let triangle = [
            quad.vertices()[quad.indices()[0] as usize].position,
            quad.vertices()[quad.indices()[1] as usize].position,
            quad.vertices()[quad.indices()[2] as usize].position,
        ];
        let normal = (triangle[1] - triangle[0]).cross(triangle[2] - triangle[0]);
        assert!(normal.z > 0.0, "the quad's front face must point at +Z");
    }
}
