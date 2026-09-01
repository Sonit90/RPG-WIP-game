//! The picture and the ground under it come out of the same numbers.
//!
//! ```text
//!  Terrain ──► terrain_model      (engine-assets)     what a player sees
//!         └──► Heightfield        (engine-collision)  what a player stands on
//! ```
//!
//! Neither of those crates can see the other — `engine-assets` has no collision world and
//! `engine-collision` has no meshes — so the one place the two descriptions can be compared
//! is a crate that depends on both. This is it, and this file exists for that single
//! assertion: a terrain whose mesh and whose collision surface split a cell along different
//! diagonals would put a ridge where the picture shows a valley, half a cell wide, over the
//! whole level. Invisible in a screenshot, and impossible to explain from one.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_assets::mesh::Geometry;
use engine_assets::procedural::terrain::terrain_model;
use engine_collision::heightfield::Heightfield;
use engine_core::math::Vec3;
use engine_environment::{BrushMode, Terrain};

/// A terrain with a hill and a hollow in it, so the comparison is over a real surface rather
/// than over a flat one where every split looks the same.
fn sculpted() -> Terrain {
    let mut terrain = Terrain::flat(17, 2.0, 1.0);
    terrain.apply_brush(Vec3::new(-6.0, 0.0, -6.0), 10.0, 4.0, BrushMode::Raise);
    terrain.apply_brush(Vec3::new(8.0, 0.0, 6.0), 8.0, 3.0, BrushMode::Lower);
    terrain
}

fn field_of(terrain: &Terrain) -> Heightfield {
    Heightfield::new(
        terrain.origin,
        terrain.cell_size,
        terrain.resolution,
        terrain.heights.clone(),
    )
    .expect("the terrain describes a surface")
}

#[test]
fn every_grid_point_is_drawn_where_it_is_collided_with() {
    let terrain = sculpted();
    let model = terrain_model(&terrain, "grass");
    let lod = model.lod0().expect("the terrain has geometry");
    let Geometry::Static(vertices) = &lod.submeshes[0].geometry else {
        panic!("terrain is rigid geometry");
    };
    let field = field_of(&terrain);

    for z in 0..terrain.resolution {
        for x in 0..terrain.resolution {
            let drawn = vertices[z * terrain.resolution + x].position;
            let collided = field.point(x, z);
            assert!(
                (drawn - collided).length() < 1e-5,
                "point ({x}, {z}) is drawn at {drawn:?} and collided with at {collided:?}"
            );
        }
    }
}

#[test]
fn every_cell_is_split_along_the_same_diagonal_in_both_descriptions() {
    let terrain = sculpted();
    let model = terrain_model(&terrain, "grass");
    let lod = model.lod0().expect("the terrain has geometry");
    let Geometry::Static(vertices) = &lod.submeshes[0].geometry else {
        panic!("terrain is rigid geometry");
    };
    let indices = &lod.submeshes[0].indices;
    let field = field_of(&terrain);

    let cells = terrain.resolution - 1;
    for z in 0..cells {
        for x in 0..cells {
            let base = (z * cells + x) * 6;
            let drawn: Vec<[Vec3; 3]> = (0..2)
                .map(|triangle| {
                    let start = base + triangle * 3;
                    [
                        vertices[indices[start] as usize].position,
                        vertices[indices[start + 1] as usize].position,
                        vertices[indices[start + 2] as usize].position,
                    ]
                })
                .collect();
            for (drawn, collided) in drawn.iter().zip(field.triangles(x, z).iter()) {
                for (drawn, collided) in drawn.iter().zip(collided.iter()) {
                    assert!(
                        (*drawn - *collided).length() < 1e-5,
                        "cell ({x}, {z}) is drawn and collided with as different triangles"
                    );
                }
            }
        }
    }
}

#[test]
fn the_surface_faces_the_sky_in_both_descriptions() {
    let terrain = sculpted();
    let model = terrain_model(&terrain, "grass");
    let lod = model.lod0().expect("the terrain has geometry");
    let Geometry::Static(vertices) = &lod.submeshes[0].geometry else {
        panic!("terrain is rigid geometry");
    };
    let indices = &lod.submeshes[0].indices;

    // Winding, checked on the drawn triangles: a terrain wound the other way is back-face
    // culled and invisible, which looks exactly like a terrain that failed to upload.
    for triangle in indices.chunks_exact(3) {
        let a = vertices[triangle[0] as usize].position;
        let b = vertices[triangle[1] as usize].position;
        let c = vertices[triangle[2] as usize].position;
        let normal = (b - a).cross(c - a);
        assert!(normal.y > 0.0, "a terrain triangle faces downwards: {normal:?}");
    }
}
