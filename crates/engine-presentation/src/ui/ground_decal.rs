//! The ground-targeting indicator's geometry: an outline laid onto the terrain surface.
//!
//! ```text
//!            border band (opaque, bright)
//!           /
//!     ~~~~~^~~~~~~~~~~~~~~~~~~~~~^~~~~~
//!    /      \  glow band          \     terrain
//!   /        `- (translucent, inside the border)
//! ```
//!
//! # Why the shape is a strip of quads rather than one flat mesh
//!
//! A disc laid flat on `y = 0` is only correct on flat ground. On a slope half of it is
//! buried and the other half floats, which is exactly the artefact a player reads as "the
//! indicator is lying about where the spell lands". The fix is to stop drawing a *surface*
//! and start drawing an *outline*: a closed polyline around the ability's footprint,
//! resampled to short pieces, each piece placed at the terrain height under it and tilted
//! to the slope it spans. A short enough piece is indistinguishable from a curve that
//! follows the ground.
//!
//! Pieces rather than one rebuilt mesh because the renderer's meshes are uploaded once and
//! never mutated: re-uploading geometry every frame the cursor moves would allocate a GPU
//! buffer per frame. Instances of one unit quad cost a transform each, and the opaque band
//! collapses into a single instanced draw.
//!
//! # The two bands
//!
//! The border carries the shape and is opaque, so it reads against any terrain material and
//! batches into one draw. The glow sits just inside it, is wide and nearly transparent, and
//! is what makes the enclosed area legible without hiding the ground - an opaque disc would
//! hide exactly the terrain the player is aiming at. Neither band fills the interior: the
//! interior is the terrain, untouched.
//!
//! **Both bands are cut at [`SEGMENT_LENGTH`].** Cutting the wide soft one coarsely to save
//! blended draws was tried and rejected: over rough ground its long pieces could not follow
//! the surface the border was following, so the two visibly parted company and the band
//! faceted. A band that marks the wrong ground is not a cheaper band, it is a wrong one.
//!
//! # The materials this expects
//!
//! Both bands want a near-black base colour with the whole colour in the *emissive* term. The
//! shader's lit contribution scales with the base colour, so a band that carried its colour
//! there would shade differently on every piece as the pieces tilted - a ring of facets on a
//! slope, and an outline that dimmed in a shadow exactly where the ground is hardest to read.
//! Emissive is added after the lighting, so an indicator built that way is one flat colour
//! whatever the ground under it does.
//!
//! # What this module does not do
//!
//! No textures, no shader, no depth-buffer projection. It emits [`Transform`]s and nothing
//! else, which is why every rule here - conformance on a slope, no gaps at a corner, a
//! bounded segment count - is checked in a unit test with no device present.

use engine_core::math::{Quat, Vec3};
use engine_core::transform::Transform;
use engine_environment::terrain::Terrain;

use super::targeting::TargetShape;

/// How long a piece of the outline is allowed to be, in metres.
///
/// The error between a straight piece and the ground under it grows with its length, so
/// this is the whole conformance budget. Half a metre is finer than any terrain cell the
/// editor authors, which means the outline samples the height field more densely than the
/// height field is defined - the piece is straight over ground that is straight too.
pub const SEGMENT_LENGTH: f32 = 0.5;

/// The most pieces one band may be cut into.
///
/// A budget rather than a truncation: an outline longer than `MAX_SEGMENTS * SEGMENT_LENGTH`
/// is cut into longer pieces instead of being cut short. Stopping at a count would leave the
/// last stretch of the outline undrawn, which is the one artefact an area indicator may not
/// have - a gap reads as "the spell does not reach here".
pub const MAX_SEGMENTS: usize = 128;

/// The fewest pieces a closed outline is cut into, whatever its size.
///
/// A tiny circle must still read as a circle rather than as a triangle.
const MIN_SEGMENTS: usize = 24;

/// How much of a piece is forgiven when deciding how many to cut an edge into.
///
/// An edge exactly one step long is one piece, but `span / step` lands a hair over one in
/// binary and `ceil` then asks for two - which doubles the whole outline and puts it over
/// [`MAX_SEGMENTS`]. A thousandth of a piece is far below anything visible and well above
/// the rounding.
const CEIL_SLACK: f32 = 1e-3;

