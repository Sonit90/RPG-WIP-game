//! Tangent generation for meshes whose source did not provide one.
//!
//! A tangent is the direction texture-space U runs across a surface. A normal map stores its
//! perturbation in tangent space, so without a tangent frame a normal map cannot be applied
//! at all — which is why this exists now, one milestone before anything samples one: the
//! vertex format has to be frozen before assets are authored against it, and a format is not
//! frozen until the data that fills it is produced.
//!
//! # Why this is written here rather than taken from a crate
//!
//! The reference implementation of tangent-space generation is MikkTSpace, and the Rust
//! binding for it (`mikktspace`) wants the mesh behind a trait with per-face/per-corner
//! accessors, splits vertices when a corner's tangent disagrees, and pulls in a C
//! translation. Splitting vertices is the part that matters: it changes the index buffer,
//! which changes what every other system in this crate has already computed — bounds, LOD
//! correspondence, and the joint indices a skinned vertex carries.
//!
//! What is implemented instead is the ordinary area-weighted accumulation with a
//! Gram-Schmidt orthogonalisation. It does not split vertices, so a hard UV seam that shares
//! a position gets an averaged tangent rather than two correct ones. For stylized art with
//! painted textures that is invisible; for a photoreal normal-mapped hero asset it would not
//! be. When that day comes the answer is to export tangents from the DCC tool, which the
//! importer already prefers over generating them — see `Geometry::has_tangents`.
//!
//! # Determinism
//!
//! Everything here is a fixed-order fold over the index buffer in `f32`. The same mesh
//! produces the same tangents on every run and every machine, which is what lets a processed
//! asset be cached by content hash later.

use engine_core::math::{Vec2, Vec3, Vec4};

use crate::mesh::Geometry;

/// Below this, a UV triangle is treated as degenerate and contributes nothing.
///
/// The determinant of the UV edge matrix is twice the triangle's area in texture space. A
/// triangle whose three corners share a texture coordinate — which is what an untextured
/// procedural mesh looks like — has a determinant of exactly zero, and dividing by it is
/// where the NaNs come from.
const DEGENERATE_UV_AREA: f32 = 1e-12;

/// Below this squared length, an accumulated tangent is treated as having no direction.
const DEGENERATE_TANGENT: f32 = 1e-12;

/// Fills in every vertex's tangent from positions, normals, UVs and the index buffer.
///
/// Overwrites whatever was there: the caller has already decided the existing tangents are
/// unusable. Vertices no triangle references, and vertices whose every triangle is
/// UV-degenerate, get [`fallback`] — a deterministic frame built from the normal alone.
///
/// The result satisfies, for every vertex with a finite normal:
///
/// ```text
/// dot(normal, tangent.xyz) ~= 0        (orthogonalised)
/// length(tangent.xyz)      ~= 1
/// tangent.w                 = +1 or -1
/// ```
///
/// and contains no NaN or infinity for any input, degenerate ones included.
pub fn generate(geometry: &mut Geometry, indices: &[u32]) {
    let count = geometry.len();
    if count == 0 {
        return;
    }

    let positions: Vec<Vec3> = geometry.positions().collect();
    let normals: Vec<Vec3> = geometry.normals().collect();
    let uvs: Vec<Vec2> = geometry.uvs().collect();

    // Accumulated per-vertex tangent and bitangent. The bitangent is kept only to recover
    // the handedness sign at the end; it is not stored on the vertex.
    let mut tangents = vec![Vec3::ZERO; count];
    let mut bitangents = vec![Vec3::ZERO; count];

    for triangle in indices.chunks_exact(3) {
        let [a, b, c] = [
            triangle[0] as usize,
            triangle[1] as usize,
            triangle[2] as usize,
        ];
        // An index buffer is validated elsewhere, but this runs *before* validation on the
        // import path, so it cannot assume the indices are in range.
        if a >= count || b >= count || c >= count {
            continue;
        }

        let edge1 = positions[b] - positions[a];
        let edge2 = positions[c] - positions[a];
        let delta1 = uvs[b] - uvs[a];
        let delta2 = uvs[c] - uvs[a];

        let determinant = delta1.x * delta2.y - delta2.x * delta1.y;
        if !determinant.is_finite() || determinant.abs() <= DEGENERATE_UV_AREA {
            // A degenerate UV triangle carries no texture-space direction. Skipping it is
            // the whole reason this function cannot produce a NaN: the reciprocal below is
            // the only division, and this is the only way it could be by zero.
            continue;
        }
        let reciprocal = 1.0 / determinant;

        // Deliberately *not* normalised before accumulation: the unnormalised vectors are
        // proportional to triangle area, so a large triangle contributes more than a sliver.
        // That area weighting is what stops one degenerate-ish triangle from dominating a
        // vertex shared by twenty good ones.
        let tangent = (edge1 * delta2.y - edge2 * delta1.y) * reciprocal;
        let bitangent = (edge2 * delta1.x - edge1 * delta2.x) * reciprocal;
        if !is_finite(tangent) || !is_finite(bitangent) {
            continue;
        }

        for vertex in [a, b, c] {
            tangents[vertex] += tangent;
            bitangents[vertex] += bitangent;
        }
    }

    let resolved: Vec<Vec4> = (0..count)
        .map(|vertex| resolve(normals[vertex], tangents[vertex], bitangents[vertex]))
        .collect();
    geometry.set_tangents(&resolved);
}

