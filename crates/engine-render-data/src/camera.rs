//! View and projection.
//!
//! A [`Camera`] is position, orientation and a projection — nothing else. It does not
//! follow anything, collide with anything or smooth anything: a third-person camera that
//! orbits a character is a *controller*, it belongs to the game, and it drives this type by
//! setting a position and a rotation. The renderer must not learn what a character is.
//!
//! That sentence is why the camera lives here rather than in `engine-renderer`: the
//! controller is gameplay, and a controller that cannot name the type it drives without
//! linking D3D12 is not a seam. `engine-showcase`'s free-fly camera is the first such
//! controller and is written against exactly this API.
//!
//! Conventions come from [`engine_core::math`]: right-handed, looking down −Z, depth in
//! [0, 1].

use engine_core::math::{Mat4, Quat, Vec3};

use crate::error::RenderDataError;

/// How the camera flattens the world.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Projection {
    /// Perspective, the normal case.
    Perspective {
        /// Vertical field of view, radians.
        fov_y: f32,
        /// Width divided by height.
        aspect: f32,
        /// Near plane distance, positive.
        near: f32,
        /// Far plane distance, positive and greater than `near`.
        far: f32,
    },
    /// Orthographic, for debugging and later for shadow projections.
    Orthographic {
        /// Half of the visible height in world units.
        half_height: f32,
        /// Width divided by height.
        aspect: f32,
        /// Near plane distance.
        near: f32,
        /// Far plane distance.
        far: f32,
    },
}

/// Default vertical field of view: 60°.
///
/// A third-person game usually wants a wider horizontal view than a first-person one; 60°
/// vertical is 90° horizontal at 16:9, which is the usual starting point.
pub const DEFAULT_FOV_Y: f32 = std::f32::consts::PI / 3.0;

/// Default near plane.
///
/// Depth precision is spent between the near plane and the far plane, and the near plane
/// costs far more of it: halving `near` costs as much precision as doubling `far` twice
/// over. 0.1 m is close enough for a camera that never enters geometry.
pub const DEFAULT_NEAR: f32 = 0.1;

/// Default far plane, in metres.
pub const DEFAULT_FAR: f32 = 1000.0;

impl Projection {
    /// A perspective projection with the default field of view and planes.
    #[must_use]
    pub fn perspective(aspect: f32) -> Self {
        Self::Perspective {
            fov_y: DEFAULT_FOV_Y,
            aspect,
            near: DEFAULT_NEAR,
            far: DEFAULT_FAR,
        }
    }

    /// The aspect ratio this projection was built with.
    #[must_use]
    pub fn aspect(&self) -> f32 {
        match *self {
            Self::Perspective { aspect, .. } | Self::Orthographic { aspect, .. } => aspect,
        }
    }

    /// The near and far plane distances.
    #[must_use]
    pub fn depth_range(&self) -> (f32, f32) {
        match *self {
            Self::Perspective { near, far, .. } | Self::Orthographic { near, far, .. } => {
                (near, far)
            }
        }
    }

    /// Replaces the aspect ratio.
    fn set_aspect(&mut self, value: f32) {
        match self {
            Self::Perspective { aspect, .. } | Self::Orthographic { aspect, .. } => *aspect = value,
        }
    }

    /// Builds the projection matrix.
    #[must_use]
    pub fn matrix(&self) -> Mat4 {
        match *self {
            Self::Perspective {
                fov_y,
                aspect,
                near,
                far,
            } => Mat4::perspective(fov_y, aspect, near, far),
            Self::Orthographic {
                half_height,
                aspect,
                near,
                far,
            } => {
                let half_width = half_height * aspect;
                Mat4::orthographic(
                    -half_width,
                    half_width,
                    -half_height,
                    half_height,
                    near,
                    far,
                )
            }
        }
    }