/// The smallest outline this module will build, in metres.
///
/// An inset larger than the shape would fold the outline through itself and produce a knot
/// of quads; clamping to a small positive size draws a dot instead, which is what a shape
/// that small should look like anyway.
const MIN_EXTENT: f32 = 0.05;

/// The ground the indicator is laid on.
///
/// Two cases and no trait: a level either has a height field or it does not, and the flat
/// arena is not a degenerate terrain - it has no grid at all. Making the absence a variant
/// keeps the flat case exact rather than approximating it with a one-cell height field.
#[derive(Debug, Clone, Copy)]
pub enum GroundSurface<'a> {
    /// A level floor at this height, in metres. What the arena scenes have.
    Plane(f32),
    /// A level's height field.
    Terrain(&'a Terrain),
}

impl GroundSurface<'_> {
    /// The ground height under a world position.
    #[must_use]
    pub fn height_at(&self, x: f32, z: f32) -> f32 {
        match self {
            Self::Plane(height) => *height,
            Self::Terrain(terrain) => terrain.height_at(Vec3::new(x, 0.0, z)),
        }
    }

    /// The lowest and highest ground in the level.
    ///
    /// What bounds a ray march: a cursor ray only has to be walked through the band of
    /// space the ground actually occupies, so the marcher spends its steps where the
    /// surface is instead of spreading them over the whole view distance.
    #[must_use]
    pub fn height_range(&self) -> (f32, f32) {
        match self {
            Self::Plane(height) => (*height, *height),
            Self::Terrain(terrain) => terrain.height_range(),
        }
    }
}

/// One ring of the indicator: how far inside the footprint it sits and how wide it is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DecalBand {
    /// Metres from the ability's true edge to the centre of this band.
    ///
    /// The border uses half its own width so that its *outer* rim is the ability's edge:
    /// the line the player reads has to be the boundary the simulation tests against.
    pub inset: f32,
    /// How wide the band is across the outline, in metres.
    pub width: f32,
    /// How far above the ground the band is lifted, in metres.
    ///
    /// Coplanar with the terrain is a depth-buffer tie and flickers; a few centimetres is
    /// below the eye's resolution at gameplay camera distances and above the depth
    /// buffer's.
    pub lift: f32,
}

impl DecalBand {
    /// A band whose outer rim lies exactly on the footprint's edge.
    #[must_use]
    pub fn edged(width: f32, lift: f32) -> Self {
        Self {
            inset: width * 0.5,
            width,
            lift,
        }
    }

    /// The same band pushed `inset` metres in from the footprint's edge.
    #[must_use]
    pub fn with_inset(mut self, inset: f32) -> Self {
        self.inset = inset;
        self
    }
}

/// Reusable working buffers for [`build_band`].
///
/// Owned by the caller and handed back in every frame so a cursor moving across the terrain
/// allocates nothing: both buffers grow to the largest outline the session has drawn and then
/// stay that size.
#[derive(Debug, Default, Clone)]
pub struct DecalScratch {
    /// The shape's corners before resampling.
    corners: Vec<(f32, f32)>,
    /// The resampled outline, one entry per joint between two pieces.
    points: Vec<OutlinePoint>,
}

