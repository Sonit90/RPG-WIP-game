//! A body of water's mesh: one transparent quad at the surface.
//!
//! ```text
//!  WaterBody ──► water_model ──► ModelAsset ──► one render entity, drawn transparent
//! ```
//!
//! # What is in the mesh and what is on the transform
//!
//! The quad is sized to the body's half-extents and sits at its surface height. The **yaw is
//! not baked in** — it goes on the render entity's transform, the way every placement's yaw
//! does. A mesh carrying its own rotation is a mesh that has to be rebuilt when the body is
//! turned, and turning a body is one key press in the editor.
//!
//! # One material, tiled by the world
//!
//! `"water"` from the starter library, which is the only transparent entry in it. The UV is
//! the world position over [`TEXTURE_METRES`](crate::procedural::terrain::TEXTURE_METRES) —
//! the same rule the terrain uses — so a pond and a lake have the same texel density and
//! neither is stretched by being large.
//!
//! # Cost model
//!
//! Two triangles. Rebuilt when the body changes, which is a frame in the editor.

use engine_core::math::{Vec2, Vec3};
use engine_environment::WaterBody;

use crate::mesh::{Geometry, MeshVertex, SubMesh};
use crate::model::ModelAsset;
use crate::procedural::starter::{StarterMaterial, starter_material};
use crate::procedural::terrain::TEXTURE_METRES;

/// The model for one body of water, textured with the named starter material.
///
/// The material is named rather than passed so a level's water and the browser's
/// `materials/` folder cannot drift apart, exactly as
/// [`terrain_model`](crate::procedural::terrain::terrain_model) does it.
///
/// # Panics
///
/// Never. A material the starter library does not have falls back to water, and a body that
/// is not a volume produces a model with no triangles — which is what a level with no water
/// looks like anyway.
#[must_use]
pub fn water_model(body: &WaterBody, material: &str) -> ModelAsset {
    let surface = starter_material(material).unwrap_or_else(water);
    if !body.is_valid() {
        return ModelAsset::rigid(
            "water",
            Vec::new(),
            vec![surface.material()],
            vec![surface.texture()],
        );
    }

    // Local space: the body's own rectangle, centred on the origin and flat in Y. The centre
    // and the yaw both live on the transform, so a body that is moved or turned reuses this
    // mesh unchanged.
    let (hx, hz) = (body.half_extents.x, body.half_extents.y);
    // Tiled by world metres, from the body's own extents rather than from where it stands:
    // moving a pond must not slide the texture across it.
    let (ux, uz) = (hx / TEXTURE_METRES, hz / TEXTURE_METRES);
    let corners = [
        (Vec3::new(-hx, 0.0, hz), Vec2::new(-ux, uz)),
        (Vec3::new(hx, 0.0, hz), Vec2::new(ux, uz)),
        (Vec3::new(hx, 0.0, -hz), Vec2::new(ux, -uz)),
        (Vec3::new(-hx, 0.0, -hz), Vec2::new(-ux, -uz)),
    ];
    let vertices = corners
        .into_iter()
        .map(|(position, uv)| MeshVertex::new(position, Vec3::Y, uv))
        .collect();
    // Wound counter-clockwise seen from above, so the surface faces the sky — the same
    // winding the terrain's cells use.
    let indices = vec![0, 1, 3, 1, 2, 3];

    let submesh = SubMesh::new("water", Geometry::Static(vertices), indices, 0);
    ModelAsset::rigid(
        "water",
        vec![submesh],
        vec![surface.material()],
        vec![surface.texture()],
    )
}

