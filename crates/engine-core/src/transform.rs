//! Position, rotation and scale.
//!
//! A [`Transform`] stores translation, rotation and scale rather than a matrix. Storing the
//! matrix would be smaller to read and worse to work with: extracting a rotation from a
//! matrix needs a decomposition, accumulating rotations in a matrix drifts into a skew that
//! cannot be renormalized, and interpolation between two matrices is not a rotation at all.
//! A quaternion renormalizes with one square root.
//!
//! Matrices are generated when they are needed — once per object per frame at most, into a
//! buffer the caller already owns. See [`write_matrices`].
//!
//! This lives in `engine-core` rather than the renderer because the simulation owns
//! transforms and the renderer only reads them. The dependency arrow points one way.

use crate::math::{Mat4, Quat, Vec3};

/// Where something is, how it is turned, and how big it is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    /// Position in the parent's space.
    pub translation: Vec3,
    /// Orientation. Expected to be unit length.
    pub rotation: Quat,
    /// Per-axis scale.
    pub scale: Vec3,
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Transform {
    /// At the origin, unrotated, unscaled.
    pub const IDENTITY: Self = Self {
        translation: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    };

    /// Builds a transform from all three parts.
    #[must_use]
    pub const fn new(translation: Vec3, rotation: Quat, scale: Vec3) -> Self {
        Self {
            translation,
            rotation,
            scale,
        }
    }

    /// A pure translation.
    #[must_use]
    pub const fn from_translation(translation: Vec3) -> Self {
        Self {
            translation,
            ..Self::IDENTITY
        }
    }

    /// A pure rotation.
    #[must_use]
    pub const fn from_rotation(rotation: Quat) -> Self {
        Self {
            rotation,
            ..Self::IDENTITY
        }
    }

    /// A pure scale.
    #[must_use]
    pub const fn from_scale(scale: Vec3) -> Self {
        Self {
            scale,
            ..Self::IDENTITY
        }
    }

    /// A uniform scale.
    #[must_use]
    pub const fn from_uniform_scale(scale: f32) -> Self {
        Self::from_scale(Vec3::splat(scale))
    }

    /// Returns the transform with its translation replaced.
    #[must_use]
    pub const fn with_translation(mut self, translation: Vec3) -> Self {
        self.translation = translation;
        self
    }

    /// Returns the transform with its rotation replaced.
    #[must_use]
    pub const fn with_rotation(mut self, rotation: Quat) -> Self {
        self.rotation = rotation;
        self
    }

    /// Returns the transform with its scale replaced.
    #[must_use]
    pub const fn with_scale(mut self, scale: Vec3) -> Self {
        self.scale = scale;
        self
    }

    /// The local-to-parent matrix: scale, then rotate, then translate.
    #[must_use]
    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }

    /// Composes with a child transform, as a parent would.
    ///
    /// `parent.then(child)` is the child expressed in the parent's parent's space, and it
    /// agrees with `parent.matrix() * child.matrix()` — which is asserted by a test,
    /// because the shortcut below is only worth having if it is exactly equivalent.
    ///
    /// Non-uniform scale plus rotation is the case where a transform *cannot* represent the
    /// composition: rotating a non-uniformly scaled child introduces a shear, and there is
    /// no scale vector that encodes shear. The result is then an approximation, and callers
    /// that need exactness with non-uniform scale must compose matrices instead. Uniform
    /// scale is exact.
    #[must_use]
    pub fn then(&self, child: Self) -> Self {
        Self {
            translation: self.transform_point(child.translation),
            rotation: (self.rotation * child.rotation).normalize(),
            scale: Vec3::new(
                self.scale.x * child.scale.x,
                self.scale.y * child.scale.y,
                self.scale.z * child.scale.z,
            ),
        }
    }

    /// The inverse transform, or `None` if any axis has zero scale.
    ///
    /// A zero scale flattens space, and nothing maps back out of a plane. Returning `None`
    /// says so rather than producing infinities.
    #[must_use]
    pub fn inverse(&self) -> Option<Self> {
        if self.scale.x == 0.0 || self.scale.y == 0.0 || self.scale.z == 0.0 {
            return None;
        }
        let scale = Vec3::new(1.0 / self.scale.x, 1.0 / self.scale.y, 1.0 / self.scale.z);
        let rotation = self.rotation.conjugate();
        let translation = rotation.rotate(-self.translation);
        Some(Self {
            translation: Vec3::new(
                translation.x * scale.x,
                translation.y * scale.y,
                translation.z * scale.z,
            ),
            rotation,
            scale,
        })
    }

    /// Applies the transform to a point: scale, rotate, translate.
    #[must_use]
    pub fn transform_point(&self, point: Vec3) -> Vec3 {
        let scaled = Vec3::new(
            point.x * self.scale.x,
            point.y * self.scale.y,
            point.z * self.scale.z,
        );
        self.rotation.rotate(scaled) + self.translation
    }

    /// Applies the transform to a direction, ignoring translation.
    #[must_use]
    pub fn transform_vector(&self, vector: Vec3) -> Vec3 {
        let scaled = Vec3::new(
            vector.x * self.scale.x,
            vector.y * self.scale.y,
            vector.z * self.scale.z,
        );
        self.rotation.rotate(scaled)
    }

    /// The largest axis scale, which is what a bounding sphere has to be multiplied by.
    ///
    /// A sphere scaled non-uniformly is an ellipsoid; culling keeps it a sphere by taking
    /// the largest axis, which over-estimates and therefore never culls something visible.
    #[must_use]
    pub fn max_scale(&self) -> f32 {
        self.scale.max_element()
    }

    /// Normalizes the rotation.
    ///
    /// Accumulating rotations drifts off the unit sphere. This is one square root and is
    /// the reason rotation is stored as a quaternion rather than a matrix.
    pub fn renormalize(&mut self) {
        self.rotation = self.rotation.normalize();
    }
}

