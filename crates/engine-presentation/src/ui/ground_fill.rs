//! The interior of an area indicator: a triangle mesh laid onto the terrain.
//!
//! [`ground_decal`](super::ground_decal) draws the *outline* of a footprint as a strip of
//! instanced quads. That is right for a border and wrong for a fill: a telegraph that says
//! "this much of the cast has elapsed" has to cover an area that changes size every frame,
//! and a fill built from instanced pieces would either leave seams or need thousands of
//! them. One mesh, rebuilt per indicator and uploaded into a dynamic buffer, is the cheaper
//! and seamless answer.
//!
//! # Why a grid and not a fan
//!
//! A single triangle fan from the centre is flat across its whole face, so on a slope it
//! buries one side and floats the other - the artefact [`ground_decal`] exists to avoid.
//! The fill is therefore a *grid*: rings and spokes for the round shapes, rows and columns
//! for the straight ones, every vertex placed at the height of the ground under it. The
//! cell size is [`SEGMENT_LENGTH`], the same budget the outline is cut at, so the fill
//! follows the surface exactly as closely as the border does.
//!
//! # Reach
//!
//! The fill covers a fraction (`reach`) of the shape measured outward from where it is
//! anchored: a disc grows from its centre, a cone from its apex, a line from its start. At
//! reach 1 it is the whole footprint, which is what the strike will hit.
//!
//! The *angular* resolution of a round shape is derived from the full shape, not from the
//! current reach. If it followed reach, the number of spokes would step up as the fill grew
//! and the edge would visibly pop from one polygon to a finer one. Rings may follow reach,
//! since a ring appearing at the rim is invisible.
//!
//! # Bounded
//!
//! Output never exceeds [`MAX_FILL_VERTICES`] / [`MAX_FILL_INDICES`], so a consumer sizes its
//! GPU buffers once. A shape too large for the budget gets a coarser grid rather than a
//! truncated one - a missing stretch of fill would read as "the attack does not reach here".

use engine_core::math::Vec3;

use super::ground_decal::{GroundSurface, SEGMENT_LENGTH, local_to_world};
use super::targeting::TargetShape;

/// The most vertices one fill is ever built with.
pub const MAX_FILL_VERTICES: usize = 2048;

/// The most indices one fill is ever built with.
///
/// Every grid cell is two triangles and a grid has at most as many cells as vertices, so six
/// indices per vertex is a worst case that cannot be exceeded.
pub const MAX_FILL_INDICES: usize = MAX_FILL_VERTICES * 6;

/// Below this reach nothing is drawn: a sliver at the anchor is not worth a draw call.
const MIN_REACH: f32 = 0.001;

/// The fewest spokes a full circle is cut into, so a small disc still reads as round.
const MIN_CIRCLE_SPOKES: usize = 24;

/// The most spokes a round shape is cut into.
const MAX_SPOKES: usize = 128;

/// The fewest spokes a cone is cut into.
const MIN_CONE_SPOKES: usize = 4;

/// How much of a cell is forgiven when counting cells, so a span of exactly one step is one
/// cell and not two after floating-point rounding.
const CEIL_SLACK: f32 = 1e-3;

/// A fill's geometry, reused from frame to frame.
///
/// Positions are relative to the anchor the fill was built around, so the consumer places the
/// mesh by translating it to the anchor and the same mesh stays valid when the anchor moves
/// over flat ground.
#[derive(Debug, Default, Clone)]
pub struct FillMesh {
    /// Vertex positions, relative to the anchor.
    pub positions: Vec<Vec3>,
    /// Triangle list indices into [`positions`](Self::positions). Counter-clockwise seen
    /// from above.
    pub indices: Vec<u32>,
}

impl FillMesh {
    /// An empty mesh.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Empties the mesh, keeping its capacity.
    pub fn clear(&mut self) {
        self.positions.clear();
        self.indices.clear();
    }
}