/// The material a body gets when nothing else was asked for.
fn water() -> StarterMaterial {
    starter_material("water").unwrap_or(crate::procedural::starter::STARTER_MATERIALS[0])
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    fn body(hx: f32, hz: f32) -> WaterBody {
        WaterBody::new(Vec3::new(3.0, 1.5, -4.0), Vec2::new(hx, hz), 2.0)
    }

    fn positions(model: &ModelAsset) -> Vec<Vec3> {
        let lod = model.lod0().expect("geometry");
        let submesh = lod.submeshes.first().expect("one submesh");
        let Geometry::Static(vertices) = &submesh.geometry else {
            panic!("water is rigid geometry");
        };
        vertices.iter().map(|vertex| vertex.position).collect()
    }

    fn uvs(model: &ModelAsset) -> Vec<Vec2> {
        let lod = model.lod0().expect("geometry");
        let submesh = lod.submeshes.first().expect("one submesh");
        let Geometry::Static(vertices) = &submesh.geometry else {
            panic!("water is rigid geometry");
        };
        vertices.iter().map(|vertex| vertex.uv).collect()
    }

    #[test]
    fn the_quad_reaches_exactly_as_far_as_the_body_does() {
        let model = water_model(&body(9.0, 3.0), "water");
        assert_eq!(model.triangle_count(), 2);
        let corners = positions(&model);
        assert_eq!(corners.len(), 4);
        for corner in &corners {
            assert!((corner.x.abs() - 9.0).abs() < 1e-5, "{corner:?}");
            assert!((corner.z.abs() - 3.0).abs() < 1e-5, "{corner:?}");
            assert!(
                corner.y.abs() < 1e-5,
                "the surface height is on the transform, not in the mesh: {corner:?}"
            );
        }
    }

    #[test]
    fn the_surface_faces_the_sky() {
        let model = water_model(&body(4.0, 4.0), "water");
        let lod = model.lod0().expect("geometry");
        let submesh = lod.submeshes.first().expect("one submesh");
        let Geometry::Static(vertices) = &submesh.geometry else {
            panic!("water is rigid geometry");
        };
        for vertex in vertices {
            assert!((vertex.normal.y - 1.0).abs() < 1e-5, "{:?}", vertex.normal);
        }

        // And the winding agrees with the normal: the cross product of the first triangle's
        // edges points up. A quad wound the other way is drawn only from underneath, which
        // in a level with water is a lake that vanishes when you stand next to it.
        let index = &submesh.indices;
        let (a, b, c) = (
            vertices[index[0] as usize].position,
            vertices[index[1] as usize].position,
            vertices[index[2] as usize].position,
        );
        assert!((b - a).cross(c - a).y > 0.0, "the quad faces down");
    }

    #[test]
    fn the_material_is_transparent() {
        // The whole point of the surface: an opaque water plane still renders, it is just a
        // solid blue slab, and nothing about the geometry says why.
        let model = water_model(&body(4.0, 4.0), "water");
        let material = model.materials.first().expect("one material");
        assert!(
            material.is_transparent(),
            "water drawn in the opaque pass is a solid blue slab"
        );
    }

    #[test]
    fn a_large_body_is_not_a_stretched_small_one() {
        // UVs per world metre, so a pond and a lake have the same texel density. Comparing
        // the span of the UVs against the span of the metres catches the bug where the quad
        // is given 0..1 whatever its size.
        let small = water_model(&body(4.0, 4.0), "water");
        let large = water_model(&body(40.0, 40.0), "water");
        let density = |model: &ModelAsset, metres: f32| {
            let uv = uvs(model);
            let span = uv
                .iter()
                .map(|value| value.x)
                .fold(f32::MIN, f32::max)
                - uv.iter().map(|value| value.x).fold(f32::MAX, f32::min);
            span / metres
        };
        let small_density = density(&small, 8.0);
        let large_density = density(&large, 80.0);
        assert!(
            (small_density - large_density).abs() < 1e-5,
            "{small_density} vs {large_density}"
        );
        assert!((small_density - 1.0 / TEXTURE_METRES).abs() < 1e-5);
    }

    #[test]
    fn a_body_with_no_size_becomes_a_model_with_no_triangles() {
        let mut broken = body(4.0, 4.0);
        broken.depth = 0.0;
        assert_eq!(water_model(&broken, "water").triangle_count(), 0);
    }
}
