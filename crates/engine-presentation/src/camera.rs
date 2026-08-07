//! The third-person camera controller.
//!
//! ```text
//!          eye
//!            \   distance
//!             \
//!              \  pitch
//!               v
//!             focus  = player feet + height
//!               |
//!             (P)
//! ```
//!
//! A *controller*, like `engine-showcase`'s free-fly camera: it reads intent and drives an
//! [`engine_render_data::Camera`] by setting a position and a rotation. The camera itself
//! stays a view and a projection and learns nothing about a player.
//!
//! It lives in presentation rather than in `engine-game` deliberately. Where the camera is
//! does not change what the simulation does — it only changes what movement means, and that
//! is carried in the command as a yaw. A future spectator, replay or cinematic camera is
//! another controller against the same seam and requires no gameplay change.

use engine_core::math::{Quat, Vec3};
use engine_render_data::error::RenderDataError;
use engine_render_data::{Camera, DEFAULT_FAR, DEFAULT_FOV_Y, DEFAULT_NEAR, Projection};

/// Radians of rotation per pixel of mouse movement.
const MOUSE_SENSITIVITY: f32 = 0.0028;

/// Lowest the camera may look from, in radians. Negative looks down at the character.
const MIN_PITCH: f32 = -1.2;

/// Highest the camera may look from. Short of straight up, where yaw and view coincide and
/// the image rolls as the mouse moves sideways.
const MAX_PITCH: f32 = 1.1;

/// Closest the camera may orbit, in metres.
const MIN_DISTANCE: f32 = 2.5;

/// Furthest the camera may orbit, in metres.
const MAX_DISTANCE: f32 = 25.0;

/// Metres of orbit distance per wheel notch.
const ZOOM_PER_NOTCH: f32 = 1.2;

/// Where the camera looks relative to the character's feet, in metres.
///
/// Roughly the chest: focusing on the feet puts the character in the top half of the screen
/// and the floor in the other half, which is where a third-person camera looks wrong
/// without being obviously misconfigured.
const FOCUS_HEIGHT: f32 = 1.4;

/// How quickly the focus catches up with the character, in units of e-folds per second.
///
/// The camera lags the character slightly, which is what makes movement read as momentum
/// rather than as the world sliding. Frame-rate independent: the smoothing is exponential
/// in elapsed time, not a fraction per frame — a per-frame fraction would follow six times
/// as fast at 360 FPS as at 60.
const FOLLOW_RATE: f32 = 12.0;

/// An orbiting follow camera.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThirdPersonCamera {
    /// Rotation about +Y, radians. Zero looks down −Z.
    pub yaw: f32,
    /// Rotation about the camera's right, radians. Negative looks down.
    pub pitch: f32,
    /// How far behind the character the eye sits, metres.
    pub distance: f32,
    /// Vertical field of view, radians.
    pub fov_y: f32,
    /// The smoothed point the camera looks at.
    focus: Vec3,
}

impl Default for ThirdPersonCamera {
    fn default() -> Self {
        Self::START
    }
}

impl ThirdPersonCamera {
    /// Behind and slightly above the character, looking down at it.
    pub const START: Self = Self {
        yaw: 0.0,
        pitch: -0.22,
        distance: 8.0,
        fov_y: DEFAULT_FOV_Y,
        focus: Vec3::ZERO,
    };

    /// A camera already snapped to a character at `position`.
    #[must_use]
    pub fn following(position: Vec3) -> Self {
        let mut camera = Self::START;
        camera.snap_to(position);
        camera
    }

    /// The point the camera is looking at.
    #[must_use]
    pub fn focus(&self) -> Vec3 {
        self.focus
    }

    /// The camera's orientation.
    #[must_use]
    pub fn rotation(&self) -> Quat {
        Quat::from_yaw_pitch_roll(self.yaw, self.pitch, 0.0)
    }

    /// Where the eye is, in world space.
    ///
    /// Behind the focus along the view direction, which is what makes zooming and pitching
    /// orbit the character rather than move it across the screen.
    #[must_use]
    pub fn eye(&self) -> Vec3 {
        self.focus + self.rotation().rotate(Vec3::Z) * self.distance
    }

