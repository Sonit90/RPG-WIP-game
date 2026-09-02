//! Water: a rectangle with a yaw, a surface height and a depth.
//!
//! ```text
//!         yaw
//!          ↻                     ~~~~~~~~~~~~~~~~~~~~~  surface, at center.y
//!    ┌───────────┐               │                   │
//!    │           │  half_extents │      depth        │
//!    │     •     │  in X and Z   │                   │
//!    │           │               └───────────────────┘  the volume the query answers about
//!    └───────────┘
//!      seen in plan                    seen from the side
//! ```
//!
//! # Why a world element and not a placement
//!
//! A placement is an asset id and a transform, and the engine resolves the id to a mesh.
//! Water has no asset: it has a size somebody dragged out, a height somebody chose, and one
//! question gameplay asks of it. Making it a placement would mean "is the player in water"
//! had to be answered by searching the object list for meshes that happen to be transparent.
//! [`EnvironmentScene`](crate::EnvironmentScene) already holds two things of this kind — its
//! lights and its terrain — and water is the third.
//!
//! # What this is not
//!
//! Not a simulation. No waves, no flow, no buoyancy, no swimming, and no collision: a
//! character walks through water because as far as movement is concerned it is not there.
//! What it is, is a transparent quad and [`WaterBody::contains`].
//!
//! # Why a depth rather than only a surface
//!
//! Without one, "under water" means "below this height anywhere in the level", which is true
//! inside the hill next to the lake. A body is a volume, and the volume is what the question
//! is about.
//!
//! # Cost model
//!
//! Forty bytes a body, and a level has a handful. [`WaterBody::contains`] is one sine, one
//! cosine and six comparisons; nothing here caches anything, because there is nothing a
//! cache would save.

use engine_core::math::{Aabb, Vec2, Vec3};

/// A body of water: a rectangle with a yaw, at a height, with a depth under it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WaterBody {
    /// The middle of the surface. `y` is the height the water sits at.
    pub center: Vec3,
    /// Half-width and half-depth of the rectangle, in metres, before rotation.
    pub half_extents: Vec2,
    /// Rotation about +Y, radians.
    pub yaw: f32,
    /// How far the volume reaches below the surface, in metres.
    pub depth: f32,
}

impl WaterBody {
    /// A body `half` metres out from `center` in each direction, `depth` metres deep.
    #[must_use]
    pub fn new(center: Vec3, half_extents: Vec2, depth: f32) -> Self {
        Self {
            center,
            half_extents,
            yaw: 0.0,
            depth,
        }
    }

    /// The same body, turned about +Y.
    #[must_use]
    pub fn with_yaw(mut self, yaw: f32) -> Self {
        self.yaw = yaw;
        self
    }

    /// The height the surface sits at.
    #[must_use]
    pub fn surface_height(&self) -> f32 {
        self.center.y
    }

    /// The height the bottom of the volume sits at.
    #[must_use]
    pub fn bottom_height(&self) -> f32 {
        self.center.y - self.depth
    }

    /// Whether a world point is inside the volume.
    ///
    /// Inside the rectangle *and* between the surface and the bottom. The point is brought
    /// into the body's own space by turning it about the centre by `-yaw` — one sine and one
    /// cosine, the same trick the editor's local-space nudge uses — so a turned pond is a
    /// turned pond rather than the box around one.
    #[must_use]
    pub fn contains(&self, point: Vec3) -> bool {
        if point.y > self.center.y || point.y < self.bottom_height() {
            return false;
        }
        let (dx, dz) = self.local_offset(point);
        dx.abs() <= self.half_extents.x && dz.abs() <= self.half_extents.y
    }

    /// Where a world point sits in the body's own space, in X and Z.
    fn local_offset(&self, point: Vec3) -> (f32, f32) {
        let (sin, cos) = (-self.yaw).sin_cos();
        let dx = point.x - self.center.x;
        let dz = point.z - self.center.z;
        (dx * cos + dz * sin, -dx * sin + dz * cos)
    }

