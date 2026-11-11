//! Where a player starts, and which way they are looking when they get there.
//!
//! ```text
//!         ↑ yaw: the way the player faces
//!         │
//!        [▲]  position
//! ```
//!
//! # Why this is a list and not a field
//!
//! [`crate::scene::SpawnPoints`] has held one player position since the arena was the only
//! level in the game. It is a *field of the scene*: something the layout function that built
//! the arena filled in, alongside where the dummies stand. Nothing could select it, move it or
//! see it, because there was nothing in the level to point at.
//!
//! A spawn point is the third thing a level *has* that an author places, after water and the
//! ground — and like water it wants every verb a placement already has: click to select, drag
//! to move, `R` to turn, `X` to delete, `Z` to take it back. So it is a list of records the
//! editor's tagged selection can name, which is the mechanism Phase 8 introduced water with
//! and the reason that refactor was done as a shape rather than as a special case.
//!
//! # What did not change
//!
//! [`SpawnPoints::player`](crate::scene::SpawnPoints::player) is still there and is still the
//! answer for every level that has no spawn point in it — which is every level built before
//! this module existed, and every built-in layout. See
//! [`EnvironmentScene::player_spawn`](crate::EnvironmentScene::player_spawn): one question,
//! one answer, and nothing outside this crate has to know which of the two it came from.
//!
//! # A yaw and not a rotation
//!
//! Everything in this game stands on the ground. A spawn point that carried a quaternion could
//! describe a player starting upside down, which is not a level anybody wants to be able to
//! author by accident — and the character controller has one facing angle to be given, not
//! three.

use engine_core::math::Vec3;

/// Somewhere a player can start.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpawnPoint {
    /// Where the player stands. `y` is their feet.
    pub position: Vec3,
    /// Which way they face, in radians about +Y.
    ///
    /// The same convention every placement's yaw uses, and the same one
    /// `engine_game::movement::Body::facing_yaw` is written in, so the number an author sets
    /// in the editor is the number the character is turned to.
    pub yaw: f32,
}

impl SpawnPoint {
    /// A spawn point facing along -Z, which is the way the camera looks.
    #[must_use]
    pub fn new(position: Vec3) -> Self {
        Self { position, yaw: 0.0 }
    }

    /// The same spawn point, turned.
    #[must_use]
    pub fn with_yaw(mut self, yaw: f32) -> Self {
        self.yaw = yaw;
        self
    }

    /// The direction a player standing here is looking.
    ///
    /// Yaw zero looks along -Z, which is where the camera looks and which is what makes a
    /// spawn point authored without turning it face the same way as an unturned placement.
    #[must_use]
    pub fn forward(&self) -> Vec3 {
        let (sin, cos) = self.yaw.sin_cos();
        Vec3::new(-sin, 0.0, -cos)
    }

    /// Whether the numbers describe somewhere a player could actually be put.
    ///
    /// What a loader checks. Everything that reads a spawn point puts a character at it, and
    /// a character at a position that is not a number is a character nothing can find again.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.position.x.is_finite()
            && self.position.y.is_finite()
            && self.position.z.is_finite()
            && self.yaw.is_finite()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_PI_2;

    #[test]
    fn an_unturned_spawn_point_looks_the_way_the_camera_does() {
        let spawn = SpawnPoint::new(Vec3::new(3.0, 0.0, -4.0));
        let forward = spawn.forward();
        assert!(forward.z < -0.99, "{forward:?}");
        assert!(forward.x.abs() < 1e-6, "{forward:?}");
    }

    #[test]
    fn a_quarter_turn_looks_along_minus_x() {
        // The same handedness every placement's yaw uses: turning by +90 degrees takes -Z to
        // -X. A spawn point that disagreed with a placement about which way "turned" is would
        // be a spawn point an author cannot line up with a doorway.
        let spawn = SpawnPoint::new(Vec3::ZERO).with_yaw(FRAC_PI_2);
        let forward = spawn.forward();
        assert!(forward.x < -0.99, "{forward:?}");
        assert!(forward.z.abs() < 1e-6, "{forward:?}");
    }

    #[test]
    fn a_spawn_point_that_is_not_a_place_is_refused() {
        let mut spawn = SpawnPoint::new(Vec3::ZERO);
        assert!(spawn.is_valid());
        spawn.position.y = f32::NAN;
        assert!(!spawn.is_valid());
        spawn.position.y = 0.0;
        spawn.yaw = f32::INFINITY;
        assert!(!spawn.is_valid());
    }
}
