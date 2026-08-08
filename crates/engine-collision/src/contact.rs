//! What a collision test reports.
//!
//! One struct, deliberately: a contact is everything movement resolution needs to push a
//! character out of a wall and to decide whether that wall was a floor.
//!
//! ```text
//!            normal (out of the surface, towards the capsule)
//!              ^
//!              |      .-.
//!   ###########|#####(   )     penetration = how far the capsule is inside
//!   ###########o######`-'      point      = where on the surface
//!   #  static  #
//! ```

use engine_core::math::Vec3;

use crate::layer::CollisionFilter;
use crate::world::StaticId;

/// A single point of contact between a moving shape and a static one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Contact {
    /// Where on the static surface the contact is, in world space.
    pub point: Vec3,
    /// Unit normal pointing **out of the static shape, towards the moving one**.
    ///
    /// The push-out direction, which is the only convention that keeps resolution from
    /// having to remember which side it is on.
    pub normal: Vec3,
    /// How far the moving shape is inside the static one, in metres. Never negative.
    pub penetration: f32,
}

impl Contact {
    /// A contact.
    #[must_use]
    pub fn new(point: Vec3, normal: Vec3, penetration: f32) -> Self {
        Self {
            point,
            normal,
            penetration: penetration.max(0.0),
        }
    }

    /// How level the contact surface is: `normal · up`, in `[-1, 1]`.
    ///
    /// 1 is a perfectly flat floor, 0 a vertical wall, −1 a ceiling.
    #[must_use]
    pub fn levelness(&self) -> f32 {
        self.normal.dot(Vec3::Y)
    }

    /// The angle between the contact surface and horizontal, in radians.
    ///
    /// A wall is `PI/2`; a floor is 0. Clamped before `acos` so a normal that is a
    /// millionth off unit length cannot produce `NaN`.
    #[must_use]
    pub fn slope_angle(&self) -> f32 {
        self.levelness().clamp(-1.0, 1.0).acos()
    }
}

/// A contact, and which static object produced it.
///
/// The world reports these; the character controller consumes them. Keeping the identity
/// beside the geometry is what lets a debug view name the surface a player is standing on
/// without a second query.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StaticContact {
    /// The geometry of the contact.
    pub contact: Contact,
    /// Which static object it was with.
    pub id: StaticId,
    /// That object's layer and mask.
    pub filter: CollisionFilter,
}

/// How much work a query or a move did.
///
/// Counted rather than estimated. "The broad phase rejects almost everything" is exactly
/// the kind of claim that quietly stops being true when a level grows, so the benchmark
/// reads these instead of trusting the design.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct QueryStats {
    /// Static objects the broad phase offered to the narrow phase.
    pub broad_candidates: u32,
    /// Exact shape tests performed.
    pub narrow_tests: u32,
    /// Tests that produced a contact.
    pub contacts: u32,
}

impl QueryStats {
    /// Zeroed.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            broad_candidates: 0,
            narrow_tests: 0,
            contacts: 0,
        }
    }

    /// Adds another set of counts into this one.
    pub fn accumulate(&mut self, other: Self) {
        self.broad_candidates += other.broad_candidates;
        self.narrow_tests += other.narrow_tests;
        self.contacts += other.contacts;
    }

    /// Sets every count back to zero, without allocating anything.
    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flat_floor_is_level_and_a_wall_is_not() {
        let floor = Contact::new(Vec3::ZERO, Vec3::Y, 0.1);
        let wall = Contact::new(Vec3::ZERO, Vec3::X, 0.1);
        let ceiling = Contact::new(Vec3::ZERO, -Vec3::Y, 0.1);

        assert_eq!(floor.levelness(), 1.0);
        assert_eq!(wall.levelness(), 0.0);
        assert_eq!(ceiling.levelness(), -1.0);

        assert!(floor.slope_angle().abs() < 1e-6);
        assert!((wall.slope_angle() - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
    }

    #[test]
    fn a_slope_angle_matches_the_tilt_it_was_built_from() {
        let angle = 35.0_f32.to_radians();
        let normal = Vec3::new(0.0, angle.cos(), angle.sin());
        let contact = Contact::new(Vec3::ZERO, normal, 0.0);
        assert!((contact.slope_angle() - angle).abs() < 1e-4);
    }

    #[test]
    fn a_slightly_denormalized_normal_does_not_produce_a_nan_angle() {
        // acos(1.0000001) is NaN, and a NaN slope angle silently makes every comparison
        // false — which reads as "the player is never grounded".
        let contact = Contact::new(Vec3::ZERO, Vec3::new(0.0, 1.000_001, 0.0), 0.0);
        assert!(contact.slope_angle().is_finite());
    }

    #[test]
    fn penetration_is_never_negative() {
        assert_eq!(Contact::new(Vec3::ZERO, Vec3::Y, -3.0).penetration, 0.0);
    }

    #[test]
    fn statistics_accumulate_and_reset() {
        let mut stats = QueryStats::new();
        stats.accumulate(QueryStats {
            broad_candidates: 10,
            narrow_tests: 4,
            contacts: 1,
        });
        stats.accumulate(QueryStats {
            broad_candidates: 5,
            narrow_tests: 2,
            contacts: 0,
        });
        assert_eq!(stats.broad_candidates, 15);
        assert_eq!(stats.narrow_tests, 6);
        assert_eq!(stats.contacts, 1);

        stats.reset();
        assert_eq!(stats, QueryStats::new());
    }
}