/// One vertex's accumulated frame turned into a stored tangent.
fn resolve(normal: Vec3, accumulated: Vec3, bitangent: Vec3) -> Vec4 {
    let normal = if is_finite(normal) && normal.length_squared() > DEGENERATE_TANGENT {
        normal.normalize()
    } else {
        // A vertex with no usable normal cannot have a meaningful tangent frame either.
        // +Y is the same stand-in the glTF importer uses for a mesh with no NORMAL accessor.
        Vec3::Y
    };

    if !is_finite(accumulated) || accumulated.length_squared() <= DEGENERATE_TANGENT {
        return fallback(normal);
    }

    // Gram-Schmidt: remove the part of the tangent that runs along the normal, so the frame
    // is orthogonal. Without it a tangent on a curved surface leans out of the surface and
    // the reconstructed bitangent leans with it.
    let orthogonal = accumulated - normal * normal.dot(accumulated);
    if !is_finite(orthogonal) || orthogonal.length_squared() <= DEGENERATE_TANGENT {
        // The accumulated tangent was parallel to the normal — geometrically impossible for
        // a real surface, and reachable when normals and UVs disagree about what the surface
        // is. There is nothing to orthogonalise, so take the deterministic frame.
        return fallback(normal);
    }
    let tangent = orthogonal.normalize();

    // Handedness. `cross(normal, tangent)` is the bitangent the shader will reconstruct; if
    // it points the opposite way from the one the UVs actually implied, the sign is -1 and
    // the shader flips it back. This is what makes a mirrored UV island shade correctly.
    let handedness = if normal.cross(tangent).dot(bitangent) < 0.0 {
        -1.0
    } else {
        1.0
    };
    Vec4::new(tangent.x, tangent.y, tangent.z, handedness)
}

/// A deterministic tangent for a vertex that has no usable one.
///
/// Any unit vector perpendicular to the normal is a valid tangent frame when there is no UV
/// direction to honour; what matters is that the *same* normal always produces the *same*
/// tangent, so an asset imported twice is byte-identical. Crossing with whichever cardinal
/// axis the normal is least aligned with is stable and never degenerate: the two candidates
/// cannot both be parallel to the normal.
#[must_use]
pub fn fallback(normal: Vec3) -> Vec4 {
    let normal = if is_finite(normal) && normal.length_squared() > DEGENERATE_TANGENT {
        normal.normalize()
    } else {
        Vec3::Y
    };
    let axis = if normal.x.abs() < 0.9 {
        Vec3::X
    } else {
        Vec3::Y
    };
    let tangent = axis.cross(normal);
    let tangent = if tangent.length_squared() > DEGENERATE_TANGENT {
        tangent.normalize()
    } else {
        // Unreachable given the axis choice above, and cheap to be sure about: a NaN here
        // would travel all the way into a vertex buffer.
        Vec3::X
    };
    Vec4::new(tangent.x, tangent.y, tangent.z, 1.0)
}

