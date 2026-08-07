//! A free-fly developer camera.
//!
//! This is a *controller*: it reads input and drives an [`engine_renderer::Camera`] by
//! setting a position and a rotation. The renderer's camera stays what it is — a view and a
//! projection — and learns nothing about keys or mice. A third-person player camera is a
//! different controller written against the same seam, and is not this.

use engine_core::input::{InputState, KeyCode};
use engine_core::math::{Quat, Vec3};
use engine_renderer::error::RendererError;
use engine_renderer::{Camera, DEFAULT_FAR, DEFAULT_NEAR, Projection};

/// Metres per second at the base movement speed.
const BASE_SPEED: f32 = 18.0;

/// Multiplier while shift is held.
const FAST_MULTIPLIER: f32 = 6.0;

/// Multiplier while control is held.
const SLOW_MULTIPLIER: f32 = 0.2;

/// Radians of rotation per pixel of mouse movement.
const MOUSE_SENSITIVITY: f32 = 0.0022;

/// How far the pitch may travel from level.
///
/// Just under a right angle: at exactly straight up the yaw axis and the view direction
/// coincide and the camera rolls unpredictably as the mouse moves sideways.
const PITCH_LIMIT: f32 = std::f32::consts::FRAC_PI_2 - 0.01;

/// Narrowest vertical field of view the wheel can reach: 20°.
const MIN_FOV: f32 = std::f32::consts::PI / 9.0;

/// Widest vertical field of view the wheel can reach: 100°.
const MAX_FOV: f32 = std::f32::consts::PI * 5.0 / 9.0;

/// Radians of field of view per wheel notch: 4°.
const ZOOM_PER_NOTCH: f32 = std::f32::consts::PI / 45.0;

/// Position and orientation of a free-fly camera, as yaw, pitch and a point.
///
/// Yaw and pitch are stored rather than a quaternion so no roll can accumulate: rebuilding
/// the rotation from two angles every frame makes an un-levelled horizon impossible, which
/// is the failure mode of integrating mouse deltas straight into a quaternion.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlyCamera {
    /// Where the camera is, in world space.
    pub position: Vec3,
    /// Rotation about +Y, in radians. Zero looks down −Z.
    pub yaw: f32,
    /// Rotation about the camera's right, in radians. Positive looks up.
    pub pitch: f32,
    /// Current vertical field of view, in radians.
    pub fov_y: f32,
}

impl Default for FlyCamera {
    fn default() -> Self {
        Self::START
    }
}

impl FlyCamera {
    /// The opening shot: above and behind the arena, looking slightly down at it.
    pub const START: Self = Self {
        position: Vec3::new(0.0, 13.0, 58.0),
        yaw: 0.0,
        pitch: -0.17,
        fov_y: engine_renderer::DEFAULT_FOV_Y,
    };

    /// A view of the stress field from its edge, where most of it is off screen.
    ///
    /// The point of the stress mode is that culling has something to reject, which only
    /// happens when the camera is *inside* a large distribution rather than looking at all
    /// of it from outside.
    pub const STRESS: Self = Self {
        position: Vec3::new(0.0, 30.0, 0.0),
        yaw: 0.0,
        pitch: -0.1,
        fov_y: engine_renderer::DEFAULT_FOV_Y,
    };

    /// The camera's rotation, rebuilt from yaw and pitch.
    #[must_use]
    pub fn rotation(&self) -> Quat {
        Quat::from_yaw_pitch_roll(self.yaw, self.pitch, 0.0)
    }

    /// The direction the camera looks.
    #[must_use]
    pub fn forward(&self) -> Vec3 {
        self.rotation().rotate(-Vec3::Z)
    }

    /// The camera's right.
    #[must_use]
    pub fn right(&self) -> Vec3 {
        self.rotation().rotate(Vec3::X)
    }

    /// Applies a mouse movement, in pixels.
    ///
    /// Screen Y grows downward, so a downward mouse movement lowers the pitch.
    pub fn rotate(&mut self, delta_x: f32, delta_y: f32) {
        self.yaw -= delta_x * MOUSE_SENSITIVITY;
        self.pitch = (self.pitch - delta_y * MOUSE_SENSITIVITY).clamp(-PITCH_LIMIT, PITCH_LIMIT);
        // Kept in (−pi, pi] so a long session cannot lose precision by accumulating turns.
        self.yaw = self.yaw.rem_euclid(std::f32::consts::TAU);
    }