    /// Applies a mouse movement, in pixels.
    ///
    /// Screen Y grows downward, so moving the mouse down lowers the pitch.
    pub fn rotate(&mut self, delta_x: f32, delta_y: f32) {
        self.yaw = (self.yaw - delta_x * MOUSE_SENSITIVITY).rem_euclid(std::f32::consts::TAU);
        self.pitch = (self.pitch - delta_y * MOUSE_SENSITIVITY).clamp(MIN_PITCH, MAX_PITCH);
    }

    /// Applies wheel notches as an orbit distance change.
    ///
    /// Rolling away from the user pulls the camera in, which is what "zoom in" means.
    pub fn zoom(&mut self, notches: f32) {
        self.distance =
            (self.distance - notches * ZOOM_PER_NOTCH).clamp(MIN_DISTANCE, MAX_DISTANCE);
    }

    /// Puts the focus exactly on the character, with no smoothing.
    ///
    /// For the first frame and after a teleport: smoothing from wherever the camera was
    /// would fly it across the arena.
    pub fn snap_to(&mut self, position: Vec3) {
        self.focus = position + Vec3::new(0.0, FOCUS_HEIGHT, 0.0);
    }

    /// Moves the focus towards the character for one frame.
    ///
    /// Driven by the *render* delta rather than the simulation step, and that is correct:
    /// the camera is presentation, it is expected to move every frame, and interpolating it
    /// at the render rate is what stops a 60 Hz simulation from looking like 60 FPS.
    pub fn follow(&mut self, position: Vec3, seconds: f32) {
        let target = position + Vec3::new(0.0, FOCUS_HEIGHT, 0.0);
        if seconds <= 0.0 {
            self.focus = target;
            return;
        }
        // Exponential smoothing: the fraction covered depends on elapsed time, so the
        // camera behaves identically at 60 and at 1400 FPS.
        let blend = 1.0 - (-FOLLOW_RATE * seconds).exp();
        self.focus = self.focus.lerp(target, blend.clamp(0.0, 1.0));
    }

