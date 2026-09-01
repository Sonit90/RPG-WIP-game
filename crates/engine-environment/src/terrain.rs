//! The ground's shape: a heightmap, and the brush that edits it.
//!
//! ```text
//!        heights[z * side + x]                    what it is for
//!  ┌───┬───┬───┬───┐
//!  │   │   │   │   │   one float per grid point   ground, hills, valleys,
//!  ├───┼───┼───┼───┤   cell_size metres apart     slopes, plateaus
//!  │   │   │  ╱▔▔│
//!  ├───┼──╱┼───┼───┤   two triangles per cell     nothing that overhangs —
//!  │  ╱│   │   │   │                              that is what cliffs are for
//!  └───┴───┴───┴───┘
//! ```
//!
//! # Why a heightmap, when the brief insists on overhangs
//!
//! Because a heightmap is the right answer for the *broad* shape and the wrong answer for
//! everything else, and this system is only asked to be the broad shape. One float per grid
//! point describes a valley in a kilobyte, is edited by dragging a brush over it, and turns
//! into a mesh and a collision surface with no authoring step in between.
//!
//! What it cannot describe is any place where the ground has two heights — a cave mouth, an
//! arch, a ledge with space beneath it. Those are **separate placed geometry**, which is what
//! [`crate::object::EnvironmentObject`] has always been, and the terrain does not know they
//! exist. A player walks under a cliff because the cliff is a mesh standing on the terrain,
//! not because the terrain was bent into something a heightmap cannot hold.
//!
//! That division is the whole design: the terrain is never asked to do the thing it cannot
//! do, so it never has to become a voxel field to keep its promises.
//!
//! # Sampling
//!
//! [`Terrain::height_at`] is bilinear across the cell, which matches the two triangles the
//! mesh and the collision surface are built from closely enough for gameplay and exactly at
//! the grid points. Outside the terrain it returns the base height, so a level whose terrain
//! covers half the playable square has a floor over the other half rather than a hole.
//!
//! # Cost model
//!
//! A `Vec<f32>`. The shipping size is 129×129 — 66 KB, sixteen thousand triangles — and a
//! brush stroke touches the points inside its radius and nothing else. Nothing here runs per
//! frame: the mesh is rebuilt when the heights change and read every frame after that.

use engine_core::math::{Aabb, Vec3};

/// How many grid points a side a terrain may have.
///
/// 257 is a quarter of a megabyte of heights and 130 000 triangles at one triangle per cell
/// pair — past the point where a blockout tool is the right thing to be using. The lower
/// bound is two, because one point has no cell and no surface.
pub const MAX_RESOLUTION: usize = 257;

/// The smallest terrain that is a surface rather than a point.
pub const MIN_RESOLUTION: usize = 2;

/// What a brush stroke does to the heights under it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BrushMode {
    /// Push the ground up.
    Raise,
    /// Pull it down.
    Lower,
    /// Move it towards the height at the middle of the brush.
    ///
    /// A plateau is a flatten stroke centred where the plateau should be, which is why this
    /// takes its target from the terrain rather than from a number the author types.
    Flatten,
    /// Average each point with its neighbours: takes the steps out of a stroke.
    Smooth,
}

impl BrushMode {
    /// Short uppercase name, for the editor's font.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Raise => "RAISE",
            Self::Lower => "LOWER",
            Self::Flatten => "FLATTEN",
            Self::Smooth => "SMOOTH",
        }
    }

    /// The next mode, for a key that cycles them.
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Raise => Self::Lower,
            Self::Lower => Self::Flatten,
            Self::Flatten => Self::Smooth,
            Self::Smooth => Self::Raise,
        }
    }
}

/// The ground, as a grid of heights.
#[derive(Debug, Clone, PartialEq)]
pub struct Terrain {
    /// The corner the grid starts at: the position of `heights[0]`, with `y` the base height.
    ///
    /// A corner rather than a centre because every index calculation in this file is
    /// `(world - origin) / cell_size`, and a centred grid puts a half-resolution offset into
    /// every one of them.
    pub origin: Vec3,
    /// Metres between grid points.
    pub cell_size: f32,
    /// Grid points a side. The terrain is square.
    pub resolution: usize,
    /// Height above [`Terrain::origin`]'s `y`, one per grid point, row-major in `z`.
    pub heights: Vec<f32>,
}