    /// Applies wheel notches as a field-of-view change.
    ///
    /// Rolling away from the user narrows the view, which is what "zoom in" means.
    pub fn zoom(&mut self, notches: f32) {
        self.fov_y = (self.fov_y - notches * ZOOM_PER_NOTCH).clamp(MIN_FOV, MAX_FOV);
    }

    /// Moves the camera for one frame of held keys.
    ///
    /// Movement is along the camera's own axes except for Q and E, which are world-vertical:
    /// a developer camera that drifts sideways while rising is harder to place than one that
    /// simply goes up.
    pub fn move_by_input(&mut self, input: &InputState, delta_seconds: f32) {
        let mut direction = Vec3::ZERO;
        let forward = self.forward();
        let right = self.right();

        if input.is_key_down(KeyCode::W) {
            direction += forward;
        }
        if input.is_key_down(KeyCode::S) {
            direction -= forward;
        }
        if input.is_key_down(KeyCode::D) {
            direction += right;
        }
        if input.is_key_down(KeyCode::A) {
            direction -= right;
        }
        if input.is_key_down(KeyCode::E) {
            direction += Vec3::Y;
        }
        if input.is_key_down(KeyCode::Q) {
            direction -= Vec3::Y;
        }

        if direction.length_squared() <= f32::EPSILON {
            return;
        }

        let mut speed = BASE_SPEED;
        if input.is_key_down(KeyCode::LeftShift) || input.is_key_down(KeyCode::RightShift) {
            speed *= FAST_MULTIPLIER;
        }
        if input.is_key_down(KeyCode::LeftControl) || input.is_key_down(KeyCode::RightControl) {
            speed *= SLOW_MULTIPLIER;
        }

        // Normalized, so moving diagonally is not faster than moving straight ahead.
        self.position += direction.normalize() * (speed * delta_seconds);
    }