    /// Rejects a projection that would produce a degenerate or infinite matrix.
    ///
    /// Every one of these fails silently otherwise: a zero aspect ratio produces infinities
    /// the GPU turns into an empty screen, and `near >= far` produces a division by zero in
    /// the depth range. A wrong number is easier to find at the call site than in a frame
    /// capture.
    fn validate(&self) -> Result<(), RenderDataError> {
        let reason = match *self {
            Self::Perspective {
                fov_y,
                aspect,
                near,
                far,
            } => {
                if !(fov_y > 0.0 && fov_y < std::f32::consts::PI) {
                    Some(format!("field of view must be in (0, pi), got {fov_y}"))
                } else {
                    Self::validate_common(aspect, near, far)
                }
            }
            Self::Orthographic {
                half_height,
                aspect,
                near,
                far,
            } => {
                if half_height <= 0.0 {
                    Some(format!("half height must be positive, got {half_height}"))
                } else {
                    Self::validate_common(aspect, near, far)
                }
            }
        };

        match reason {
            Some(reason) => Err(RenderDataError::InvalidConfig { reason }),
            None => Ok(()),
        }
    }

    /// The checks both projections share.
    fn validate_common(aspect: f32, near: f32, far: f32) -> Option<String> {
        if !(aspect > 0.0 && aspect.is_finite()) {
            Some(format!("aspect ratio must be positive, got {aspect}"))
        } else if near <= 0.0 {
            Some(format!("near plane must be positive, got {near}"))
        } else if far <= near {
            Some(format!(
                "far plane must be beyond the near plane, got near {near} far {far}"
            ))
        } else {
            None
        }
    }
}

/// A view onto the world.
///
/// The view, projection and their product are computed when something changes, not when
/// they are read. Reading happens once per object per frame at worst; changing happens
/// once per frame at most.
#[derive(Debug, Clone, PartialEq)]
pub struct Camera {
    position: Vec3,
    rotation: Quat,
    projection: Projection,
    view: Mat4,
    projection_matrix: Mat4,
    view_projection: Mat4,
}

impl Camera {
    /// Creates a camera at the origin, looking down −Z.
    ///
    /// # Errors
    ///
    /// Returns [`RenderDataError::InvalidConfig`] if the projection is degenerate.
    pub fn new(projection: Projection) -> Result<Self, RenderDataError> {
        projection.validate()?;
        let mut camera = Self {
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            projection,
            view: Mat4::IDENTITY,
            projection_matrix: Mat4::IDENTITY,
            view_projection: Mat4::IDENTITY,
        };
        camera.rebuild_projection();
        camera.rebuild_view();
        Ok(camera)
    }

    /// Creates a camera positioned at `eye` and aimed at `target`.
    ///
    /// # Errors
    ///
    /// Returns [`RenderDataError::InvalidConfig`] if the projection is degenerate or the eye
    /// and the target coincide, which leaves no direction to look in.
    pub fn looking_at(
        projection: Projection,
        eye: Vec3,
        target: Vec3,
        up: Vec3,
    ) -> Result<Self, RenderDataError> {
        let mut camera = Self::new(projection)?;
        camera.look_at(eye, target, up)?;
        Ok(camera)
    }

    /// Where the camera is.
    #[must_use]
    pub fn position(&self) -> Vec3 {
        self.position
    }

    /// How the camera is oriented.
    #[must_use]
    pub fn rotation(&self) -> Quat {
        self.rotation
    }

    /// The projection description.
    #[must_use]
    pub fn projection(&self) -> Projection {
        self.projection
    }

    /// Moves the camera.
    pub fn set_position(&mut self, position: Vec3) {
        self.position = position;
        self.rebuild_view();
    }

    /// Orients the camera. The rotation is normalized, so accumulated drift cannot skew it.
    pub fn set_rotation(&mut self, rotation: Quat) {
        self.rotation = rotation.normalize();
        self.rebuild_view();
    }