/// Whether every component is a real number.
fn is_finite(value: Vec3) -> bool {
    value.x.is_finite() && value.y.is_finite() && value.z.is_finite()
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::mesh::{MeshVertex, SkinVertex};

    /// A unit quad in the XY plane, facing +Z, with a conventional UV layout.
    fn quad() -> (Geometry, Vec<u32>) {
        let vertices = vec![
            MeshVertex::new(Vec3::new(0.0, 0.0, 0.0), Vec3::Z, Vec2::new(0.0, 0.0)),
            MeshVertex::new(Vec3::new(1.0, 0.0, 0.0), Vec3::Z, Vec2::new(1.0, 0.0)),
            MeshVertex::new(Vec3::new(1.0, 1.0, 0.0), Vec3::Z, Vec2::new(1.0, 1.0)),
            MeshVertex::new(Vec3::new(0.0, 1.0, 0.0), Vec3::Z, Vec2::new(0.0, 1.0)),
        ];
        (Geometry::Static(vertices), vec![0, 1, 2, 0, 2, 3])
    }

    fn assert_usable(geometry: &Geometry) {
        for (index, (tangent, normal)) in geometry.tangents().zip(geometry.normals()).enumerate() {
            assert!(
                tangent.x.is_finite()
                    && tangent.y.is_finite()
                    && tangent.z.is_finite()
                    && tangent.w.is_finite(),
                "vertex {index} tangent is not finite: {tangent:?}"
            );
            let length =
                (tangent.x * tangent.x + tangent.y * tangent.y + tangent.z * tangent.z).sqrt();
            assert!(
                (length - 1.0).abs() < 1e-4,
                "vertex {index} tangent is not unit: {length}"
            );
            assert!(
                (tangent.w.abs() - 1.0).abs() < 1e-6,
                "vertex {index} handedness is {}",
                tangent.w
            );
            let dot = normal.x * tangent.x + normal.y * tangent.y + normal.z * tangent.z;
            assert!(
                dot.abs() < 1e-4,
                "vertex {index} tangent is not orthogonal to its normal: dot = {dot}"
            );
        }
    }

    #[test]
    fn a_flat_quad_gets_the_tangent_its_uvs_imply() {
        // U runs along +X on this quad, so the tangent must too. This is the test that
        // catches a transposed edge/delta matrix, which produces a perfectly orthogonal,
        // perfectly unit, perfectly wrong tangent.
        let (mut geometry, indices) = quad();
        generate(&mut geometry, &indices);
        assert_usable(&geometry);

        for tangent in geometry.tangents() {
            assert!(
                (tangent.x - 1.0).abs() < 1e-4,
                "tangent should run along +X: {tangent:?}"
            );
            assert!(tangent.y.abs() < 1e-4 && tangent.z.abs() < 1e-4);
            assert!(tangent.w > 0.0, "an unmirrored quad is right-handed");
        }
    }

    #[test]
    fn mirrored_uvs_produce_the_opposite_handedness() {
        // The whole reason W exists. Flipping V reverses the bitangent, and a shader that
        // reconstructed it without the sign would light the mirrored half inside out.
        let (mut geometry, indices) = quad();
        if let Geometry::Static(vertices) = &mut geometry {
            for vertex in vertices.iter_mut() {
                vertex.uv = Vec2::new(vertex.uv.x, -vertex.uv.y);
            }
        }
        generate(&mut geometry, &indices);
        assert_usable(&geometry);
        for tangent in geometry.tangents() {
            assert!(tangent.w < 0.0, "mirrored UVs should be left-handed");
        }
    }

    #[test]
    fn degenerate_uvs_produce_a_finite_deterministic_fallback() {
        // Every corner shares one texture coordinate, so the UV determinant is exactly zero.
        // This is not a corner case — it is what every untextured procedural mesh in this
        // crate looks like.
        let vertices = vec![
            MeshVertex::new(Vec3::ZERO, Vec3::Y, Vec2::ZERO),
            MeshVertex::new(Vec3::X, Vec3::Y, Vec2::ZERO),
            MeshVertex::new(Vec3::Z, Vec3::Y, Vec2::ZERO),
        ];
        let mut geometry = Geometry::Static(vertices);
        generate(&mut geometry, &[0, 1, 2]);
        assert_usable(&geometry);

        // And it is the same answer every time.
        let first: Vec<Vec4> = geometry.tangents().collect();
        let mut again = Geometry::Static(vec![
            MeshVertex::new(Vec3::ZERO, Vec3::Y, Vec2::ZERO),
            MeshVertex::new(Vec3::X, Vec3::Y, Vec2::ZERO),
            MeshVertex::new(Vec3::Z, Vec3::Y, Vec2::ZERO),
        ]);
        generate(&mut again, &[0, 1, 2]);
        assert_eq!(first, again.tangents().collect::<Vec<_>>());
    }

    #[test]
    fn a_vertex_no_triangle_references_still_gets_a_frame() {
        let (mut geometry, _) = quad();
        // Only the first triangle: vertex 3 is orphaned.
        generate(&mut geometry, &[0, 1, 2]);
        assert_usable(&geometry);
    }

    #[test]
    fn nothing_here_can_produce_a_nan() {
        // Non-finite positions, normals and UVs at once — the worst an exporter could do.
        let vertices = vec![
            MeshVertex::new(
                Vec3::new(f32::NAN, 0.0, 0.0),
                Vec3::new(f32::INFINITY, 0.0, 0.0),
                Vec2::new(f32::NAN, 0.0),
            ),
            MeshVertex::new(Vec3::X, Vec3::ZERO, Vec2::new(1.0, 0.0)),
            MeshVertex::new(Vec3::Y, Vec3::Z, Vec2::new(0.0, 1.0)),
        ];
        let mut geometry = Geometry::Static(vertices);
        generate(&mut geometry, &[0, 1, 2]);
        for tangent in geometry.tangents() {
            assert!(tangent.x.is_finite() && tangent.y.is_finite());
            assert!(tangent.z.is_finite() && tangent.w.is_finite());
        }
    }

    #[test]
    fn out_of_range_indices_are_skipped_rather_than_panicking() {
        // Generation runs before validation on the import path, so it meets unvalidated
        // index buffers by construction.
        let (mut geometry, _) = quad();
        generate(&mut geometry, &[0, 1, 99, 0, 1, 2]);
        assert_usable(&geometry);
    }

    #[test]
    fn skinned_geometry_is_handled_the_same_way() {
        let vertices = vec![
            SkinVertex::rigid(Vec3::ZERO, Vec3::Z, Vec2::new(0.0, 0.0), 0),
            SkinVertex::rigid(Vec3::X, Vec3::Z, Vec2::new(1.0, 0.0), 0),
            SkinVertex::rigid(Vec3::Y, Vec3::Z, Vec2::new(0.0, 1.0), 0),
        ];
        let mut geometry = Geometry::Skinned(vertices);
        generate(&mut geometry, &[0, 1, 2]);
        assert_usable(&geometry);
    }

    #[test]
    fn every_procedural_asset_leaves_its_constructor_with_usable_tangents() {
        // The invariant this module exists to establish, checked against the geometry the
        // engine actually ships rather than against a quad written for the test: `SubMesh`
        // is the one door every producer in this crate walks through, so a mesh that reached
        // a vertex buffer without a tangent would have to have skipped it.
        //
        // Both layouts are covered — the environment library is rigid, the fire mage is
        // skinned — because the two take different arms of every match in this file.
        let mut checked = 0;
        let mut models: Vec<crate::model::ModelAsset> = crate::procedural::environment_library()
            .into_iter()
            .map(|(_, model)| model)
            .collect();
        let Ok(mage) = crate::procedural::fire_mage() else {
            panic!("the fire mage must build");
        };
        models.push(mage);

        for model in &models {
            for lod in &model.lods {
                for submesh in &lod.submeshes {
                    assert!(
                        submesh.geometry.has_tangents(),
                        "{}/{} left the constructor without usable tangents",
                        model.name,
                        submesh.name
                    );
                    // And the colour every procedural author leaves alone is the identity,
                    // not the derived transparent black — a mesh tinted by zero is invisible
                    // the moment anything multiplies by it.
                    assert!(
                        submesh
                            .geometry
                            .colors()
                            .all(|color| color == crate::mesh::DEFAULT_VERTEX_COLOR),
                        "{}/{} has a vertex colour that is not opaque white",
                        model.name,
                        submesh.name
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked > 0, "the libraries must have produced geometry");
    }

    #[test]
    fn an_empty_geometry_is_left_alone() {
        let mut geometry = Geometry::Static(Vec::new());
        generate(&mut geometry, &[]);
        assert_eq!(geometry.len(), 0);
    }

    #[test]
    fn the_fallback_is_perpendicular_for_every_axis() {
        for normal in [Vec3::X, Vec3::Y, Vec3::Z, -Vec3::X, -Vec3::Y, -Vec3::Z] {
            let tangent = fallback(normal);
            let dot = normal.x * tangent.x + normal.y * tangent.y + normal.z * tangent.z;
            assert!(dot.abs() < 1e-6, "{normal:?} -> {tangent:?}");
            assert_eq!(tangent.w, 1.0);
        }
    }
}