    /// Writes this controller's state into a renderer camera.
    ///
    /// # Errors
    ///
    /// Returns [`RenderDataError::InvalidConfig`] if the aspect ratio is degenerate, which
    /// a minimized window produces.
    pub fn apply(&self, camera: &mut Camera, aspect: f32) -> Result<(), RenderDataError> {
        camera.set_projection(Projection::Perspective {
            fov_y: self.fov_y,
            aspect,
            near: DEFAULT_NEAR,
            far: DEFAULT_FAR,
        })?;
        camera.set_position(self.eye());
        camera.set_rotation(self.rotation());
        Ok(())
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    const EPSILON: f32 = 1e-3;

    fn camera() -> Camera {
        Camera::new(Projection::perspective(16.0 / 9.0)).expect("a valid projection")
    }

    #[test]
    fn the_eye_sits_behind_and_above_the_character() {
        let controller = ThirdPersonCamera::following(Vec3::ZERO);
        let eye = controller.eye();
        assert!(eye.z > 0.0, "the camera is behind a character facing -Z");
        assert!(eye.y > 0.0, "and above it");
        assert!((eye.x).abs() < EPSILON);
        assert!(
            (eye - controller.focus()).length() - controller.distance < EPSILON,
            "the eye is exactly `distance` from the focus"
        );
    }

    #[test]
    fn the_camera_looks_at_the_character() {
        let controller = ThirdPersonCamera::following(Vec3::new(3.0, 0.0, -4.0));
        let mut view = camera();
        controller
            .apply(&mut view, 16.0 / 9.0)
            .expect("a valid aspect");

        // The focus must land in front of the camera, on its axis.
        let seen = view.view().transform_point(controller.focus());
        assert!(seen.x.abs() < EPSILON && seen.y.abs() < EPSILON, "{seen:?}");
        assert!(seen.z < 0.0, "the focus is behind the camera: {seen:?}");
    }

    #[test]
    fn yaw_orbits_the_character_rather_than_moving_it() {
        let mut controller = ThirdPersonCamera::following(Vec3::new(10.0, 0.0, 10.0));
        let focus = controller.focus();
        let before = controller.eye();
        controller.rotate(200.0, 0.0);

        assert_eq!(controller.focus(), focus, "the focus moved");
        assert!(
            ((controller.eye() - focus).length() - (before - focus).length()).abs() < EPSILON,
            "the orbit radius changed"
        );
        assert!((controller.eye() - before).length() > 0.5, "nothing moved");
    }

    #[test]
    fn pitch_stops_short_of_straight_up_and_straight_down() {
        let mut controller = ThirdPersonCamera::START;
        controller.rotate(0.0, -100_000.0);
        assert!((controller.pitch - MAX_PITCH).abs() < EPSILON);
        controller.rotate(0.0, 200_000.0);
        assert!((controller.pitch - MIN_PITCH).abs() < EPSILON);
    }

    #[test]
    fn yaw_stays_bounded_across_many_turns() {
        let mut controller = ThirdPersonCamera::START;
        for _ in 0..1_000 {
            controller.rotate(500.0, 0.0);
        }
        assert!(controller.yaw.is_finite());
        assert!((0.0..std::f32::consts::TAU).contains(&controller.yaw));
    }

    #[test]
    fn the_wheel_zooms_between_its_limits() {
        let mut controller = ThirdPersonCamera::START;
        controller.zoom(100.0);
        assert!((controller.distance - MIN_DISTANCE).abs() < EPSILON);
        controller.zoom(-1_000.0);
        assert!((controller.distance - MAX_DISTANCE).abs() < EPSILON);
    }

    #[test]
    fn following_catches_up_without_ever_overshooting() {
        let mut controller = ThirdPersonCamera::following(Vec3::ZERO);
        let target = Vec3::new(0.0, 0.0, -20.0);
        let mut previous = controller.focus().z;

        for _ in 0..600 {
            controller.follow(target, 1.0 / 60.0);
            let z = controller.focus().z;
            assert!(z <= previous + EPSILON, "the focus moved backwards");
            assert!(z >= target.z - EPSILON, "the focus overshot: {z}");
            previous = z;
        }
        assert!((controller.focus().z - target.z).abs() < 0.01, "{previous}");
    }

    #[test]
    fn following_is_frame_rate_independent() {
        // The property exponential smoothing exists for: the same elapsed time must produce
        // the same camera at 60 and at 480 FPS, or the camera lags differently on a faster
        // machine.
        let target = Vec3::new(0.0, 0.0, -10.0);
        let mut slow = ThirdPersonCamera::following(Vec3::ZERO);
        let mut fast = ThirdPersonCamera::following(Vec3::ZERO);

        for _ in 0..6 {
            slow.follow(target, 1.0 / 60.0);
        }
        for _ in 0..48 {
            fast.follow(target, 1.0 / 480.0);
        }

        assert!(
            (slow.focus().z - fast.focus().z).abs() < 0.02,
            "{:?} vs {:?}",
            slow.focus(),
            fast.focus()
        );
    }

    #[test]
    fn snapping_skips_the_smoothing() {
        let mut controller = ThirdPersonCamera::following(Vec3::ZERO);
        controller.snap_to(Vec3::new(100.0, 0.0, 100.0));
        assert!((controller.focus().x - 100.0).abs() < EPSILON);
        assert!((controller.focus().z - 100.0).abs() < EPSILON);
    }

    #[test]
    fn a_degenerate_viewport_is_rejected_rather_than_producing_infinities() {
        let mut view = camera();
        assert!(ThirdPersonCamera::START.apply(&mut view, 0.0).is_err());
    }

    #[test]
    fn a_zero_delta_frame_places_the_focus_exactly() {
        // The first frame has a zero delta, and a camera that refused to move on it would
        // start the game looking at the origin from wherever it was constructed.
        let mut controller = ThirdPersonCamera::START;
        controller.follow(Vec3::new(5.0, 0.0, 5.0), 0.0);
        assert!((controller.focus().x - 5.0).abs() < EPSILON);
    }
}