impl DecalScratch {
    /// Empty buffers.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// One joint of the resampled outline: where it is, how high the ground is, how far the piece
/// leaving it dips below that ground, and how far the pieces reach past it.
#[derive(Debug, Default, Clone, Copy)]
struct OutlinePoint {
    /// World X.
    x: f32,
    /// World Z.
    z: f32,
    /// The ground height here.
    height: f32,
    /// How far under the ground the piece *starting* here passes at its midpoint.
    ///
    /// A straight piece is a chord, and a chord across a ridge cuts under the ground it is
    /// meant to lie on. Held per piece and then resolved into a per-joint lift, so the two
    /// pieces meeting at a joint agree about where the joint is - a lift computed
    /// independently for each piece would leave a step at every one of them.
    sag: f32,
    /// How far each piece meeting here is extended past the joint, in metres.
    ///
    /// Two rectangles that meet centre-line to centre-line at an angle overlap on the inside
    /// of the turn and leave a wedge of bare ground on the outside of it. The miter - half the
    /// band's width times the tangent of half the turn - is exactly the extension that closes
    /// that wedge. It is a hair of nothing along a curve and half a band width at a square
    /// corner, which is where the notch was actually visible.
    miter: f32,
}

/// Fills `out` with one transform per piece of `shape`'s outline, laid on `ground`.
///
/// Each transform places a unit quad - one metre across, one metre long, lying in XZ and
/// facing up - so the caller uploads one mesh and draws it once per piece. `out` and
/// `scratch` are cleared first and never reallocated once they have grown, which is what
/// makes moving the cursor across the terrain free of per-frame allocation.
///
/// `facing` is the caster's yaw in radians about +Y, zero facing -Z, as everywhere else in
/// the project. Circles ignore it.
///
/// # The four passes
///
/// 1. The shape becomes a closed polygon, inset by the band.
/// 2. Every edge of it is cut into pieces no longer than [`SEGMENT_LENGTH`], and the ground
///    height under each joint is sampled once. Adjacent pieces therefore share a joint
///    *exactly*, which is what leaves no gap along the line.
/// 3. Each joint learns how far the chords either side of it dip under the ground, and how
///    far the pieces have to reach past it to close the corner they turn.
/// 4. Each piece is emitted between its two joints, lifted by the deeper of the two dips and
///    extended by the miter at each end.
pub fn build_band(
    shape: TargetShape,
    anchor: Vec3,
    facing: f32,
    band: DecalBand,
    ground: &GroundSurface<'_>,
    scratch: &mut DecalScratch,
    out: &mut Vec<Transform>,
) {
    out.clear();
    outline(shape, anchor, facing, band, &mut scratch.corners);
    let corners = &scratch.corners;
    if corners.len() < 2 {
        scratch.points.clear();
        return;
    }

    // One step for the whole outline, chosen from its total length. A per-edge step would
    // let a large shape run past the budget and be cut short mid-loop; deriving the step
    // from the perimeter spends the budget evenly and always closes the loop.
    let count = corners.len();
    let perimeter: f32 = (0..count)
        .map(|index| distance(corners[index], corners[(index + 1) % count]))
        .sum();
    let step = SEGMENT_LENGTH
        .max(perimeter / MAX_SEGMENTS as f32)
        .max(f32::MIN_POSITIVE);

    // A closed loop: the last corner joins the first, so a rectangle's fourth side exists
    // and a circle has no seam. Only the *start* of each piece is recorded - the end of one
    // is the start of the next, and the last piece closes onto the first joint.
    scratch.points.clear();
    for index in 0..count {
        let start = corners[index];
        let end = corners[(index + 1) % count];
        let span = distance(start, end);
        if span <= f32::EPSILON {
            continue;
        }
        // The subtraction absorbs the rounding that would otherwise turn an edge of exactly
        // one step into two pieces and put the whole outline over budget.
        let pieces = ((span / step) - CEIL_SLACK).ceil().max(1.0) as usize;
        for piece in 0..pieces {
            let (x, z) = lerp(start, end, piece as f32 / pieces as f32);
            scratch.points.push(OutlinePoint {
                x,
                z,
                height: ground.height_at(x, z),
                sag: 0.0,
                miter: 0.0,
            });
        }
    }

    let joints = scratch.points.len();
    if joints < 2 {
        return;
    }

    // How far each chord dips under the ground it spans, measured at its midpoint. One extra
    // height sample per piece, and the only thing that keeps a hilltop from swallowing the
    // line between two joints either side of it.
    for index in 0..joints {
        let a = scratch.points[index];
        let b = scratch.points[(index + 1) % joints];
        let middle_x = (a.x + b.x) * 0.5;
        let middle_z = (a.z + b.z) * 0.5;
        let chord = (a.height + b.height) * 0.5;
        scratch.points[index].sag = (ground.height_at(middle_x, middle_z) - chord).max(0.0);
    }

    // How far the two pieces meeting at each joint have to reach past it to close the wedge
    // the turn opens on the outside of the corner.
    for index in 0..joints {
        let previous = scratch.points[(index + joints - 1) % joints];
        let here = scratch.points[index];
        let next = scratch.points[(index + 1) % joints];
        scratch.points[index].miter = miter(
            (here.x - previous.x, here.z - previous.z),
            (next.x - here.x, next.z - here.z),
            band.width,
        );
    }

    for index in 0..joints {
        let a = scratch.points[index];
        let b = scratch.points[(index + 1) % joints];
        // The deeper of the two chords meeting at a joint. Taking the maximum rather than
        // each piece's own sag is what makes the two agree on the joint's height, and taking
        // it rather than an average is what keeps the shallower piece out of the ground too.
        let previous = scratch.points[(index + joints - 1) % joints].sag;
        let lift_a = previous.max(a.sag) + band.lift;
        let lift_b = a.sag.max(b.sag) + band.lift;
        out.push(place(
            (a.x, a.height + lift_a, a.z),
            (b.x, b.height + lift_b, b.z),
            (a.miter, b.miter),
            band,
        ));
    }
}

/// Where a single piece of the outline goes, given its two placed ends and their miters.
///
/// The quad is centred on the piece, turned to run along it, and pitched by the slope it
/// spans, so both of its ends land where they were placed rather than only its middle. Each
/// end then reaches `miters` metres further along the piece, which is what fills the corner.
fn place(a: (f32, f32, f32), b: (f32, f32, f32), miters: (f32, f32), band: DecalBand) -> Transform {
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    let dz = b.2 - a.2;
    let horizontal = (dx * dx + dz * dz).sqrt();
    let span = (horizontal * horizontal + dy * dy).sqrt();

    // Zero yaw faces -Z, so the yaw that points a quad's local -Z along the piece is
    // `atan2(-dx, -dz)`; the pitch that lifts that direction to the slope is the angle
    // between the piece and the horizontal.
    let yaw = (-dx).atan2(-dz);
    let pitch = dy.atan2(horizontal);

    // Growing the quad and sliding its centre by half the difference is the same thing as
    // moving each end outwards by its own miter, without normalizing the direction twice.
    let length = span + miters.0 + miters.1;
    let shift = (miters.1 - miters.0) * 0.5;
    let along = if span > f32::EPSILON {
        (dx / span, dy / span, dz / span)
    } else {
        (0.0, 0.0, 0.0)
    };

    Transform {
        translation: Vec3::new(
            (a.0 + b.0) * 0.5 + along.0 * shift,
            (a.1 + b.1) * 0.5 + along.1 * shift,
            (a.2 + b.2) * 0.5 + along.2 * shift,
        ),
        rotation: Quat::from_yaw_pitch_roll(yaw, pitch, 0.0),
        scale: Vec3::new(band.width, 1.0, length.max(f32::MIN_POSITIVE)),
    }
}

/// How far past a joint each piece must reach so no bare ground shows on the outside of it.
///
/// `half_width * tan(turn / 2)`, the standard miter, capped at one band width: the tangent
/// runs away at a hairpin, and a piece that reached a metre past a doubled-back joint would
/// stick out of the outline as a spike instead of filling a corner.
fn miter(incoming: (f32, f32), outgoing: (f32, f32), width: f32) -> f32 {
    let before = (incoming.0 * incoming.0 + incoming.1 * incoming.1).sqrt();
    let after = (outgoing.0 * outgoing.0 + outgoing.1 * outgoing.1).sqrt();
    if before <= f32::EPSILON || after <= f32::EPSILON {
        return 0.0;
    }
    let dot = (incoming.0 * outgoing.0 + incoming.1 * outgoing.1) / (before * after);
    let cross = (incoming.0 * outgoing.1 - incoming.1 * outgoing.0) / (before * after);
    let turn = cross.abs().atan2(dot);
    (width * 0.5 * (turn * 0.5).tan()).clamp(0.0, width)
}

/// The closed polygon around `shape`, moved `inset` metres inwards.
///
/// Corners only for the straight-sided shapes and a dense arc for the round ones: the
/// caller resamples every edge to [`SEGMENT_LENGTH`] anyway, so this only has to be right
/// about *where the boundary is*, not about how finely it is cut.
fn outline(
    shape: TargetShape,
    anchor: Vec3,
    facing: f32,
    band: DecalBand,
    out: &mut Vec<(f32, f32)>,
) {
    let inset = band.inset;
    out.clear();
    match shape {
        TargetShape::Circle { radius } => {
            let radius = (radius - inset).max(MIN_EXTENT);
            let count = ring_segments(std::f32::consts::TAU * radius);
            for index in 0..count {
                let angle = index as f32 / count as f32 * std::f32::consts::TAU;
                let (sin, cos) = angle.sin_cos();
                out.push((anchor.x + cos * radius, anchor.z + sin * radius));
            }
        }
        TargetShape::Cone { range, half_angle } => {
            // A wedge shrinks towards its apex: the two straight edges move inwards by the
            // inset, which slides the apex forward by `inset / sin(half_angle)` - the miter
            // of the corner they make. The arc simply loses the inset from its radius.
            let half_angle = half_angle.clamp(0.01, std::f32::consts::PI - 0.01);
            let radius = (range - inset).max(MIN_EXTENT);
            let apex = (inset / half_angle.sin()).min(radius - MIN_EXTENT).max(0.0);
            let count = ring_segments(2.0 * half_angle * radius).max(3);
            out.push(local_to_world(anchor, facing, 0.0, -apex));
            for index in 0..=count {
                let angle = -half_angle + 2.0 * half_angle * (index as f32 / count as f32);
                let (sin, cos) = angle.sin_cos();
                // Zero yaw looks along -Z, so the wedge opens about -Z and `angle` sweeps
                // across it.
                out.push(local_to_world(
                    anchor,
                    facing,
                    sin * radius,
                    -(cos * radius),
                ));
            }
        }
        TargetShape::Line { width, length } => {
            // Anchored at the caster and running forwards, so the near edge is inset from
            // the caster and the far edge from the tip.
            let half = (width * 0.5 - inset).max(MIN_EXTENT);
            let near = inset.min(length * 0.5 - MIN_EXTENT).max(0.0);
            let far = (length - inset).max(near + MIN_EXTENT);
            out.push(local_to_world(anchor, facing, -half, -near));
            out.push(local_to_world(anchor, facing, half, -near));
            out.push(local_to_world(anchor, facing, half, -far));
            out.push(local_to_world(anchor, facing, -half, -far));
        }
        TargetShape::Rectangle { width, length } => {
            let half_width = (width * 0.5 - inset).max(MIN_EXTENT);
            let half_length = (length * 0.5 - inset).max(MIN_EXTENT);
            out.push(local_to_world(anchor, facing, -half_width, -half_length));
            out.push(local_to_world(anchor, facing, half_width, -half_length));
            out.push(local_to_world(anchor, facing, half_width, half_length));
            out.push(local_to_world(anchor, facing, -half_width, half_length));
        }
    }
}

/// How many corners a curve of `arc` metres is described with.
///
/// The corner count is what makes a circle round, so it is chosen here rather than left to
/// the resampling pass: an edge is only ever *subdivided* later, never merged with its
/// neighbour.
fn ring_segments(arc: f32) -> usize {
    ((arc / SEGMENT_LENGTH).ceil() as usize).clamp(MIN_SEGMENTS, MAX_SEGMENTS)
}

/// A point in the caster's frame - `x` across, `z` along, -Z forward - placed in the world.
pub(crate) fn local_to_world(anchor: Vec3, facing: f32, x: f32, z: f32) -> (f32, f32) {
    let (sin, cos) = facing.sin_cos();
    (anchor.x + x * cos + z * sin, anchor.z - x * sin + z * cos)
}

/// Distance between two points on the ground.
fn distance(a: (f32, f32), b: (f32, f32)) -> f32 {
    let dx = b.0 - a.0;
    let dz = b.1 - a.1;
    (dx * dx + dz * dz).sqrt()
}

/// A point a fraction of the way along a segment.
fn lerp(a: (f32, f32), b: (f32, f32), t: f32) -> (f32, f32) {
    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_environment::terrain::Terrain;

    /// The border the app draws, at the app's own width.
    fn border() -> DecalBand {
        DecalBand::edged(0.28, 0.05)
    }

    /// Ground built from a closure of world X and Z, on a one-metre grid.
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

    /// Every point a band's pieces cover, in world space: both ends of every quad.
    ///
    /// Reconstructed from the transform rather than from the builder's own intermediate
    /// values, so a test that passes is a test the *renderer* would draw correctly - a
    /// rotation with the wrong sign shows up here and nowhere else.
    fn piece_ends(pieces: &[Transform]) -> Vec<Vec3> {
        let mut ends = Vec::with_capacity(pieces.len() * 2);
        for piece in pieces {
            // The unit quad runs from z = -0.5 to z = +0.5 in its own space, scaled by the
            // piece length; local -Z is the direction the piece runs.
            let along = piece.rotation.rotate(Vec3::new(0.0, 0.0, -1.0)) * (piece.scale.z * 0.5);
            ends.push(piece.translation - along);
            ends.push(piece.translation + along);
        }
        ends
    }

    fn build(shape: TargetShape, ground: &GroundSurface<'_>, band: DecalBand) -> Vec<Transform> {
        let mut scratch = DecalScratch::new();
        let mut pieces = Vec::new();
        build_band(
            shape,
            Vec3::ZERO,
            0.0,
            band,
            ground,
            &mut scratch,
            &mut pieces,
        );
        pieces
    }

    #[test]
    fn a_circle_on_flat_ground_closes_and_sits_at_the_floor() {
        let ground = GroundSurface::Plane(2.0);
        let pieces = build(TargetShape::Circle { radius: 6.0 }, &ground, border());
        assert!(pieces.len() > 20, "{} pieces", pieces.len());

        for point in piece_ends(&pieces) {
            // Lifted by exactly the band's lift: flat ground has no slope to correct for and
            // no sag to compensate, so any other height is a bug in the placement.
            assert!(
                (point.y - (2.0 + border().lift)).abs() < 1e-3,
                "a piece left the floor at {point:?}",
            );
            // The band's *outer* rim is the ability's edge, so its centre line is half a
            // width inside it.
            let radius = (point.x * point.x + point.z * point.z).sqrt();
            assert!(
                (radius - (6.0 - border().width * 0.5)).abs() < 0.05,
                "a piece left the outline at radius {radius}",
            );
        }
    }

    #[test]
    fn adjacent_pieces_meet_or_overlap_so_the_outline_has_no_gaps() {
        // The one artefact an area indicator may not have. Two pieces that merely came close
        // would leave a bead of bare ground at every joint, all the way round. They are
        // allowed to *overlap* - the miter that closes a corner is an overlap on the inside
        // of the turn - but never to part.
        let terrain = terrain_from(|x, z| (x * 0.3).sin() * 2.0 + (z * 0.2).cos() * 1.5);
        let ground = GroundSurface::Terrain(&terrain);
        for shape in [
            TargetShape::Circle { radius: 7.0 },
            TargetShape::Cone {
                range: 9.0,
                half_angle: 0.6,
            },
            TargetShape::Line {
                width: 10.0,
                length: 20.0,
            },
            TargetShape::Rectangle {
                width: 6.0,
                length: 12.0,
            },
        ] {
            let pieces = build(shape, &ground, border());
            for pair in pieces.windows(2) {
                let (previous, next) = (&pair[0], &pair[1]);
                let previous_end = piece_ends(std::slice::from_ref(previous))[1];
                let ends = piece_ends(std::slice::from_ref(next));
                let (next_start, next_end) = (ends[0], ends[1]);
                // Along the *next* piece: where its own start sits relative to where the
                // previous one stopped. Zero is a shared joint and negative is an overlap;
                // positive is bare ground between them.
                let along = (next_end - next_start).normalize();
                let offset = next_start - previous_end;
                let gap = offset.x * along.x + offset.y * along.y + offset.z * along.z;
                assert!(
                    gap < 1e-3,
                    "{shape:?}: {gap} m of bare ground between two pieces",
                );
                // And the overlap is bounded: a piece that shot past its joint would spike
                // out of the outline instead of filling a corner.
                assert!(
                    (-gap) <= border().width * 2.0 + 1e-3,
                    "{shape:?}: a piece overshot its joint by {} m",
                    -gap,
                );
            }
            // The loop closes: the last piece reaches at least where the first one starts.
            let ends = piece_ends(&pieces);
            let first = ends[0];
            let last = ends[ends.len() - 1];
            let closing = (first.x - last.x).abs() + (first.z - last.z).abs();
            assert!(
                closing < border().width * 2.0,
                "{shape:?}: the outline did not close, {closing} m apart",
            );
        }
    }

    #[test]
    fn a_piece_never_floats_over_the_ground_and_never_sinks_under_it() {
        // The whole point of the feature, stated as a bound: every end of every piece is
        // within a few centimetres of the terrain under it, uphill, downhill and across.
        for (name, terrain) in [
            ("gentle slope", terrain_from(|x, _| x * 0.15)),
            ("steep slope", terrain_from(|x, _| x * 1.4)),
            ("hill", terrain_from(|x, z| 8.0 - (x * x + z * z) * 0.05)),
            ("valley", terrain_from(|x, z| (x * x + z * z) * 0.04)),
            (
                "ripples",
                terrain_from(|x, z| (x * 0.8).sin() + (z * 0.8).sin()),
            ),
            (
                "small bumps",
                terrain_from(|x, z| (x * 0.4).sin() * 0.15 + z * 0.01),
            ),
        ] {
            let ground = GroundSurface::Terrain(&terrain);
            for radius in [1.5_f32, 4.0, 8.0, 15.0] {
                let shape = TargetShape::Circle { radius };
                let pieces = build(shape, &ground, border());
                for point in piece_ends(&pieces) {
                    let under = ground.height_at(point.x, point.z);
                    let above = point.y - under;
                    assert!(
                        above > -1e-3,
                        "{name} at r={radius}: a piece sank {above} m under the ground",
                    );
                    assert!(
                        above < 0.35,
                        "{name} at r={radius}: a piece floated {above} m over the ground",
                    );
                }
            }
        }
    }

    #[test]
    fn a_piece_is_turned_and_tilted_to_run_along_the_ground_it_spans() {
        // The rotation has to place both ends, not only the middle: a piece that was placed
        // at the right height but left horizontal would poke through the uphill side of
        // every slope it crossed.
        let terrain = terrain_from(|x, _| x * 0.8);
        let ground = GroundSurface::Terrain(&terrain);
        let pieces = build(TargetShape::Circle { radius: 5.0 }, &ground, border());

        let mut tilted = 0;
        for piece in &pieces {
            // The quad still faces upwards - a piece rolled past vertical would be
            // backface-culled and simply not drawn.
            let up = piece.rotation.rotate(Vec3::Y);
            assert!(up.y > 0.5, "a piece was tilted onto its side: {up:?}");
            if piece.rotation.rotate(Vec3::new(0.0, 0.0, -1.0)).y.abs() > 0.05 {
                tilted += 1;
            }
        }
        assert!(
            tilted > pieces.len() / 4,
            "only {tilted} of {} pieces followed the slope",
            pieces.len(),
        );
    }

    #[test]
    fn the_glow_band_sits_inside_the_border_and_never_crosses_it() {
        // Two bands that overlapped would double-blend into a bright seam, and a glow band
        // outside the border would claim ground the ability does not reach.
        let ground = GroundSurface::Plane(0.0);
        let radius = 8.0_f32;
        let border_band = border();
        let glow = DecalBand::edged(0.85, 0.035).with_inset(border_band.width + 0.85 * 0.5);

        let border_inner = radius - border_band.width;
        for piece in build(TargetShape::Circle { radius }, &ground, glow) {
            let centre = (piece.translation.x.powi(2) + piece.translation.z.powi(2)).sqrt();
            let outer = centre + glow.width * 0.5;
            assert!(
                outer <= border_inner + 0.05,
                "the glow reached {outer} m, past the border at {border_inner} m",
            );
        }
        // And it is cheaper than the border it sits inside, because every piece of it is a
        // blended draw of its own.
        let border_pieces = build(TargetShape::Circle { radius }, &ground, border_band).len();
        let glow_pieces = build(TargetShape::Circle { radius }, &ground, glow).len();
        assert!(
            glow_pieces < border_pieces,
            "the glow is {glow_pieces} pieces against the border's {border_pieces}",
        );
    }

    #[test]
    fn a_huge_shape_spends_its_budget_evenly_rather_than_stopping_short() {
        // The failure this guards: capping the piece *count* would draw the first stretch of
        // a large outline at full detail and leave the rest of the loop missing.
        let ground = GroundSurface::Plane(0.0);
        let pieces = build(TargetShape::Circle { radius: 400.0 }, &ground, border());
        assert!(
            pieces.len() <= MAX_SEGMENTS,
            "{} pieces is over budget",
            pieces.len(),
        );

        let ends = piece_ends(&pieces);
        let first = ends[0];
        let last = ends[ends.len() - 1];
        assert!(
            (first.x - last.x).abs() + (first.z - last.z).abs() < 1e-1,
            "the outline stopped short of closing",
        );
        // Every piece is about the same length: the budget was divided, not truncated.
        let lengths: Vec<f32> = pieces.iter().map(|piece| piece.scale.z).collect();
        let longest = lengths.iter().copied().fold(0.0_f32, f32::max);
        let shortest = lengths.iter().copied().fold(f32::MAX, f32::min);
        assert!(
            longest - shortest < 0.5,
            "piece lengths ran from {shortest} to {longest}",
        );
    }

    #[test]
    fn moving_the_target_across_the_terrain_moves_the_outline_smoothly() {
        // Dragging the cursor must not make the indicator jump: a placement half a metre
        // along may not change the piece count or move a piece by more than it moved.
        let terrain = terrain_from(|x, z| (x * 0.25).sin() * 3.0 + (z * 0.35).cos() * 2.0);
        let ground = GroundSurface::Terrain(&terrain);
        let shape = TargetShape::Circle { radius: 6.0 };
        let mut scratch = DecalScratch::new();
        let mut previous: Vec<Transform> = Vec::new();

        for step in 0..40 {
            let anchor = Vec3::new(-8.0 + step as f32 * 0.4, 0.0, 3.0);
            let mut pieces = Vec::new();
            build_band(
                shape,
                anchor,
                0.0,
                border(),
                &ground,
                &mut scratch,
                &mut pieces,
            );
            if !previous.is_empty() {
                assert_eq!(
                    pieces.len(),
                    previous.len(),
                    "the piece count changed mid-drag at step {step}",
                );
                for (before, after) in previous.iter().zip(&pieces) {
                    let jump = (after.translation.y - before.translation.y).abs();
                    assert!(
                        jump < 1.0,
                        "a piece jumped {jump} m vertically at step {step}",
                    );
                }
            }
            previous = pieces;
        }
    }

    #[test]
    fn a_shape_smaller_than_its_own_band_still_draws_a_closed_outline() {
        // The degenerate case: an inset wider than the shape would otherwise fold the
        // outline through itself into a knot of overlapping quads.
        let ground = GroundSurface::Plane(0.0);
        for shape in [
            TargetShape::Circle { radius: 0.1 },
            TargetShape::Cone {
                range: 0.2,
                half_angle: 0.05,
            },
            TargetShape::Line {
                width: 0.2,
                length: 0.3,
            },
            TargetShape::Rectangle {
                width: 0.1,
                length: 0.1,
            },
        ] {
            let pieces = build(shape, &ground, DecalBand::edged(2.0, 0.05));
            assert!(!pieces.is_empty(), "{shape:?} drew nothing at all");
            for piece in &pieces {
                assert!(
                    piece.scale.z.is_finite() && piece.scale.z > 0.0,
                    "{shape:?}: a piece of length {}",
                    piece.scale.z,
                );
                assert!(
                    piece.translation.y.is_finite(),
                    "{shape:?}: a piece at {:?}",
                    piece.translation,
                );
            }
        }
    }

    #[test]
    fn a_directional_shape_is_turned_to_where_the_caster_is_aiming() {
        // Zero yaw faces -Z everywhere in the project, and a quarter turn has to move the
        // line onto -X. A shape that ignored the facing would draw the strip behind the
        // player exactly as often as in front.
        let ground = GroundSurface::Plane(0.0);
        let shape = TargetShape::Line {
            width: 6.0,
            length: 18.0,
        };
        let mut scratch = DecalScratch::new();
        let mut pieces = Vec::new();

        build_band(
            shape,
            Vec3::ZERO,
            0.0,
            border(),
            &ground,
            &mut scratch,
            &mut pieces,
        );
        let ahead = piece_ends(&pieces)
            .into_iter()
            .fold(0.0_f32, |far, point| far.min(point.z));
        assert!(ahead < -16.0, "the strip reached only {ahead} along -Z");

        build_band(
            shape,
            Vec3::ZERO,
            std::f32::consts::FRAC_PI_2,
            border(),
            &ground,
            &mut scratch,
            &mut pieces,
        );
        let sideways = piece_ends(&pieces)
            .into_iter()
            .fold(0.0_f32, |far, point| far.min(point.x));
        assert!(sideways < -16.0, "a quarter turn reached only {sideways}");
    }

    #[test]
    fn the_flat_case_is_exact_rather_than_a_one_cell_height_field() {
        let plane = GroundSurface::Plane(3.25);
        assert_eq!(plane.height_at(120.0, -400.0), 3.25);
        assert_eq!(plane.height_range(), (3.25, 3.25));

        let terrain = terrain_from(|x, _| x);
        let ground = GroundSurface::Terrain(&terrain);
        let (low, high) = ground.height_range();
        assert!(low < -60.0 && high > 60.0, "range was {low}..{high}");
    }
}