/// Fills `out` with a mesh covering `reach` (0..=1) of `shape`, laid on `ground`.
///
/// `anchor` is where the shape sits (a circle's centre, a cone's apex, a line's start, a
/// rectangle's centre), `facing` is its yaw about +Y with zero facing -Z, and `lift` raises
/// every vertex above the ground to avoid a depth tie. Positions are written relative to
/// `anchor`. `out` is cleared first and does not reallocate once it has grown.
pub fn build_fill(
    shape: TargetShape,
    anchor: Vec3,
    facing: f32,
    reach: f32,
    lift: f32,
    ground: &GroundSurface<'_>,
    out: &mut FillMesh,
) {
    out.clear();
    let reach = if reach.is_finite() {
        reach.clamp(0.0, 1.0)
    } else {
        0.0
    };
    if reach <= MIN_REACH {
        return;
    }

    let mut place = |out: &mut FillMesh, x: f32, z: f32| {
        let (world_x, world_z) = local_to_world(anchor, facing, x, z);
        out.positions.push(Vec3::new(
            world_x - anchor.x,
            ground.height_at(world_x, world_z) + lift - anchor.y,
            world_z - anchor.z,
        ));
    };

    match shape {
        TargetShape::Circle { radius } => {
            if radius <= 0.0 {
                return;
            }
            let spokes = spokes_for(std::f32::consts::TAU * radius, MIN_CIRCLE_SPOKES);
            polar(
                out,
                &mut place,
                0.0,
                std::f32::consts::TAU,
                spokes,
                true,
                radius * reach,
            );
        }
        TargetShape::Cone { range, half_angle } => {
            if range <= 0.0 {
                return;
            }
            let half_angle = half_angle.clamp(0.01, std::f32::consts::PI - 0.01);
            let spokes = spokes_for(2.0 * half_angle * range, MIN_CONE_SPOKES);
            polar(
                out,
                &mut place,
                -half_angle,
                2.0 * half_angle,
                spokes,
                false,
                range * reach,
            );
        }
        TargetShape::Line { width, length } => {
            if width <= 0.0 || length <= 0.0 {
                return;
            }
            // Columns from the full width so the sides do not re-tessellate as it grows.
            let columns = cells(width);
            let rows = cells(length * reach);
            grid(
                out,
                &mut place,
                (-width * 0.5, width * 0.5),
                (0.0, -length * reach),
                columns,
                rows,
            );
        }
        TargetShape::Rectangle { width, length } => {
            if width <= 0.0 || length <= 0.0 {
                return;
            }
            let (half_width, half_length) = (width * 0.5 * reach, length * 0.5 * reach);
            grid(
                out,
                &mut place,
                (-half_width, half_width),
                (half_length, -half_length),
                cells(half_width * 2.0),
                cells(half_length * 2.0),
            );
        }
    }
}

/// How many cells an edge of `span` metres is cut into.
fn cells(span: f32) -> usize {
    ((span / SEGMENT_LENGTH) - CEIL_SLACK).ceil().max(1.0) as usize
}

/// How many spokes a sweep with an outer arc of `arc` metres gets, so the outermost
/// segment is no longer than [`SEGMENT_LENGTH`].
fn spokes_for(arc: f32, minimum: usize) -> usize {
    ((arc / SEGMENT_LENGTH).ceil() as usize).clamp(minimum, MAX_SPOKES)
}

/// A polar grid: a centre vertex and concentric rings, swept from `start` over `span` radians.
///
/// Angle zero points along local -Z, positive angles towards +X. A `closed` sweep (a full
/// circle) shares its first and last spoke so there is no seam; an open one (a cone) has one
/// more vertex per ring than it has segments.
fn polar(
    out: &mut FillMesh,
    place: &mut impl FnMut(&mut FillMesh, f32, f32),
    start: f32,
    span: f32,
    spokes: usize,
    closed: bool,
    radius: f32,
) {
    let per_ring = if closed { spokes } else { spokes + 1 };
    // Coarsen the rings, never the spokes, when the budget is tight: the spokes decide how
    // round the edge is, and they are already capped at MAX_SPOKES.
    let max_rings = ((MAX_FILL_VERTICES - 1) / per_ring).max(1);
    let rings = (((radius / SEGMENT_LENGTH) - CEIL_SLACK).ceil().max(1.0) as usize).min(max_rings);

    place(out, 0.0, 0.0);
    for ring in 1..=rings {
        let ring_radius = radius * ring as f32 / rings as f32;
        for spoke in 0..per_ring {
            let angle = start + span * spoke as f32 / spokes as f32;
            let (sin, cos) = angle.sin_cos();
            place(out, sin * ring_radius, -(cos * ring_radius));
        }
    }

    // Index of the vertex on `ring` (1-based) at `spoke`, wrapping a closed sweep.
    let at = |ring: usize, spoke: usize| -> u32 {
        (1 + (ring - 1) * per_ring + spoke % per_ring) as u32
    };
    for spoke in 0..spokes {
        // The centre fan. Later spoke first: the sweep runs from -Z towards +X, which is
        // clockwise seen from above, so the order is reversed to face upwards.
        out.indices.extend([0, at(1, spoke + 1), at(1, spoke)]);
    }
    for ring in 1..rings {
        for spoke in 0..spokes {
            let (inner0, inner1) = (at(ring, spoke), at(ring, spoke + 1));
            let (outer0, outer1) = (at(ring + 1, spoke), at(ring + 1, spoke + 1));
            out.indices.extend([inner0, outer1, outer0]);
            out.indices.extend([inner0, inner1, outer1]);
        }
    }
}