    /// Places the camera at `eye` and aims it at `target`.
    ///
    /// # Errors
    ///
    /// Returns [`RenderDataError::InvalidConfig`] if `eye` and `target` coincide or `up` is
    /// parallel to the view direction — both leave the view matrix undefined, and both are
    /// easy to produce by accident when a follow camera snaps to its subject.
    pub fn look_at(&mut self, eye: Vec3, target: Vec3, up: Vec3) -> Result<(), RenderDataError> {
        let forward = target - eye;
        if forward.length_squared() <= f32::EPSILON {
            return Err(RenderDataError::InvalidConfig {
                reason: "the camera cannot look at its own position".to_owned(),
            });
        }
        if forward.normalize().cross(up.normalize()).length_squared() <= f32::EPSILON {
            return Err(RenderDataError::InvalidConfig {
                reason: "the up vector is parallel to the view direction".to_owned(),
            });
        }

        self.position = eye;
        self.view = Mat4::look_at(eye, target, up);
        self.view_projection = self.projection_matrix * self.view;
        // The rotation is derived so `rotation()` keeps agreeing with the view matrix; the
        // view matrix itself is not rebuilt from it, because `look_at` is exact and a
        // round trip through a quaternion is not.
        self.rotation = rotation_from_view(&self.view);
        Ok(())
    }

    /// Replaces the projection.
    ///
    /// # Errors
    ///
    /// Returns [`RenderDataError::InvalidConfig`] if the projection is degenerate.
    pub fn set_projection(&mut self, projection: Projection) -> Result<(), RenderDataError> {
        projection.validate()?;
        self.projection = projection;
        self.rebuild_projection();
        Ok(())
    }

    /// Updates the aspect ratio from a viewport size, which is what a resize produces.
    ///
    /// # Errors
    ///
    /// Returns [`RenderDataError::InvalidConfig`] for a zero dimension. A minimized window
    /// reports one, and dividing by it yields a projection full of infinities.
    pub fn set_viewport(&mut self, width: u32, height: u32) -> Result<(), RenderDataError> {
        if width == 0 || height == 0 {
            return Err(RenderDataError::InvalidConfig {
                reason: format!("a viewport needs a non-zero size, got {width}x{height}"),
            });
        }
        let mut projection = self.projection;
        projection.set_aspect(width as f32 / height as f32);
        self.set_projection(projection)
    }

    /// The view matrix: world space to view space.
    #[must_use]
    pub fn view(&self) -> Mat4 {
        self.view
    }

    /// The projection matrix: view space to clip space.
    #[must_use]
    pub fn projection_matrix(&self) -> Mat4 {
        self.projection_matrix
    }

    /// The combined `projection * view`, which is what a shader needs.
    #[must_use]
    pub fn view_projection(&self) -> Mat4 {
        self.view_projection
    }

    /// The direction the camera is looking, in world space.
    #[must_use]
    pub fn forward(&self) -> Vec3 {
        self.rotation.rotate(-Vec3::Z)
    }

    /// The camera's right, in world space.
    #[must_use]
    pub fn right(&self) -> Vec3 {
        self.rotation.rotate(Vec3::X)
    }

    /// The camera's up, in world space.
    #[must_use]
    pub fn up(&self) -> Vec3 {
        self.rotation.rotate(Vec3::Y)
    }

    /// Recomputes the projection matrix and the product.
    fn rebuild_projection(&mut self) {
        self.projection_matrix = self.projection.matrix();
        self.view_projection = self.projection_matrix * self.view;
    }

    /// Recomputes the view matrix and the product from position and rotation.
    fn rebuild_view(&mut self) {
        // The view matrix is the inverse of the camera's world transform. For a rotation
        // and a translation that inverse is exact and cheap: transpose the rotation and
        // rotate the negated position, rather than running a general 4x4 inverse.
        let inverse_rotation = self.rotation.conjugate();
        let mut view = Mat4::from_quat(inverse_rotation);
        view.columns[3] = inverse_rotation.rotate(-self.position).extend(1.0);
        self.view = view;
        self.view_projection = self.projection_matrix * self.view;
    }
}

