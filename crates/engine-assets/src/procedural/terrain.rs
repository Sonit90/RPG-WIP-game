//! The terrain's mesh: the heightmap, as triangles.
//!
//! ```text
//!  Terrain (heights) ──► terrain_model ──► ModelAsset ──► one render entity
//!                   └──► Heightfield   ──► one collision shape
//! ```
//!
//! Both come from the same numbers and split every cell along the same diagonal, so what a
//! player sees and what they stand on are the same surface. That is the only thing this
//! module has to get right, and it is why the split is stated in both places rather than
//! being whatever each side happened to do.
//!
//! # One material, tiled
//!
//! A blockout terrain is one surface. Texturing it by slope or by height is a shading
//! decision that belongs to a real art pass; here the UV is the world position over
//! [`TEXTURE_METRES`], so the texel density is the same on a plain and on a hillside and an
//! author can read the scale of what they are building off the ground.
//!
//! # Cost model
//!
//! Rebuilt when the heights change — once per brush stroke, in the editor, where nothing else
//! is running. A 129×129 terrain is 32 768 triangles and about two megabytes of vertices; a
//! 65×65 one is a quarter of that and is what a blockout actually needs.

use engine_core::math::Vec2;
use engine_environment::Terrain;

use crate::mesh::{Geometry, MeshVertex, SubMesh};
use crate::model::ModelAsset;
use crate::procedural::starter::{StarterMaterial, starter_material};

/// How many metres of ground one tile of the texture covers.
pub const TEXTURE_METRES: f32 = 8.0;

/// The model for a terrain, textured with the named starter material.
///
/// The material is named rather than passed so that a level's ground and the browser's
/// `materials/` folder cannot drift apart: `"grass"` here is the same `grass` an author can
/// open and edit.
///
/// # Panics
///
/// Never. A material the starter library does not have falls back to grass, and a terrain
/// that is not a surface produces an empty model rather than an indexing failure — the caller
/// sees a model with no triangles, which is what a level with no terrain looks like anyway.
#[must_use]
pub fn terrain_model(terrain: &Terrain, material: &str) -> ModelAsset {
    let surface = starter_material(material).unwrap_or_else(grass);
    if !terrain.is_valid() {
        return ModelAsset::rigid(
            "terrain",
            Vec::new(),
            vec![surface.material()],
            vec![surface.texture()],
        );
    }

    let resolution = terrain.resolution;
    let mut vertices = Vec::with_capacity(resolution * resolution);
    for z in 0..resolution {
        for x in 0..resolution {
            let position = terrain.position_at_point(x, z);
            // The normal from the heights around the point rather than from the two triangles
            // that meet at it: a per-face normal makes a smooth hill look like a folded map.
            let normal = terrain.normal_at(position);
            let uv = Vec2::new(
                position.x / TEXTURE_METRES,
                position.z / TEXTURE_METRES,
            );
            vertices.push(MeshVertex::new(position, normal, uv));
        }
    }

    let mut indices = Vec::with_capacity((resolution - 1) * (resolution - 1) * 6);
    for z in 0..resolution - 1 {
        for x in 0..resolution - 1 {
            let a = (z * resolution + x) as u32;
            let b = a + 1;
            let c = a + resolution as u32;
            let d = c + 1;
            // The same diagonal `Heightfield::triangles` splits on, wound counter-clockwise
            // seen from above so the surface faces the sky.
            indices.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }

    let submesh = SubMesh::new("terrain", Geometry::Static(vertices), indices, 0);
    ModelAsset::rigid(
        "terrain",
        vec![submesh],
        vec![surface.material()],
        vec![surface.texture()],
    )
}

/// The material a terrain gets when nothing else was asked for.
fn grass() -> StarterMaterial {
    starter_material("grass").unwrap_or(crate::procedural::starter::STARTER_MATERIALS[0])
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_core::math::Vec3;
    use engine_environment::BrushMode;

    #[test]
    fn a_flat_terrain_becomes_a_flat_grid_of_triangles() {
        let terrain = Terrain::flat(9, 4.0, 0.0);
        let model = terrain_model(&terrain, "grass");
        let lod = model.lod0().expect("geometry");
        assert_eq!(lod.submeshes.len(), 1);
        assert_eq!(
            model.triangle_count(),
            (9 - 1) * (9 - 1) * 2,
            "two triangles per cell"
        );
        let Geometry::Static(vertices) = &lod.submeshes[0].geometry else {
            panic!("terrain is rigid geometry");
        };
        assert_eq!(vertices.len(), 9 * 9);
        for vertex in vertices {
            assert!(vertex.normal.y > 0.99, "a flat terrain has a leaning normal");
            assert_eq!(vertex.position.y, 0.0);
        }
    }

    #[test]
    fn a_hill_leans_its_normals_away_from_the_peak() {
        let mut terrain = Terrain::flat(17, 2.0, 0.0);
        terrain.apply_brush(Vec3::ZERO, 12.0, 6.0, BrushMode::Raise);
        let model = terrain_model(&terrain, "grass");
        let Geometry::Static(vertices) = &model.lod0().expect("geometry").submeshes[0].geometry
        else {
            panic!("rigid");
        };
        let leaning = vertices
            .iter()
            .filter(|vertex| vertex.normal.y < 0.99)
            .count();
        assert!(leaning > 20, "a hill with {leaning} sloping vertices");
        for vertex in vertices {
            assert!(vertex.normal.y > 0.0, "a normal pointing into the ground");
            assert!((vertex.normal.length() - 1.0).abs() < 1e-3);
        }
    }

    #[test]
    fn the_texture_is_tiled_by_world_size_rather_than_stretched_over_the_terrain() {
        let small = terrain_model(&Terrain::flat(5, 4.0, 0.0), "grass");
        let large = terrain_model(&Terrain::flat(33, 4.0, 0.0), "grass");
        let uv_of = |model: &ModelAsset, index: usize| {
            let Geometry::Static(vertices) = &model.lod0().expect("geometry").submeshes[0].geometry
            else {
                panic!("rigid");
            };
            vertices[index].uv
        };
        // The point one cell along from the corner is the same distance in metres in both, so
        // it must carry the same texture coordinate — a stretched UV would scale with the
        // terrain and the grass would get bigger as the level did.
        let step = uv_of(&small, 1) - uv_of(&small, 0);
        let same = uv_of(&large, 1) - uv_of(&large, 0);
        assert!((step - same).length() < 1e-6, "{step:?} vs {same:?}");
        assert!((step.x - 4.0 / TEXTURE_METRES).abs() < 1e-6);
    }

    #[test]
    fn a_malformed_terrain_makes_an_empty_model_rather_than_a_panic() {
        let broken = Terrain {
            origin: Vec3::ZERO,
            cell_size: 2.0,
            resolution: 9,
            heights: vec![0.0; 4],
        };
        assert!(!broken.is_valid());
        let model = terrain_model(&broken, "grass");
        assert_eq!(model.triangle_count(), 0);
    }

    #[test]
    fn an_unknown_material_falls_back_to_grass_rather_than_to_nothing() {
        let model = terrain_model(&Terrain::flat(5, 4.0, 0.0), "not_a_material");
        assert_eq!(model.materials.len(), 1);
        assert_eq!(model.textures.len(), 1);
        assert_eq!(model.materials[0].name, "grass");
    }
}