/// A rectangular grid from `x.0` to `x.1` across and `z.0` to `z.1` along, in local space.
///
/// `z.1` must be less than `z.0` (the grid runs towards -Z), which is what fixes the
/// winding. The cell counts are reduced until the grid fits [`MAX_FILL_VERTICES`].
fn grid(
    out: &mut FillMesh,
    place: &mut impl FnMut(&mut FillMesh, f32, f32),
    x: (f32, f32),
    z: (f32, f32),
    mut columns: usize,
    mut rows: usize,
) {
    if (columns + 1) * (rows + 1) > MAX_FILL_VERTICES {
        let scale = (MAX_FILL_VERTICES as f32 / ((columns + 1) * (rows + 1)) as f32).sqrt();
        columns = ((columns as f32 * scale) as usize).max(1);
        rows = ((rows as f32 * scale) as usize).max(1);
        // The scaling lands within a few cells of the budget; trim whichever is larger.
        while (columns + 1) * (rows + 1) > MAX_FILL_VERTICES {
            if columns >= rows && columns > 1 {
                columns -= 1;
            } else if rows > 1 {
                rows -= 1;
            } else {
                break;
            }
        }
    }

    for row in 0..=rows {
        let local_z = z.0 + (z.1 - z.0) * row as f32 / rows as f32;
        for column in 0..=columns {
            let local_x = x.0 + (x.1 - x.0) * column as f32 / columns as f32;
            place(out, local_x, local_z);
        }
    }
    let stride = columns + 1;
    for row in 0..rows {
        for column in 0..columns {
            let a = (row * stride + column) as u32;
            let b = a + 1;
            let c = a + stride as u32;
            let d = c + 1;
            out.indices.extend([a, b, c]);
            out.indices.extend([b, d, c]);
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_environment::terrain::Terrain;

    const LIFT: f32 = 0.04;

    fn terrain_from(height: impl Fn(f32, f32) -> f32) -> Terrain {
        let mut terrain = Terrain::flat(129, 1.0, 0.0);
        for z in 0..terrain.resolution {
            for x in 0..terrain.resolution {
                let world_x = terrain.origin.x + x as f32 * terrain.cell_size;
                let world_z = terrain.origin.z + z as f32 * terrain.cell_size;
                terrain.heights[z * terrain.resolution + x] = height(world_x, world_z);
            }
        }
        terrain
    }

    fn build(
        shape: TargetShape,
        anchor: Vec3,
        facing: f32,
        reach: f32,
        ground: &GroundSurface<'_>,
    ) -> FillMesh {
        let mut mesh = FillMesh::new();
        build_fill(shape, anchor, facing, reach, LIFT, ground, &mut mesh);
        mesh
    }

    fn all_shapes() -> [TargetShape; 4] {
        [
            TargetShape::Circle { radius: 6.0 },
            TargetShape::Cone {
                range: 9.0,
                half_angle: 0.6,
            },
            TargetShape::Line {
                width: 3.0,
                length: 14.0,
            },
            TargetShape::Rectangle {
                width: 6.0,
                length: 10.0,
            },
        ]
    }

    fn horizontal(point: Vec3) -> f32 {
        (point.x * point.x + point.z * point.z).sqrt()
    }

    #[test]
    fn flat_ground_puts_every_vertex_at_the_plane_plus_lift() {
        let ground = GroundSurface::Plane(2.0);
        let anchor = Vec3::new(3.0, 1.5, -4.0);
        for shape in all_shapes() {
            let mesh = build(shape, anchor, 0.7, 1.0, &ground);
            assert!(!mesh.positions.is_empty(), "{shape:?} built nothing");
            for point in &mesh.positions {
                assert!(
                    (point.y - (2.0 + LIFT - anchor.y)).abs() < 1e-4,
                    "{shape:?}: vertex at {point:?}",
                );
            }
        }
    }

    #[test]
    fn sloped_ground_is_followed_within_a_few_centimetres() {
        let terrain = terrain_from(|x, z| x * 0.6 + (z * 0.5).sin());
        let ground = GroundSurface::Terrain(&terrain);
        let anchor = Vec3::new(2.0, 0.0, 3.0);
        for shape in all_shapes() {
            let mesh = build(shape, anchor, 0.4, 1.0, &ground);
            for point in &mesh.positions {
                let world = *point + anchor;
                let under = ground.height_at(world.x, world.z);
                assert!(
                    (world.y - under - LIFT).abs() < 0.05,
                    "{shape:?}: vertex {} m off the ground",
                    world.y - under - LIFT,
                );
            }
        }
    }

    #[test]
    fn every_triangle_faces_up() {
        let terrain = terrain_from(|x, z| x * 0.05 + z * 0.03);
        let ground = GroundSurface::Terrain(&terrain);
        for shape in all_shapes() {
            for facing in [0.0, 1.1, -2.5] {
                for reach in [0.2, 0.5, 1.0] {
                    let mesh = build(shape, Vec3::ZERO, facing, reach, &ground);
                    assert!(!mesh.indices.is_empty());
                    assert_eq!(mesh.indices.len() % 3, 0);
                    for triangle in mesh.indices.chunks_exact(3) {
                        let a = mesh.positions[triangle[0] as usize];
                        let b = mesh.positions[triangle[1] as usize];
                        let c = mesh.positions[triangle[2] as usize];
                        // (b-a) x (c-a), Y component = uz*vx - ux*vz.
                        let cross_y = (b.z - a.z) * (c.x - a.x) - (b.x - a.x) * (c.z - a.z);
                        assert!(
                            cross_y > 0.0,
                            "{shape:?} facing {facing} reach {reach}: triangle faces down ({cross_y})",
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn indices_stay_in_range_and_within_the_budget() {
        let ground = GroundSurface::Plane(0.0);
        let huge = [
            TargetShape::Circle { radius: 500.0 },
            TargetShape::Cone {
                range: 500.0,
                half_angle: 1.5,
            },
            TargetShape::Line {
                width: 200.0,
                length: 500.0,
            },
            TargetShape::Rectangle {
                width: 300.0,
                length: 300.0,
            },
        ];
        for shape in huge.into_iter().chain(all_shapes()) {
            let mesh = build(shape, Vec3::ZERO, 0.0, 1.0, &ground);
            assert!(mesh.positions.len() <= MAX_FILL_VERTICES, "{shape:?}");
            assert!(mesh.indices.len() <= MAX_FILL_INDICES, "{shape:?}");
            assert!(
                mesh.indices
                    .iter()
                    .all(|index| (*index as usize) < mesh.positions.len()),
                "{shape:?}: an index is out of range",
            );
        }
    }

    #[test]
    fn a_large_shape_is_coarsened_not_truncated() {
        let ground = GroundSurface::Plane(0.0);
        let mesh = build(
            TargetShape::Circle { radius: 500.0 },
            Vec3::ZERO,
            0.0,
            1.0,
            &ground,
        );
        let reached = mesh
            .positions
            .iter()
            .map(|p| horizontal(*p))
            .fold(0.0, f32::max);
        assert!((reached - 500.0).abs() < 0.1, "reached only {reached}");

        let line = build(
            TargetShape::Line {
                width: 100.0,
                length: 400.0,
            },
            Vec3::ZERO,
            0.0,
            1.0,
            &ground,
        );
        let far = line.positions.iter().fold(0.0_f32, |far, p| far.min(p.z));
        let wide = line.positions.iter().fold(0.0_f32, |wide, p| wide.max(p.x));
        assert!((far + 400.0).abs() < 1e-2 && (wide - 50.0).abs() < 1e-2);
    }

    #[test]
    fn half_reach_covers_half_the_circle_radius() {
        let ground = GroundSurface::Plane(0.0);
        let mesh = build(
            TargetShape::Circle { radius: 8.0 },
            Vec3::ZERO,
            0.0,
            0.5,
            &ground,
        );
        let reached = mesh
            .positions
            .iter()
            .map(|p| horizontal(*p))
            .fold(0.0, f32::max);
        assert!((reached - 4.0).abs() < 1e-3, "reached {reached}");
    }

    #[test]
    fn a_line_grows_from_its_start_over_its_full_width() {
        let ground = GroundSurface::Plane(0.0);
        let mesh = build(
            TargetShape::Line {
                width: 4.0,
                length: 10.0,
            },
            Vec3::ZERO,
            0.0,
            0.5,
            &ground,
        );
        let far = mesh.positions.iter().fold(0.0_f32, |far, p| far.min(p.z));
        let wide = mesh.positions.iter().fold(0.0_f32, |wide, p| wide.max(p.x));
        let behind = mesh.positions.iter().fold(0.0_f32, |back, p| back.max(p.z));
        assert!((far + 5.0).abs() < 1e-3, "reached {far}");
        assert!((wide - 2.0).abs() < 1e-3, "half width {wide}");
        assert!(behind < 1e-3, "reached behind the start: {behind}");
    }

    #[test]
    fn the_spoke_count_does_not_change_as_a_round_fill_grows() {
        let ground = GroundSurface::Plane(0.0);
        // The spoke count comes from the full shape, so the edge does not re-tessellate
        // while growing.
        let shape = TargetShape::Cone {
            range: 10.0,
            half_angle: 0.5,
        };
        let a = build(shape, Vec3::ZERO, 0.0, 0.3, &ground);
        let b = build(shape, Vec3::ZERO, 0.9, 1.0, &ground);
        // The centre fan has one triangle per spoke.
        let fan = |mesh: &FillMesh| {
            mesh.indices
                .chunks_exact(3)
                .take_while(|triangle| triangle[0] == 0)
                .count()
        };
        assert_eq!(fan(&a), fan(&b));
    }

    #[test]
    fn a_cone_is_turned_to_its_facing() {
        let ground = GroundSurface::Plane(0.0);
        let shape = TargetShape::Cone {
            range: 8.0,
            half_angle: 0.4,
        };
        let ahead = build(shape, Vec3::ZERO, 0.0, 1.0, &ground);
        let far = ahead.positions.iter().fold(0.0_f32, |far, p| far.min(p.z));
        assert!(far < -7.0, "a cone at zero yaw reached {far} along -Z");

        let turned = build(shape, Vec3::ZERO, std::f32::consts::FRAC_PI_2, 1.0, &ground);
        let left = turned.positions.iter().fold(0.0_f32, |far, p| far.min(p.x));
        assert!(left < -7.0, "a quarter turn reached {left} along -X");
    }

    #[test]
    fn no_reach_builds_nothing() {
        let ground = GroundSurface::Plane(0.0);
        for shape in all_shapes() {
            for reach in [0.0, -1.0, 0.0005, f32::NAN] {
                let mesh = build(shape, Vec3::ZERO, 0.0, reach, &ground);
                assert!(mesh.positions.is_empty() && mesh.indices.is_empty());
            }
        }
    }

    #[test]
    fn a_rebuild_reuses_the_buffers() {
        let ground = GroundSurface::Plane(0.0);
        let shape = TargetShape::Circle { radius: 6.0 };
        let mut mesh = FillMesh::new();
        build_fill(shape, Vec3::ZERO, 0.0, 1.0, LIFT, &ground, &mut mesh);
        let (vertices, indices) = (mesh.positions.capacity(), mesh.indices.capacity());
        let count = mesh.positions.len();
        build_fill(shape, Vec3::ZERO, 0.0, 1.0, LIFT, &ground, &mut mesh);
        assert_eq!(mesh.positions.len(), count);
        assert_eq!(mesh.positions.capacity(), vertices);
        assert_eq!(mesh.indices.capacity(), indices);
    }

    #[test]
    fn the_anchor_offsets_the_world_position_not_the_stored_one() {
        let ground = GroundSurface::Plane(0.0);
        let shape = TargetShape::Circle { radius: 3.0 };
        let at_origin = build(shape, Vec3::ZERO, 0.0, 1.0, &ground);
        let moved = build(shape, Vec3::new(10.0, 0.0, -7.0), 0.0, 1.0, &ground);
        assert_eq!(at_origin.positions.len(), moved.positions.len());
        for (a, b) in at_origin.positions.iter().zip(&moved.positions) {
            assert!((*a - *b).length() < 1e-4);
        }
    }
}
