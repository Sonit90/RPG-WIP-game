//! Procedural geometry the renderer does not ship on its own.
//!
//! `MeshData` already generates a cube, a plane and a UV sphere. A cylinder is the third
//! silhouette the showcase needs — round in one axis and flat in another, so a wrong normal
//! or a wrong winding shows up as an obviously broken shape — and it lives here rather than
//! in the renderer because it is a demo shape, not an engine feature.

use engine_core::math::{Vec2, Vec3};
use engine_renderer::error::RendererError;
use engine_renderer::mesh::{Index, MeshData, MeshVertex};

/// A capped cylinder about the Y axis, centred on the origin.
///
/// The side has flat per-face normals pointing away from the axis, the caps have normals
/// along ±Y, and the seam is duplicated rather than shared: a shared seam vertex would have
/// to average two normals a third of a turn apart and would put a visible crease down one
/// side of every cylinder in the scene.
///
/// # Errors
///
/// Returns [`RendererError::InvalidConfig`] if the generated mesh is degenerate, which the
/// clamping below makes impossible in practice — the check exists because `MeshData::new`
/// is the only thing that validates indices, and skipping it would trade a clear error for
/// an out-of-range index the GPU does not bounds-check.
pub fn cylinder(radius: f32, height: f32, segments: u32) -> Result<MeshData, RendererError> {
    let segments = segments.max(3);
    let half = height * 0.5;

    let mut vertices: Vec<MeshVertex> = Vec::with_capacity((segments as usize + 1) * 4 + 2);
    let mut indices: Vec<Index> = Vec::with_capacity(segments as usize * 12);

    // The side: two vertices per angle step, one ring's worth plus a duplicated seam.
    let side_base = vertices.len() as Index;
    for step in 0..=segments {
        let u = step as f32 / segments as f32;
        let (sin, cos) = (u * std::f32::consts::TAU).sin_cos();
        let normal = Vec3::new(cos, 0.0, sin);
        vertices.push(MeshVertex::new(
            Vec3::new(cos * radius, half, sin * radius),
            normal,
            Vec2::new(u, 0.0),
        ));
        vertices.push(MeshVertex::new(
            Vec3::new(cos * radius, -half, sin * radius),
            normal,
            Vec2::new(u, 1.0),
        ));
    }
    for step in 0..segments {
        let top = side_base + step * 2;
        let bottom = top + 1;
        let next_top = top + 2;
        let next_bottom = top + 3;
        // Counter-clockwise seen from outside, which is what the rasterizer calls
        // front-facing. Reversed, the outward faces are culled and the inward ones are
        // drawn — which looks correct until the light hits it.
        indices.extend_from_slice(&[top, next_bottom, bottom, top, next_top, next_bottom]);
    }

    // Both caps: a triangle fan around a centre vertex, with their own normals.
    for (sign, normal) in [(1.0_f32, Vec3::Y), (-1.0_f32, -Vec3::Y)] {
        let center = vertices.len() as Index;
        vertices.push(MeshVertex::new(
            Vec3::new(0.0, half * sign, 0.0),
            normal,
            Vec2::new(0.5, 0.5),
        ));
        let ring = vertices.len() as Index;
        for step in 0..segments {
            let u = step as f32 / segments as f32;
            let (sin, cos) = (u * std::f32::consts::TAU).sin_cos();
            vertices.push(MeshVertex::new(
                Vec3::new(cos * radius, half * sign, sin * radius),
                normal,
                Vec2::new(cos * 0.5 + 0.5, sin * 0.5 + 0.5),
            ));
        }
        for step in 0..segments {
            let current = ring + step;
            let next = ring + (step + 1) % segments;
            // The two caps face opposite ways, so their fans wind opposite ways too.
            if sign > 0.0 {
                indices.extend_from_slice(&[center, next, current]);
            } else {
                indices.extend_from_slice(&[center, current, next]);
            }
        }
    }

    MeshData::new(vertices, indices)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPSILON: f32 = 1e-4;

    fn built(segments: u32) -> MeshData {
        match cylinder(1.0, 2.0, segments) {
            Ok(mesh) => mesh,
            Err(error) => panic!("the generator must produce a valid mesh: {error}"),
        }
    }

    #[test]
    fn every_vertex_sits_on_the_cylinder_surface() {
        let mesh = built(16);
        for vertex in mesh.vertices() {
            assert!(vertex.position.y.abs() <= 1.0 + EPSILON, "{vertex:?}");
            let radial = (vertex.position.x * vertex.position.x
                + vertex.position.z * vertex.position.z)
                .sqrt();
            assert!(radial <= 1.0 + EPSILON, "{vertex:?}");
            assert!((vertex.normal.length() - 1.0).abs() < EPSILON, "{vertex:?}");
        }
    }

    #[test]
    fn side_normals_point_away_from_the_axis_and_cap_normals_along_it() {
        let mesh = built(12);
        for vertex in mesh.vertices() {
            if vertex.normal.y.abs() > EPSILON {
                assert!(
                    (vertex.normal.y.abs() - 1.0).abs() < EPSILON,
                    "a cap normal must be exactly +-Y: {vertex:?}"
                );
            } else {
                // A side normal is the radial direction, so it agrees with the position.
                let radial = Vec3::new(vertex.position.x, 0.0, vertex.position.z).normalize();
                assert!((radial.dot(vertex.normal) - 1.0).abs() < 1e-3, "{vertex:?}");
            }
        }
    }

    #[test]
    fn the_triangle_count_matches_the_segment_count() {
        // Two triangles per side quad plus one per cap wedge.
        for segments in [3_u32, 8, 32] {
            let mesh = built(segments);
            assert_eq!(mesh.triangle_count() as u32, segments * 4);
        }
    }

    #[test]
    fn faces_wind_counter_clockwise_seen_from_outside() {
        // Getting this wrong does not produce a missing cylinder: the outward faces are
        // culled and the inward ones drawn instead, which is only visible once it is lit.
        let mesh = built(24);
        let vertices = mesh.vertices();
        for triangle in mesh.indices().chunks_exact(3) {
            let [a, b, c] = [
                vertices[triangle[0] as usize],
                vertices[triangle[1] as usize],
                vertices[triangle[2] as usize],
            ];
            let face = (b.position - a.position).cross(c.position - a.position);
            let outward = a.normal + b.normal + c.normal;
            assert!(
                face.dot(outward) > 0.0,
                "triangle {triangle:?} faces inward: {face:?} against {outward:?}"
            );
        }
    }

    #[test]
    fn a_degenerate_request_is_clamped_rather_than_rejected() {
        // A generator for test scenes, not an asset importer.
        assert!(cylinder(1.0, 2.0, 0).is_ok());
    }

    #[test]
    fn the_bounds_contain_the_whole_mesh() {
        let mesh = cylinder(1.5, 4.0, 20);
        let Ok(mesh) = mesh else {
            panic!("the generator must produce a valid mesh");
        };
        let bounds = mesh.bounds();
        for vertex in mesh.vertices() {
            let distance = (vertex.position - bounds.center).length();
            assert!(
                distance <= bounds.radius + EPSILON,
                "{vertex:?} is {distance} from the centre of a {}-radius bound",
                bounds.radius
            );
        }
    }
}