/// Recovers the camera rotation from a view matrix.
///
/// The upper-left 3×3 of a view matrix is the transpose of the camera's rotation, so the
/// rotation is read from its transpose. The branch picks the largest diagonal term, which
/// keeps the square root away from zero — the naive single-formula version loses precision
/// exactly when the camera is looking backwards.
fn rotation_from_view(view: &Mat4) -> Quat {
    let m = |row: usize, column: usize| view.get(column, row);

    let trace = m(0, 0) + m(1, 1) + m(2, 2);
    if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        Quat::new(
            (m(2, 1) - m(1, 2)) / s,
            (m(0, 2) - m(2, 0)) / s,
            (m(1, 0) - m(0, 1)) / s,
            0.25 * s,
        )
    } else if m(0, 0) > m(1, 1) && m(0, 0) > m(2, 2) {
        let s = (1.0 + m(0, 0) - m(1, 1) - m(2, 2)).sqrt() * 2.0;
        Quat::new(
            0.25 * s,
            (m(0, 1) + m(1, 0)) / s,
            (m(0, 2) + m(2, 0)) / s,
            (m(2, 1) - m(1, 2)) / s,
        )
    } else if m(1, 1) > m(2, 2) {
        let s = (1.0 + m(1, 1) - m(0, 0) - m(2, 2)).sqrt() * 2.0;
        Quat::new(
            (m(0, 1) + m(1, 0)) / s,
            0.25 * s,
            (m(1, 2) + m(2, 1)) / s,
            (m(0, 2) - m(2, 0)) / s,
        )
    } else {
        let s = (1.0 + m(2, 2) - m(0, 0) - m(1, 1)).sqrt() * 2.0;
        Quat::new(
            (m(0, 2) + m(2, 0)) / s,
            (m(1, 2) + m(2, 1)) / s,
            0.25 * s,
            (m(1, 0) - m(0, 1)) / s,
        )
    }
    .normalize()
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    const EPSILON: f32 = 1e-3;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < EPSILON
    }

    fn vec_close(a: Vec3, b: Vec3) -> bool {
        close(a.x, b.x) && close(a.y, b.y) && close(a.z, b.z)
    }

    fn camera() -> Camera {
        Camera::new(Projection::perspective(16.0 / 9.0)).expect("a valid projection")
    }

    #[test]
    fn a_new_camera_sits_at_the_origin_looking_down_minus_z() {
        let camera = camera();
        assert_eq!(camera.position(), Vec3::ZERO);
        assert!(vec_close(camera.forward(), -Vec3::Z));
        assert!(vec_close(camera.up(), Vec3::Y));
        assert!(vec_close(camera.right(), Vec3::X));
        assert_eq!(camera.view(), Mat4::IDENTITY);
    }

    #[test]
    fn the_view_matrix_moves_the_world_opposite_to_the_camera() {
        let mut camera = camera();
        camera.set_position(Vec3::new(0.0, 0.0, 5.0));
        // The camera moved back, so a point at the origin is 5 units in front of it.
        let seen = camera.view().transform_point(Vec3::ZERO);
        assert!(vec_close(seen, Vec3::new(0.0, 0.0, -5.0)), "{seen:?}");
    }

    #[test]
    fn looking_at_a_target_agrees_with_the_rotation_it_reports() {
        let mut camera = camera();
        camera
            .look_at(Vec3::new(3.0, 4.0, 5.0), Vec3::ZERO, Vec3::Y)
            .expect("a valid look direction");

        let expected = (Vec3::ZERO - Vec3::new(3.0, 4.0, 5.0)).normalize();
        assert!(
            vec_close(camera.forward(), expected),
            "{:?}",
            camera.forward()
        );
        // The target must land on the view axis, in front of the camera.
        let seen = camera.view().transform_point(Vec3::ZERO);
        assert!(close(seen.x, 0.0) && close(seen.y, 0.0), "{seen:?}");
        assert!(seen.z < 0.0, "{seen:?}");
    }

    #[test]
    fn a_rotated_camera_builds_the_same_view_as_look_at() {
        // Two ways to aim a camera must agree, or a follow camera that sets a rotation and
        // one that sets a target will disagree by a frame.
        let mut by_target = camera();
        by_target
            .look_at(Vec3::new(0.0, 2.0, 6.0), Vec3::new(0.0, 0.0, -1.0), Vec3::Y)
            .expect("a valid look direction");

        let mut by_rotation = camera();
        by_rotation.set_position(by_target.position());
        by_rotation.set_rotation(by_target.rotation());

        let probe = Vec3::new(1.5, -2.0, 3.0);
        assert!(vec_close(
            by_target.view().transform_point(probe),
            by_rotation.view().transform_point(probe)
        ));
    }

    #[test]
    fn the_view_projection_is_the_product_of_its_parts() {
        let mut camera = camera();
        camera.set_position(Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(
            camera.view_projection(),
            camera.projection_matrix() * camera.view()
        );
    }

    #[test]
    fn a_point_in_front_lands_inside_the_unit_cube() {
        let mut camera = camera();
        camera.set_position(Vec3::new(0.0, 0.0, 5.0));
        let clip = camera.view_projection().transform_point(Vec3::ZERO);
        assert!(clip.x.abs() <= 1.0 && clip.y.abs() <= 1.0, "{clip:?}");
        assert!((0.0..=1.0).contains(&clip.z), "{clip:?}");
    }

    #[test]
    fn a_point_behind_the_camera_is_outside_the_depth_range() {
        let mut camera = camera();
        camera.set_position(Vec3::new(0.0, 0.0, 5.0));
        // Ten units further away than the camera, i.e. behind it.
        let clip = camera
            .view_projection()
            .transform_point(Vec3::new(0.0, 0.0, 15.0));
        assert!(!(0.0..=1.0).contains(&clip.z), "{clip:?}");
    }

    #[test]
    fn resizing_the_viewport_changes_only_the_aspect_ratio() {
        let mut camera = camera();
        let view = camera.view();
        camera.set_viewport(1280, 720).expect("a valid viewport");
        assert!(close(camera.projection().aspect(), 1280.0 / 720.0));
        assert_eq!(camera.view(), view);
        assert_eq!(
            camera.projection().depth_range(),
            (DEFAULT_NEAR, DEFAULT_FAR)
        );
    }

    #[test]
    fn a_zero_sized_viewport_is_rejected() {
        // A minimized window reports one, and the resulting projection is all infinities.
        let mut camera = camera();
        assert!(camera.set_viewport(0, 1080).is_err());
        assert!(camera.set_viewport(1920, 0).is_err());
    }

    #[test]
    fn degenerate_projections_are_rejected() {
        assert!(Camera::new(Projection::perspective(0.0)).is_err());
        assert!(
            Camera::new(Projection::Perspective {
                fov_y: 0.0,
                aspect: 1.0,
                near: 0.1,
                far: 100.0
            })
            .is_err()
        );
        assert!(
            Camera::new(Projection::Perspective {
                fov_y: DEFAULT_FOV_Y,
                aspect: 1.0,
                near: 100.0,
                far: 1.0
            })
            .is_err()
        );
        assert!(
            Camera::new(Projection::Orthographic {
                half_height: -1.0,
                aspect: 1.0,
                near: 0.1,
                far: 10.0
            })
            .is_err()
        );
    }

    #[test]
    fn a_camera_cannot_look_at_its_own_position() {
        let mut camera = camera();
        assert!(camera.look_at(Vec3::ONE, Vec3::ONE, Vec3::Y).is_err());
    }

    #[test]
    fn an_up_vector_parallel_to_the_view_is_rejected() {
        // Looking straight down with +Y up: the cross product is zero and the view matrix
        // would be full of NaN.
        let mut camera = camera();
        assert!(
            camera
                .look_at(Vec3::new(0.0, 10.0, 0.0), Vec3::ZERO, Vec3::Y)
                .is_err()
        );
    }

    #[test]
    fn an_orthographic_camera_keeps_sizes_independent_of_distance() {
        let mut camera = Camera::new(Projection::Orthographic {
            half_height: 5.0,
            aspect: 1.0,
            near: 0.1,
            far: 100.0,
        })
        .expect("a valid projection");
        camera.set_position(Vec3::new(0.0, 0.0, 50.0));

        let near = camera
            .view_projection()
            .transform_point(Vec3::new(5.0, 0.0, 0.0));
        let far = camera
            .view_projection()
            .transform_point(Vec3::new(5.0, 0.0, -40.0));
        assert!(close(near.x, 1.0) && close(far.x, 1.0), "{near:?} {far:?}");
    }
}