    /// The four corners of the surface, in world space, counter-clockwise seen from above.
    ///
    /// What the mesh is built from and what [`WaterBody::bounds`] is the box around, so the
    /// rectangle drawn and the rectangle picked cannot disagree about where its corners are.
    #[must_use]
    pub fn corners(&self) -> [Vec3; 4] {
        let (sin, cos) = self.yaw.sin_cos();
        let (hx, hz) = (self.half_extents.x, self.half_extents.y);
        let place = |x: f32, z: f32| {
            Vec3::new(
                self.center.x + x * cos + z * sin,
                self.center.y,
                self.center.z - x * sin + z * cos,
            )
        };
        [place(-hx, hz), place(hx, hz), place(hx, -hz), place(-hx, -hz)]
    }

    /// The box containing the rotated rectangle and everything under it.
    ///
    /// For the editor's picking and for anything that wants a cheap reject before paying for
    /// [`WaterBody::contains`].
    #[must_use]
    pub fn bounds(&self) -> Aabb {
        let mut bounds = Aabb::from_points(self.corners());
        bounds.insert(Vec3::new(bounds.min.x, self.bottom_height(), bounds.min.z));
        bounds
    }

    /// Whether the numbers describe a volume something could be inside.
    ///
    /// What a loader checks. A zero extent is a surface nothing can be within and a query
    /// that can never be true; a zero depth is the same fact said about the other axis.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.center.x.is_finite()
            && self.center.y.is_finite()
            && self.center.z.is_finite()
            && self.yaw.is_finite()
            && self.half_extents.x.is_finite()
            && self.half_extents.y.is_finite()
            && self.half_extents.x > 0.0
            && self.half_extents.y > 0.0
            && self.depth.is_finite()
            && self.depth > 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_PI_2;

    fn pond() -> WaterBody {
        WaterBody::new(Vec3::new(0.0, 1.0, 0.0), Vec2::new(6.0, 2.0), 3.0)
    }

    #[test]
    fn a_point_under_the_surface_and_inside_the_rectangle_is_in_the_water() {
        let body = pond();
        assert!(body.contains(Vec3::new(0.0, 0.0, 0.0)));
        assert!(body.contains(Vec3::new(5.9, 1.0, 1.9)));
    }

    #[test]
    fn above_below_and_outside_are_three_separate_falses() {
        let body = pond();
        assert!(
            !body.contains(Vec3::new(0.0, 1.5, 0.0)),
            "a head above the surface is not under water"
        );
        assert!(
            !body.contains(Vec3::new(0.0, -2.5, 0.0)),
            "deeper than the depth is under the volume, not in it"
        );
        assert!(
            !body.contains(Vec3::new(9.0, 0.0, 0.0)),
            "outside the rectangle in X"
        );
        assert!(
            !body.contains(Vec3::new(0.0, 0.0, 9.0)),
            "outside the rectangle in Z"
        );
    }

    #[test]
    fn a_yaw_turns_the_rectangle_rather_than_the_box_around_it() {
        // The pond is twelve metres along X and four along Z. A quarter turn swaps them, so
        // a point five metres out along Z is inside the turned body and outside the
        // unturned one — and a point five metres out along X is the reverse.
        let flat = pond();
        let turned = pond().with_yaw(FRAC_PI_2);

        let along_z = Vec3::new(0.0, 0.5, 5.0);
        assert!(!flat.contains(along_z));
        assert!(turned.contains(along_z));

        let along_x = Vec3::new(5.0, 0.5, 0.0);
        assert!(flat.contains(along_x));
        assert!(!turned.contains(along_x));
    }

    #[test]
    fn the_bounds_hold_the_turned_rectangle_and_the_depth_under_it() {
        let body = pond().with_yaw(FRAC_PI_2);
        let bounds = body.bounds();
        assert!((bounds.max.x - 2.0).abs() < 1e-4, "{:?}", bounds);
        assert!((bounds.max.z - 6.0).abs() < 1e-4, "{:?}", bounds);
        assert!((bounds.max.y - 1.0).abs() < 1e-4, "{:?}", bounds);
        assert!((bounds.min.y + 2.0).abs() < 1e-4, "{:?}", bounds);
    }

    #[test]
    fn a_body_with_no_size_is_refused_rather_than_queried() {
        let mut body = pond();
        assert!(body.is_valid());
        body.half_extents.x = 0.0;
        assert!(!body.is_valid());
        body.half_extents.x = 6.0;
        body.depth = 0.0;
        assert!(!body.is_valid(), "a volume with no depth holds nothing");
        body.depth = f32::NAN;
        assert!(!body.is_valid());
    }
}