/// Writes the matrices for `transforms` into `out`.
///
/// One flat pass over two contiguous arrays with no allocation, which is what the render
/// path wants: the caller keeps `out` across frames and this only fills it. Returns the
/// number written, which is `transforms.len()` clamped to `out.len()` — a short output
/// buffer truncates rather than panicking, because the caller can then grow it and the
/// count says how much was missing.
pub fn write_matrices(transforms: &[Transform], out: &mut [Mat4]) -> usize {
    let count = transforms.len().min(out.len());
    for (transform, slot) in transforms.iter().zip(out.iter_mut()).take(count) {
        *slot = transform.matrix();
    }
    count
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    const EPSILON: f32 = 1e-4;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < EPSILON
    }

    fn vec_close(a: Vec3, b: Vec3) -> bool {
        close(a.x, b.x) && close(a.y, b.y) && close(a.z, b.z)
    }

    fn mat_close(a: Mat4, b: Mat4) -> bool {
        (0..4).all(|c| (0..4).all(|r| close(a.get(r, c), b.get(r, c))))
    }

    fn sample() -> Transform {
        Transform::new(
            Vec3::new(3.0, -4.0, 5.0),
            Quat::from_yaw_pitch_roll(0.6, -0.2, 0.35),
            Vec3::splat(2.0),
        )
    }

    #[test]
    fn the_identity_moves_nothing() {
        let point = Vec3::new(1.0, 2.0, 3.0);
        assert_eq!(Transform::IDENTITY.transform_point(point), point);
        assert_eq!(Transform::IDENTITY.matrix(), Mat4::IDENTITY);
        assert_eq!(Transform::default(), Transform::IDENTITY);
    }

    #[test]
    fn the_matrix_agrees_with_transforming_a_point_directly() {
        // Two code paths produce the same answer, so the shortcut is a shortcut and not a
        // second definition.
        let transform = sample();
        let point = Vec3::new(-1.5, 0.75, 4.0);
        assert!(vec_close(
            transform.transform_point(point),
            transform.matrix().transform_point(point)
        ));
    }

    #[test]
    fn transforming_a_vector_ignores_translation() {
        let transform = sample();
        let direction = Vec3::new(0.0, 1.0, 0.0);
        assert!(vec_close(
            transform.transform_vector(direction),
            transform.matrix().transform_vector(direction)
        ));
    }

    #[test]
    fn scale_applies_before_rotation() {
        // The other order stretches along world axes instead of local ones, which looks
        // right until something is both scaled and turned.
        let transform = Transform::from_scale(Vec3::new(2.0, 1.0, 1.0))
            .with_rotation(Quat::from_axis_angle(Vec3::Y, std::f32::consts::FRAC_PI_2));
        // Local +X is scaled to length 2, then rotated onto −Z.
        assert!(vec_close(
            transform.transform_point(Vec3::X),
            Vec3::new(0.0, 0.0, -2.0)
        ));
    }

    #[test]
    fn composition_matches_multiplying_the_matrices() {
        let parent = Transform::new(
            Vec3::new(1.0, 2.0, 3.0),
            Quat::from_axis_angle(Vec3::Y, 0.8),
            Vec3::splat(2.0),
        );
        let child = Transform::new(
            Vec3::new(0.0, 1.0, 0.0),
            Quat::from_axis_angle(Vec3::X, -0.4),
            Vec3::splat(0.5),
        );
        assert!(mat_close(
            parent.then(child).matrix(),
            parent.matrix() * child.matrix()
        ));
    }

    #[test]
    fn composing_with_the_identity_changes_nothing() {
        let transform = sample();
        assert!(mat_close(
            transform.then(Transform::IDENTITY).matrix(),
            transform.matrix()
        ));
        assert!(mat_close(
            Transform::IDENTITY.then(transform).matrix(),
            transform.matrix()
        ));
    }

    #[test]
    fn a_transform_composed_with_its_inverse_is_the_identity() {
        let transform = sample();
        let inverse = transform.inverse().expect("a non-degenerate transform");
        let point = Vec3::new(7.0, -2.0, 0.5);
        assert!(vec_close(
            inverse.transform_point(transform.transform_point(point)),
            point
        ));
    }

    #[test]
    fn a_flattened_transform_has_no_inverse() {
        assert!(
            Transform::from_scale(Vec3::new(1.0, 0.0, 1.0))
                .inverse()
                .is_none()
        );
    }

    #[test]
    fn renormalizing_removes_accumulated_drift() {
        let mut transform = Transform::from_rotation(Quat::new(0.0, 0.0, 0.0, 2.0));
        transform.renormalize();
        assert!(close(transform.rotation.length(), 1.0));
    }

    #[test]
    fn the_bounding_scale_is_the_largest_axis() {
        // Under-estimating here culls objects that are on screen.
        assert!(close(
            Transform::from_scale(Vec3::new(1.0, 5.0, 2.0)).max_scale(),
            5.0
        ));
    }

    #[test]
    fn matrices_are_written_in_order_without_allocating() {
        let transforms = [
            Transform::from_translation(Vec3::X),
            Transform::from_translation(Vec3::Y),
            Transform::from_translation(Vec3::Z),
        ];
        let mut out = vec![Mat4::ZERO; 3];
        assert_eq!(write_matrices(&transforms, &mut out), 3);
        for (transform, matrix) in transforms.iter().zip(&out) {
            assert_eq!(*matrix, transform.matrix());
        }
    }

    #[test]
    fn a_short_output_buffer_truncates_and_says_how_much_it_took() {
        let transforms = [Transform::IDENTITY; 4];
        let mut out = vec![Mat4::ZERO; 2];
        assert_eq!(write_matrices(&transforms, &mut out), 2);
    }
}