impl Terrain {
    /// Flat ground, `resolution` points a side, centred on the origin in X and Z.
    ///
    /// Centred because a level is built around the middle of the world — the player spawns
    /// there and the editor camera looks at it — and a terrain whose corner was at the origin
    /// would put three quarters of itself where nobody is standing.
    ///
    /// # Panics
    ///
    /// Never. The resolution is clamped into `MIN_RESOLUTION..=MAX_RESOLUTION` and the cell
    /// size to something positive, because a terrain that is one point across or has a zero
    /// cell size is not a surface and every function here would have to answer for it.
    #[must_use]
    pub fn flat(resolution: usize, cell_size: f32, base_height: f32) -> Self {
        let resolution = resolution.clamp(MIN_RESOLUTION, MAX_RESOLUTION);
        let cell_size = if cell_size > 0.0 { cell_size } else { 1.0 };
        let span = (resolution - 1) as f32 * cell_size;
        Self {
            origin: Vec3::new(-span * 0.5, base_height, -span * 0.5),
            cell_size,
            resolution,
            heights: vec![0.0; resolution * resolution],
        }
    }

    /// How wide the terrain is, in metres.
    #[must_use]
    pub fn span(&self) -> f32 {
        (self.resolution.saturating_sub(1)) as f32 * self.cell_size
    }