    /// Writes this controller's state into a renderer camera.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] if the aspect ratio is degenerate, which a
    /// minimized window produces.
    pub fn apply(&self, camera: &mut Camera, aspect: f32) -> Result<(), RendererError> {
        camera.set_projection(Projection::Perspective {
            fov_y: self.fov_y,
            aspect,
            near: DEFAULT_NEAR,
            far: DEFAULT_FAR,
        })?;
        camera.set_position(self.position);
        camera.set_rotation(self.rotation());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPSILON: f32 = 1e-3;

    fn input_with(keys: &[KeyCode]) -> InputState {
        let mut state = InputState::new();
        for key in keys {
            state.apply(engine_core::input::InputEvent::KeyDown {
                key: *key,
                repeat: false,
            });
        }
        state
    }

    #[test]
    fn a_fresh_camera_looks_roughly_down_minus_z() {
        let camera = FlyCamera {
            pitch: 0.0,
            ..FlyCamera::START
        };
        let forward = camera.forward();
        assert!((forward.z + 1.0).abs() < EPSILON, "{forward:?}");
    }

    #[test]
    fn w_moves_along_the_view_direction() {
        let mut camera = FlyCamera {
            pitch: 0.0,
            ..FlyCamera::START
        };
        let start = camera.position;
        camera.move_by_input(&input_with(&[KeyCode::W]), 1.0);
        assert!(camera.position.z < start.z, "W must move forward");
        assert!((camera.position.x - start.x).abs() < EPSILON);
    }

    #[test]
    fn diagonal_movement_is_not_faster_than_straight_movement() {
        let mut straight = FlyCamera::START;
        straight.move_by_input(&input_with(&[KeyCode::W]), 1.0);
        let mut diagonal = FlyCamera::START;
        diagonal.move_by_input(&input_with(&[KeyCode::W, KeyCode::D]), 1.0);

        let straight_distance = (straight.position - FlyCamera::START.position).length();
        let diagonal_distance = (diagonal.position - FlyCamera::START.position).length();
        assert!((straight_distance - diagonal_distance).abs() < EPSILON);
    }

    #[test]
    fn opposite_keys_cancel_instead_of_drifting() {
        let mut camera = FlyCamera::START;
        camera.move_by_input(&input_with(&[KeyCode::W, KeyCode::S]), 1.0);
        assert_eq!(camera.position, FlyCamera::START.position);
    }

    #[test]
    fn q_and_e_move_vertically_regardless_of_where_the_camera_looks() {
        let mut camera = FlyCamera {
            pitch: 0.9,
            yaw: 1.3,
            ..FlyCamera::START
        };
        let start = camera.position;
        camera.move_by_input(&input_with(&[KeyCode::E]), 1.0);
        assert!(camera.position.y > start.y);
        assert!((camera.position.x - start.x).abs() < EPSILON);
        assert!((camera.position.z - start.z).abs() < EPSILON);
    }

    #[test]
    fn shift_moves_further_than_control_in_the_same_time() {
        let mut fast = FlyCamera::START;
        fast.move_by_input(&input_with(&[KeyCode::W, KeyCode::LeftShift]), 1.0);
        let mut slow = FlyCamera::START;
        slow.move_by_input(&input_with(&[KeyCode::W, KeyCode::LeftControl]), 1.0);

        let fast_distance = (fast.position - FlyCamera::START.position).length();
        let slow_distance = (slow.position - FlyCamera::START.position).length();
        assert!(fast_distance > slow_distance * 10.0);
    }

    #[test]
    fn pitch_stops_short_of_straight_up_and_straight_down() {
        // At exactly vertical the yaw axis and the view direction coincide and the camera
        // rolls as the mouse moves sideways.
        let mut camera = FlyCamera::START;
        camera.rotate(0.0, -100_000.0);
        assert!(camera.pitch < PITCH_LIMIT + EPSILON && camera.pitch > 0.0);
        camera.rotate(0.0, 200_000.0);
        assert!(camera.pitch > -PITCH_LIMIT - EPSILON && camera.pitch < 0.0);
    }

    #[test]
    fn yaw_stays_bounded_across_many_turns() {
        let mut camera = FlyCamera::START;
        for _ in 0..1000 {
            camera.rotate(500.0, 0.0);
        }
        assert!(camera.yaw.is_finite());
        assert!((0.0..std::f32::consts::TAU).contains(&camera.yaw));
    }

    #[test]
    fn moving_the_mouse_right_turns_the_view_right() {
        let mut camera = FlyCamera {
            pitch: 0.0,
            ..FlyCamera::START
        };
        camera.rotate(100.0, 0.0);
        // Looking down −Z, turning right sends the view towards +X.
        assert!(camera.forward().x > 0.0, "{:?}", camera.forward());
    }

    #[test]
    fn the_wheel_zooms_between_its_limits() {
        let mut camera = FlyCamera::START;
        camera.zoom(100.0);
        assert!((camera.fov_y - MIN_FOV).abs() < 1e-5);
        camera.zoom(-1000.0);
        assert!((camera.fov_y - MAX_FOV).abs() < 1e-5);
    }

    #[test]
    fn applying_the_controller_agrees_with_the_camera_it_drives() {
        let Ok(mut camera) = Camera::new(Projection::perspective(16.0 / 9.0)) else {
            panic!("a 16:9 perspective projection is valid");
        };
        let controller = FlyCamera {
            position: Vec3::new(4.0, 5.0, 6.0),
            yaw: 0.7,
            pitch: -0.3,
            fov_y: engine_renderer::DEFAULT_FOV_Y,
        };
        let Ok(()) = controller.apply(&mut camera, 16.0 / 9.0) else {
            panic!("a 16:9 aspect ratio is valid");
        };

        assert_eq!(camera.position(), controller.position);
        let forward = camera.forward();
        let expected = controller.forward();
        assert!((forward.x - expected.x).abs() < EPSILON, "{forward:?}");
        assert!((forward.y - expected.y).abs() < EPSILON, "{forward:?}");
        assert!((forward.z - expected.z).abs() < EPSILON, "{forward:?}");
    }

    #[test]
    fn a_degenerate_viewport_is_rejected_rather_than_producing_infinities() {
        let Ok(mut camera) = Camera::new(Projection::perspective(1.0)) else {
            panic!("a square perspective projection is valid");
        };
        assert!(FlyCamera::START.apply(&mut camera, 0.0).is_err());
    }
}
