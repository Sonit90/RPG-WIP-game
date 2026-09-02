//! The editor's camera: a free-flying eye, not a character's shoulder.
//!
//! ```text
//!   W/S  forward and back along the look direction
//!   A/D  strafe
//!   Q/E  down and up, in world space, always
//!  SHIFT four times the speed
//!  right-drag  turn
//! ```
//!
//! # Why not the third-person camera
//!
//! [`ThirdPersonCamera`](engine_presentation::ThirdPersonCamera) orbits a focus point at a
//! fixed distance, because it is watching a character. An editor's camera has no character to
//! watch: it goes where the author is building, including inside a wall to check its footing
//! and two hundred metres up to see the whole layout. Bending the orbit camera into a fly
//! camera would give the game's camera a mode that gameplay must never enter.

use engine_core::input::{InputState, KeyCode, MouseButton};
use engine_core::math::{Quat, Vec3};
use engine_render_data::{Camera, Projection, RenderDataError};

/// Metres per second at a walk.
const SPEED: f32 = 14.0;
/// How much faster `SHIFT` makes it.
const SPRINT: f32 = 4.0;
/// Radians per pixel of mouse movement.
const SENSITIVITY: f32 = 0.0032;
/// How far the camera can look up or down. Just short of straight, so the horizon never
/// flips and the up vector never degenerates.
const PITCH_LIMIT: f32 = 1.5533;
/// Vertical field of view, radians. Wider than the game's, because an editor is looking at a
/// layout rather than at a fight.
///
/// Public because the editor's own overlay has to size things the projection would have sized
/// for it: a selected object behind the camera projects nowhere, and its silhouette is drawn
/// at the size it *would* have been — which needs the same focal length the camera uses.
pub const FOV_Y: f32 = 1.20;
/// Near and far planes. The far plane is the game's, so what is drawn here is what is drawn
/// there.
const NEAR: f32 = 0.1;
/// See [`NEAR`].
const FAR: f32 = 1200.0;

/// A camera the author flies.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlyCamera {
    /// Where the eye is.
    pub position: Vec3,
    /// Rotation about +Y, radians.
    pub yaw: f32,
    /// Rotation about the camera's right, radians. Clamped short of vertical.
    pub pitch: f32,
}

impl Default for FlyCamera {
    fn default() -> Self {
        Self {
            position: Vec3::new(0.0, 18.0, 34.0),
            yaw: 0.0,
            pitch: -0.45,
        }
    }
}

impl FlyCamera {
    /// A camera looking at `focus` from a comfortable height and distance behind it.
    ///
    /// Where the editor starts: at the player's position, high enough to see the ground
    /// around it, so entering edit mode does not lose the place the author was standing in.
    #[must_use]
    pub fn overlooking(focus: Vec3) -> Self {
        Self {
            position: focus + Vec3::new(0.0, 16.0, 26.0),
            yaw: 0.0,
            pitch: -0.55,
        }
    }

    /// The camera's rotation.
    #[must_use]
    pub fn rotation(&self) -> Quat {
        Quat::from_yaw_pitch_roll(self.yaw, self.pitch, 0.0)
    }

    /// Which way it is looking.
    #[must_use]
    pub fn forward(&self) -> Vec3 {
        self.rotation().rotate(Vec3::new(0.0, 0.0, -1.0))
    }

    /// Which way is right of it.
    #[must_use]
    pub fn right(&self) -> Vec3 {
        self.rotation().rotate(Vec3::new(1.0, 0.0, 0.0))
    }

    /// Turns the camera by a mouse movement in pixels.
    pub fn turn(&mut self, delta_x: f32, delta_y: f32) {
        self.yaw -= delta_x * SENSITIVITY;
        self.pitch = (self.pitch - delta_y * SENSITIVITY).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }

    /// Applies one frame of input.
    ///
    /// Looking is on the **right** button, the same button that turns the camera during play,
    /// so the one gesture an author already has in their hands means the same thing here. The
    /// left button is left entirely to placing and selecting.
    pub fn update(&mut self, input: &InputState, seconds: f32) {
        if input.is_mouse_button_down(MouseButton::Right) {
            let (dx, dy) = input.mouse_delta();
            self.turn(dx as f32, dy as f32);
        }

        let mut motion = Vec3::ZERO;
        if input.is_key_down(KeyCode::W) {
            motion += self.forward();
        }
        if input.is_key_down(KeyCode::S) {
            motion -= self.forward();
        }
        if input.is_key_down(KeyCode::D) {
            motion += self.right();
        }
        if input.is_key_down(KeyCode::A) {
            motion -= self.right();
        }
        // World up rather than the camera's, so rising while looking down does not drift
        // forwards — an editor climbing to look at a roof wants to climb, not to arc.
        if input.is_key_down(KeyCode::E) {
            motion += Vec3::Y;
        }
        if input.is_key_down(KeyCode::Q) {
            motion -= Vec3::Y;
        }

        if motion.length_squared() > 1e-6 {
            let sprint = if input.is_key_down(KeyCode::LeftShift)
                || input.is_key_down(KeyCode::RightShift)
            {
                SPRINT
            } else {
                1.0
            };
            self.position += motion.normalize() * (SPEED * sprint * seconds);
        }
    }

    /// Writes this camera into the renderer's.
    ///
    /// # Errors
    ///
    /// [`RenderDataError::InvalidConfig`] for a degenerate aspect ratio, which a minimized
    /// window produces.
    pub fn apply(&self, camera: &mut Camera, aspect: f32) -> Result<(), RenderDataError> {
        camera.set_projection(Projection::Perspective {
            fov_y: FOV_Y,
            aspect,
            near: NEAR,
            far: FAR,
        })?;
        camera.set_position(self.position);
        camera.set_rotation(self.rotation());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looking_up_and_down_stops_short_of_vertical() {
        let mut camera = FlyCamera::default();
        for _ in 0..1_000 {
            camera.turn(0.0, -100.0);
        }
        assert!(camera.pitch <= PITCH_LIMIT);
        assert!(
            camera.forward().y < 0.9999,
            "a camera looking exactly up has no usable up vector"
        );

        for _ in 0..2_000 {
            camera.turn(0.0, 100.0);
        }
        assert!(camera.pitch >= -PITCH_LIMIT);
    }

    #[test]
    fn turning_right_looks_right() {
        let mut camera = FlyCamera {
            position: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
        };
        let before = camera.forward();
        assert!(
            (before - Vec3::new(0.0, 0.0, -1.0)).length() < 1e-4,
            "-Z is forward"
        );

        camera.turn(200.0, 0.0);
        let after = camera.forward();
        // Right of a camera looking down -Z is +X, and this is the game's own sign: the
        // third-person camera subtracts the same delta from its yaw, so the gesture means
        // the same thing in both.
        assert!(after.x > 0.0, "moving the mouse right turns the view right");
        assert!(
            (camera.right().y).abs() < 1e-4,
            "the right vector stays level whatever the pitch"
        );
    }

    #[test]
    fn rising_while_looking_down_does_not_drift_forwards() {
        // World up, not the camera's: an author climbing to see over a wall would otherwise
        // slide backwards over it.
        let camera = FlyCamera {
            position: Vec3::ZERO,
            yaw: 0.0,
            pitch: -1.0,
        };
        assert_eq!(Vec3::Y, Vec3::new(0.0, 1.0, 0.0));
        assert!(camera.forward().y < 0.0, "it is looking down");
    }
}