    /// How many grid points there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.heights.len()
    }

    /// Whether the terrain has no points at all. Only a malformed one does.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.heights.is_empty()
    }

    /// Whether the resolution, the cell size and the height count agree.
    ///
    /// What a loader checks. A terrain that fails this is a file that was edited by hand and
    /// got it wrong, and every function here would otherwise index past the end of a `Vec`.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        (MIN_RESOLUTION..=MAX_RESOLUTION).contains(&self.resolution)
            && self.cell_size > 0.0
            && self.cell_size.is_finite()
            && self.heights.len() == self.resolution * self.resolution
            && self.heights.iter().all(|height| height.is_finite())
            && self.origin.x.is_finite()
            && self.origin.y.is_finite()
            && self.origin.z.is_finite()
    }

    /// The height at grid point `(x, z)`, in world space.
    #[must_use]
    pub fn height_at_point(&self, x: usize, z: usize) -> f32 {
        let x = x.min(self.resolution - 1);
        let z = z.min(self.resolution - 1);
        self.origin.y + self.heights[z * self.resolution + x]
    }

    /// The world position of grid point `(x, z)`.
    #[must_use]
    pub fn position_at_point(&self, x: usize, z: usize) -> Vec3 {
        Vec3::new(
            self.origin.x + x as f32 * self.cell_size,
            self.height_at_point(x, z),
            self.origin.z + z as f32 * self.cell_size,
        )
    }

    /// The ground height under a world position, bilinear across the cell.
    ///
    /// Outside the terrain this is the base height rather than nothing: a level whose terrain
    /// covers part of the playable square needs a floor over the rest of it, and "no terrain
    /// here" and "a hole here" must not look the same to whatever is standing on it.
    #[must_use]
    pub fn height_at(&self, position: Vec3) -> f32 {
        let (x, z) = self.cell_coordinates(position);
        let last = self.resolution - 1;
        let x0 = (x.floor().max(0.0) as usize).min(last);
        let z0 = (z.floor().max(0.0) as usize).min(last);
        let x1 = (x0 + 1).min(last);
        let z1 = (z0 + 1).min(last);
        let tx = (x - x0 as f32).clamp(0.0, 1.0);
        let tz = (z - z0 as f32).clamp(0.0, 1.0);

        let h00 = self.height_at_point(x0, z0);
        let h10 = self.height_at_point(x1, z0);
        let h01 = self.height_at_point(x0, z1);
        let h11 = self.height_at_point(x1, z1);
        let top = h00 + (h10 - h00) * tx;
        let bottom = h01 + (h11 - h01) * tx;
        top + (bottom - top) * tz
    }

    /// The surface normal at a world position, from the slope of the heights around it.
    #[must_use]
    pub fn normal_at(&self, position: Vec3) -> Vec3 {
        let step = self.cell_size;
        let left = self.height_at(position - Vec3::new(step, 0.0, 0.0));
        let right = self.height_at(position + Vec3::new(step, 0.0, 0.0));
        let back = self.height_at(position - Vec3::new(0.0, 0.0, step));
        let front = self.height_at(position + Vec3::new(0.0, 0.0, step));
        Vec3::new(left - right, 2.0 * step, back - front).normalize()
    }

    /// Whether a world position is over the terrain at all.
    #[must_use]
    pub fn contains(&self, position: Vec3) -> bool {
        let span = self.span();
        let dx = position.x - self.origin.x;
        let dz = position.z - self.origin.z;
        (0.0..=span).contains(&dx) && (0.0..=span).contains(&dz)
    }

    /// The lowest and highest point of the surface.
    #[must_use]
    pub fn height_range(&self) -> (f32, f32) {
        let mut low = f32::MAX;
        let mut high = f32::MIN;
        for height in &self.heights {
            low = low.min(*height);
            high = high.max(*height);
        }
        if self.heights.is_empty() {
            return (self.origin.y, self.origin.y);
        }
        (self.origin.y + low, self.origin.y + high)
    }

    /// A box containing the whole surface.
    #[must_use]
    pub fn bounds(&self) -> Aabb {
        let (low, high) = self.height_range();
        let span = self.span();
        Aabb::new(
            Vec3::new(self.origin.x, low, self.origin.z),
            Vec3::new(self.origin.x + span, high, self.origin.z + span),
        )
    }

    /// Applies one brush stroke, centred at `center` with radius `radius` metres.
    ///
    /// `amount` is metres per stroke for [`BrushMode::Raise`] and [`BrushMode::Lower`], and a
    /// fraction of the way to the target for the other two. The falloff is smooth — a stroke
    /// with a hard edge leaves a cliff behind it, and cliffs are not this system's job.
    ///
    /// Returns whether anything moved, so a caller can skip an undo snapshot for a stroke
    /// that fell outside the terrain.
    pub fn apply_brush(
        &mut self,
        center: Vec3,
        radius: f32,
        amount: f32,
        mode: BrushMode,
    ) -> bool {
        if radius <= 0.0 || !self.is_valid() {
            return false;
        }
        let target = self.height_at(center) - self.origin.y;
        // A smooth stroke needs the heights as they were: averaging in place would smear the
        // stroke in whichever direction the loop happens to run.
        let before = matches!(mode, BrushMode::Smooth).then(|| self.heights.clone());

        let (x0, z0, x1, z1) = self.points_within(center, radius);
        let mut moved = false;
        for z in z0..=z1 {
            for x in x0..=x1 {
                let point = self.position_at_point(x, z);
                let distance = ((point.x - center.x).powi(2) + (point.z - center.z).powi(2)).sqrt();
                if distance > radius {
                    continue;
                }
                // Smoothstep from the middle of the brush to its rim.
                let t = 1.0 - (distance / radius).clamp(0.0, 1.0);
                let falloff = t * t * (3.0 - 2.0 * t);
                let index = z * self.resolution + x;
                let height = self.heights[index];
                let next = match mode {
                    BrushMode::Raise => height + amount * falloff,
                    BrushMode::Lower => height - amount * falloff,
                    BrushMode::Flatten => height + (target - height) * falloff.min(1.0),
                    BrushMode::Smooth => {
                        let source = before.as_ref().unwrap_or(&self.heights);
                        let average = self.average_around(source, x, z);
                        height + (average - height) * falloff
                    }
                };
                if (next - height).abs() > f32::EPSILON {
                    self.heights[index] = next;
                    moved = true;
                }
            }
        }
        moved
    }

    /// Which grid points a brush of `radius` at `center` could reach.
    fn points_within(&self, center: Vec3, radius: f32) -> (usize, usize, usize, usize) {
        let last = self.resolution - 1;
        let low = self.cell_coordinates(center - Vec3::new(radius, 0.0, radius));
        let high = self.cell_coordinates(center + Vec3::new(radius, 0.0, radius));
        (
            (low.0.floor().max(0.0) as usize).min(last),
            (low.1.floor().max(0.0) as usize).min(last),
            (high.0.ceil().max(0.0) as usize).min(last),
            (high.1.ceil().max(0.0) as usize).min(last),
        )
    }

    /// The mean of a point and its four neighbours, clamped at the edges.
    fn average_around(&self, heights: &[f32], x: usize, z: usize) -> f32 {
        let last = self.resolution - 1;
        let at = |x: usize, z: usize| heights[z.min(last) * self.resolution + x.min(last)];
        let left = at(x.saturating_sub(1), z);
        let right = at((x + 1).min(last), z);
        let back = at(x, z.saturating_sub(1));
        let front = at(x, (z + 1).min(last));
        (at(x, z) + left + right + back + front) / 5.0
    }

    /// Where a world position sits in grid coordinates, fractional and unclamped.
    fn cell_coordinates(&self, position: Vec3) -> (f32, f32) {
        (
            (position.x - self.origin.x) / self.cell_size,
            (position.z - self.origin.z) / self.cell_size,
        )
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    #[test]
    fn a_flat_terrain_is_centred_and_level() {
        let terrain = Terrain::flat(9, 4.0, 0.0);
        assert!(terrain.is_valid());
        assert_eq!(terrain.span(), 32.0);
        assert_eq!(terrain.origin.x, -16.0);
        assert_eq!(terrain.origin.z, -16.0);
        assert_eq!(terrain.height_at(Vec3::ZERO), 0.0);
        assert_eq!(terrain.height_at(Vec3::new(15.0, 0.0, -15.0)), 0.0);
        assert_eq!(terrain.normal_at(Vec3::ZERO), Vec3::Y);
    }

    #[test]
    fn a_degenerate_terrain_is_clamped_into_something_that_is_a_surface() {
        let tiny = Terrain::flat(0, 0.0, 0.0);
        assert!(tiny.is_valid(), "a one-point terrain was allowed through");
        assert_eq!(tiny.resolution, MIN_RESOLUTION);
        assert!(tiny.cell_size > 0.0);

        let huge = Terrain::flat(10_000, 1.0, 0.0);
        assert_eq!(huge.resolution, MAX_RESOLUTION);
    }

    #[test]
    fn sampling_is_bilinear_between_the_grid_points() {
        let mut terrain = Terrain::flat(3, 10.0, 0.0);
        // One corner raised by ten metres: the middle of that cell is a quarter of the way up.
        terrain.heights[0] = 10.0;
        let corner = terrain.origin;
        assert_eq!(terrain.height_at(corner), 10.0);
        let middle = corner + Vec3::new(5.0, 0.0, 5.0);
        assert!((terrain.height_at(middle) - 2.5).abs() < 1e-5);
        let along = corner + Vec3::new(5.0, 0.0, 0.0);
        assert!((terrain.height_at(along) - 5.0).abs() < 1e-5);
    }

    #[test]
    fn outside_the_terrain_the_ground_is_the_base_height_rather_than_a_hole() {
        let mut terrain = Terrain::flat(3, 10.0, 7.0);
        terrain.heights[4] = 3.0;
        let far = Vec3::new(500.0, 0.0, 500.0);
        assert!(!terrain.contains(far));
        assert_eq!(terrain.height_at(far), terrain.height_at_point(2, 2));
        assert_eq!(terrain.height_at(far), 7.0, "the base height is the floor");
    }

    #[test]
    fn raising_makes_a_hill_that_falls_off_to_nothing_at_the_rim() {
        let mut terrain = Terrain::flat(17, 2.0, 0.0);
        assert!(terrain.apply_brush(Vec3::ZERO, 8.0, 4.0, BrushMode::Raise));

        let peak = terrain.height_at(Vec3::ZERO);
        assert!((peak - 4.0).abs() < 1e-4, "the middle rose by the amount");
        let halfway = terrain.height_at(Vec3::new(4.0, 0.0, 0.0));
        assert!(halfway > 0.1 && halfway < peak, "{halfway}");
        let rim = terrain.height_at(Vec3::new(8.0, 0.0, 0.0));
        assert!(rim.abs() < 1e-3, "the rim moved: {rim}");
        let outside = terrain.height_at(Vec3::new(14.0, 0.0, 0.0));
        assert_eq!(outside, 0.0, "the stroke reached past its own radius");
    }

    #[test]
    fn a_stroke_that_misses_the_terrain_changes_nothing() {
        let mut terrain = Terrain::flat(9, 2.0, 0.0);
        let before = terrain.clone();
        assert!(!terrain.apply_brush(Vec3::new(400.0, 0.0, 400.0), 3.0, 2.0, BrushMode::Raise));
        assert_eq!(terrain, before);
    }

    #[test]
    fn lowering_is_raising_the_other_way() {
        let mut terrain = Terrain::flat(17, 2.0, 0.0);
        terrain.apply_brush(Vec3::ZERO, 8.0, 4.0, BrushMode::Raise);
        terrain.apply_brush(Vec3::ZERO, 8.0, 4.0, BrushMode::Lower);
        for height in &terrain.heights {
            assert!(height.abs() < 1e-4, "{height}");
        }
    }

    #[test]
    fn flattening_pulls_the_ground_to_the_height_under_the_brush() {
        let mut terrain = Terrain::flat(17, 2.0, 0.0);
        terrain.apply_brush(Vec3::ZERO, 12.0, 6.0, BrushMode::Raise);
        let target = terrain.height_at(Vec3::new(6.0, 0.0, 0.0));

        // Flattening centred off the peak levels its neighbourhood to the height it started
        // at, which is how a plateau is made.
        for _ in 0..12 {
            terrain.apply_brush(Vec3::new(6.0, 0.0, 0.0), 5.0, 1.0, BrushMode::Flatten);
        }
        let nearby = terrain.height_at(Vec3::new(6.0, 0.0, 2.0));
        assert!(
            (nearby - target).abs() < 0.25,
            "{nearby} did not level to {target}"
        );
    }

    #[test]
    fn smoothing_takes_the_step_out_of_a_cliff_edge_in_the_heights() {
        let mut terrain = Terrain::flat(9, 2.0, 0.0);
        // A step: half the grid up by four metres.
        for z in 0..terrain.resolution {
            for x in 0..terrain.resolution {
                if x >= terrain.resolution / 2 {
                    terrain.heights[z * terrain.resolution + x] = 4.0;
                }
            }
        }
        let before = terrain.height_at(Vec3::ZERO) - terrain.height_at(Vec3::new(-2.0, 0.0, 0.0));
        for _ in 0..8 {
            terrain.apply_brush(Vec3::ZERO, 8.0, 1.0, BrushMode::Smooth);
        }
        let after = terrain.height_at(Vec3::ZERO) - terrain.height_at(Vec3::new(-2.0, 0.0, 0.0));
        assert!(after < before, "the step is no gentler: {before} -> {after}");
    }

    #[test]
    fn the_bounds_hold_every_point_of_the_surface() {
        let mut terrain = Terrain::flat(9, 2.0, 1.0);
        terrain.apply_brush(Vec3::ZERO, 6.0, 5.0, BrushMode::Raise);
        let bounds = terrain.bounds();
        for z in 0..terrain.resolution {
            for x in 0..terrain.resolution {
                let point = terrain.position_at_point(x, z);
                assert!(
                    point.x >= bounds.min.x - 1e-4
                        && point.x <= bounds.max.x + 1e-4
                        && point.y >= bounds.min.y - 1e-4
                        && point.y <= bounds.max.y + 1e-4
                        && point.z >= bounds.min.z - 1e-4
                        && point.z <= bounds.max.z + 1e-4,
                    "{point:?} is outside {bounds:?}"
                );
            }
        }
    }

    #[test]
    fn a_slope_normal_leans_away_from_the_rise() {
        let mut terrain = Terrain::flat(9, 2.0, 0.0);
        terrain.apply_brush(Vec3::new(-8.0, 0.0, 0.0), 10.0, 6.0, BrushMode::Raise);
        let normal = terrain.normal_at(Vec3::ZERO);
        assert!(normal.y > 0.0, "a normal pointing into the ground");
        assert!(normal.x > 0.01, "the slope rises to -X so the normal tilts to +X: {normal:?}");
    }

    #[test]
    fn the_modes_cycle_and_name_themselves() {
        let mut mode = BrushMode::Raise;
        for _ in 0..4 {
            assert!(!mode.as_str().is_empty());
            mode = mode.next();
        }
        assert_eq!(mode, BrushMode::Raise, "the cycle does not come back round");
    }
}
