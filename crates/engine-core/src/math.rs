//! Vectors, matrices and quaternions.
//!
//! Written here rather than taken from a crate, for the same reason the logger, the
//! allocators and the job system are: the engine's only third-party dependencies are `log`
//! and `windows`, and the conventions below have to be pinned down anyway.
//!
//! # Conventions
//!
//! Getting these wrong is the classic source of "everything renders inside-out", so they
//! are stated once and tested:
//!
//! * **Right-handed** view space. The camera looks down **−Z**; +X is right, +Y is up.
//! * Projection maps depth to **[0, 1]**, which is what D3D12 expects — not OpenGL's
//!   [−1, 1].
//! * [`Mat4`] stores **columns**, and transforms are applied to column vectors:
//!   `clip = P * V * M * v`.
//! * Angles are radians.
//!
//! The shaders declare their constant-buffer matrices `column_major`, which is the layout
//! [`Mat4`] already has, so a matrix is memcpy'd into a constant buffer with **no
//! transpose** anywhere. That agreement is asserted by a test in the renderer rather than
//! left as a convention two files happen to share.

use std::ops::{Add, AddAssign, Div, Mul, MulAssign, Neg, Sub, SubAssign};

/// Two floats.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec2 {
    /// First component.
    pub x: f32,
    /// Second component.
    pub y: f32,
}

/// Three floats: a position, direction or colour.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec3 {
    /// Right.
    pub x: f32,
    /// Up.
    pub y: f32,
    /// Backwards — the camera looks down −Z.
    pub z: f32,
}

/// Four floats: a homogeneous point, a plane, or a matrix column.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec4 {
    /// First component.
    pub x: f32,
    /// Second component.
    pub y: f32,
    /// Third component.
    pub z: f32,
    /// Fourth component.
    pub w: f32,
}

impl Vec2 {
    /// Both components zero.
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };

    /// Builds a vector.
    #[must_use]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    /// Dot product.
    #[must_use]
    pub fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y
    }

    /// Euclidean length.
    #[must_use]
    pub fn length(self) -> f32 {
        self.dot(self).sqrt()
    }
}

impl Vec3 {
    /// All components zero.
    pub const ZERO: Self = Self::splat(0.0);
    /// All components one.
    pub const ONE: Self = Self::splat(1.0);
    /// Right.
    pub const X: Self = Self::new(1.0, 0.0, 0.0);
    /// Up.
    pub const Y: Self = Self::new(0.0, 1.0, 0.0);
    /// Backwards.
    pub const Z: Self = Self::new(0.0, 0.0, 1.0);

    /// Builds a vector.
    #[must_use]
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    /// Builds a vector with every component equal.
    #[must_use]
    pub const fn splat(value: f32) -> Self {
        Self {
            x: value,
            y: value,
            z: value,
        }
    }

    /// Dot product.
    #[must_use]
    pub fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    /// Cross product, right-handed: `X × Y = Z`.
    #[must_use]
    pub fn cross(self, other: Self) -> Self {
        Self::new(
            self.y * other.z - self.z * other.y,
            self.z * other.x - self.x * other.z,
            self.x * other.y - self.y * other.x,
        )
    }

    /// Squared length, for comparisons that do not need the square root.
    #[must_use]
    pub fn length_squared(self) -> f32 {
        self.dot(self)
    }

    /// Euclidean length.
    #[must_use]
    pub fn length(self) -> f32 {
        self.length_squared().sqrt()
    }

    /// Returns the vector scaled to unit length.
    ///
    /// A zero-length vector has no direction, so it is returned unchanged rather than
    /// producing `NaN`. A silent `NaN` propagates into a view matrix and blanks the screen
    /// with nothing to point at.
    #[must_use]
    pub fn normalize(self) -> Self {
        let length = self.length();
        if length > 0.0 { self / length } else { self }
    }

    /// Component-wise minimum.
    #[must_use]
    pub fn min(self, other: Self) -> Self {
        Self::new(
            self.x.min(other.x),
            self.y.min(other.y),
            self.z.min(other.z),
        )
    }

    /// Component-wise maximum.
    #[must_use]
    pub fn max(self, other: Self) -> Self {
        Self::new(
            self.x.max(other.x),
            self.y.max(other.y),
            self.z.max(other.z),
        )
    }

    /// The largest component.
    #[must_use]
    pub fn max_element(self) -> f32 {
        self.x.max(self.y).max(self.z)
    }

    /// Linear interpolation, `t = 0` giving `self`.
    #[must_use]
    pub fn lerp(self, other: Self, t: f32) -> Self {
        self + (other - self) * t
    }

    /// Extends to a [`Vec4`] with the given `w`.
    #[must_use]
    pub fn extend(self, w: f32) -> Vec4 {
        Vec4::new(self.x, self.y, self.z, w)
    }
}

impl Vec4 {
    /// All components zero.
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0, 0.0);

    /// Builds a vector.
    #[must_use]
    pub const fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }

    /// Dot product over all four components.
    #[must_use]
    pub fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z + self.w * other.w
    }

    /// Drops `w`.
    #[must_use]
    pub fn truncate(self) -> Vec3 {
        Vec3::new(self.x, self.y, self.z)
    }

    /// Divides by `w`, turning a clip-space point into a normalized-device one.
    ///
    /// Returns `None` for `w == 0`, which is a point on the eye plane and has no
    /// projection.
    #[must_use]
    pub fn perspective_divide(self) -> Option<Vec3> {
        if self.w == 0.0 {
            return None;
        }
        Some(Vec3::new(self.x / self.w, self.y / self.w, self.z / self.w))
    }
}

/// Implements the arithmetic operators for a vector type.
macro_rules! impl_vector_ops {
    ($type:ty, $($field:ident),+) => {
        impl Add for $type {
            type Output = Self;
            fn add(self, other: Self) -> Self {
                Self { $($field: self.$field + other.$field),+ }
            }
        }

        impl Sub for $type {
            type Output = Self;
            fn sub(self, other: Self) -> Self {
                Self { $($field: self.$field - other.$field),+ }
            }
        }

        impl Neg for $type {
            type Output = Self;
            fn neg(self) -> Self {
                Self { $($field: -self.$field),+ }
            }
        }

        impl Mul<f32> for $type {
            type Output = Self;
            fn mul(self, scalar: f32) -> Self {
                Self { $($field: self.$field * scalar),+ }
            }
        }

        impl Mul<$type> for f32 {
            type Output = $type;
            fn mul(self, vector: $type) -> $type {
                vector * self
            }
        }

        impl Div<f32> for $type {
            type Output = Self;
            fn div(self, scalar: f32) -> Self {
                Self { $($field: self.$field / scalar),+ }
            }
        }

        impl AddAssign for $type {
            fn add_assign(&mut self, other: Self) {
                $(self.$field += other.$field;)+
            }
        }

        impl SubAssign for $type {
            fn sub_assign(&mut self, other: Self) {
                $(self.$field -= other.$field;)+
            }
        }

        impl MulAssign<f32> for $type {
            fn mul_assign(&mut self, scalar: f32) {
                $(self.$field *= scalar;)+
            }
        }
    };
}

impl_vector_ops!(Vec2, x, y);
impl_vector_ops!(Vec3, x, y, z);
impl_vector_ops!(Vec4, x, y, z, w);

/// A rotation.
///
/// Stored `(x, y, z, w)` with `w` the scalar part, and expected to be unit length. Rotation
/// is stored as a quaternion rather than a matrix because composing and interpolating are
/// cheaper and renormalizing is one square root — see `docs/architecture.md`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quat {
    /// Vector part, X.
    pub x: f32,
    /// Vector part, Y.
    pub y: f32,
    /// Vector part, Z.
    pub z: f32,
    /// Scalar part.
    pub w: f32,
}

impl Default for Quat {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Quat {
    /// No rotation.
    pub const IDENTITY: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
        w: 1.0,
    };

    /// Builds a quaternion from raw components.
    #[must_use]
    pub const fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }

    /// A rotation of `radians` about `axis`, which is normalized first.
    #[must_use]
    pub fn from_axis_angle(axis: Vec3, radians: f32) -> Self {
        let axis = axis.normalize();
        let (sin, cos) = (radians * 0.5).sin_cos();
        Self::new(axis.x * sin, axis.y * sin, axis.z * sin, cos)
    }

    /// Yaw about +Y, then pitch about +X, then roll about −Z.
    ///
    /// This order is what a third-person camera wants: yaw orbits the character, pitch
    /// raises and lowers the view, and roll (rarely used) tilts the result.
    #[must_use]
    pub fn from_yaw_pitch_roll(yaw: f32, pitch: f32, roll: f32) -> Self {
        Self::from_axis_angle(Vec3::Y, yaw)
            * Self::from_axis_angle(Vec3::X, pitch)
            * Self::from_axis_angle(Vec3::Z, roll)
    }

    /// Squared length.
    #[must_use]
    pub fn length_squared(self) -> f32 {
        self.x * self.x + self.y * self.y + self.z * self.z + self.w * self.w
    }

    /// Length.
    #[must_use]
    pub fn length(self) -> f32 {
        self.length_squared().sqrt()
    }

    /// Returns the quaternion scaled to unit length, or the identity if it has none.
    #[must_use]
    pub fn normalize(self) -> Self {
        let length = self.length();
        if length > 0.0 {
            Self::new(
                self.x / length,
                self.y / length,
                self.z / length,
                self.w / length,
            )
        } else {
            Self::IDENTITY
        }
    }

    /// The inverse rotation, assuming unit length.
    #[must_use]
    pub fn conjugate(self) -> Self {
        Self::new(-self.x, -self.y, -self.z, self.w)
    }

    /// Rotates a vector.
    #[must_use]
    pub fn rotate(self, vector: Vec3) -> Vec3 {
        // v + 2 * cross(q.xyz, cross(q.xyz, v) + q.w * v) — the standard sandwich product
        // written without building a matrix.
        let axis = Vec3::new(self.x, self.y, self.z);
        let temp = axis.cross(vector) + vector * self.w;
        vector + axis.cross(temp) * 2.0
    }
}

impl Mul for Quat {
    type Output = Self;

    /// Composes rotations: `a * b` applies `b` first.
    fn mul(self, other: Self) -> Self {
        Self::new(
            self.w * other.x + self.x * other.w + self.y * other.z - self.z * other.y,
            self.w * other.y - self.x * other.z + self.y * other.w + self.z * other.x,
            self.w * other.z + self.x * other.y - self.y * other.x + self.z * other.w,
            self.w * other.w - self.x * other.x - self.y * other.y - self.z * other.z,
        )
    }
}

/// A 4×4 matrix, stored as four columns.
///
/// `columns[c][r]` is the element in row `r`, column `c` — the layout D3D12 and HLSL call
/// column-major, and the one a `Mat4 * Vec4` product reads contiguously.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mat4 {
    /// The four columns, left to right.
    pub columns: [Vec4; 4],
}

impl Default for Mat4 {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Mat4 {
    /// The identity matrix.
    pub const IDENTITY: Self = Self {
        columns: [
            Vec4::new(1.0, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 1.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 1.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        ],
    };

    /// Every element zero.
    pub const ZERO: Self = Self {
        columns: [Vec4::ZERO; 4],
    };

    /// Builds a matrix from its columns.
    #[must_use]
    pub const fn from_columns(x: Vec4, y: Vec4, z: Vec4, w: Vec4) -> Self {
        Self {
            columns: [x, y, z, w],
        }
    }

    /// A translation.
    #[must_use]
    pub fn from_translation(translation: Vec3) -> Self {
        let mut matrix = Self::IDENTITY;
        matrix.columns[3] = translation.extend(1.0);
        matrix
    }

    /// A non-uniform scale.
    #[must_use]
    pub fn from_scale(scale: Vec3) -> Self {
        let mut matrix = Self::IDENTITY;
        matrix.columns[0].x = scale.x;
        matrix.columns[1].y = scale.y;
        matrix.columns[2].z = scale.z;
        matrix
    }

    /// A rotation.
    #[must_use]
    pub fn from_quat(rotation: Quat) -> Self {
        let Quat { x, y, z, w } = rotation;
        let (x2, y2, z2) = (x + x, y + y, z + z);
        let (xx, xy, xz) = (x * x2, x * y2, x * z2);
        let (yy, yz, zz) = (y * y2, y * z2, z * z2);
        let (wx, wy, wz) = (w * x2, w * y2, w * z2);

        Self::from_columns(
            Vec4::new(1.0 - (yy + zz), xy + wz, xz - wy, 0.0),
            Vec4::new(xy - wz, 1.0 - (xx + zz), yz + wx, 0.0),
            Vec4::new(xz + wy, yz - wx, 1.0 - (xx + yy), 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        )
    }

    /// Scale, then rotate, then translate — the order a transform hierarchy expects.
    #[must_use]
    pub fn from_scale_rotation_translation(scale: Vec3, rotation: Quat, translation: Vec3) -> Self {
        let mut matrix = Self::from_quat(rotation);
        matrix.columns[0] *= scale.x;
        matrix.columns[1] *= scale.y;
        matrix.columns[2] *= scale.z;
        matrix.columns[3] = translation.extend(1.0);
        matrix
    }

    /// A right-handed view matrix looking from `eye` towards `target`.
    ///
    /// `up` only has to be non-parallel to the view direction; it is re-orthogonalized.
    #[must_use]
    pub fn look_at(eye: Vec3, target: Vec3, up: Vec3) -> Self {
        let forward = (target - eye).normalize();
        let right = forward.cross(up).normalize();
        let up = right.cross(forward);

        Self::from_columns(
            Vec4::new(right.x, up.x, -forward.x, 0.0),
            Vec4::new(right.y, up.y, -forward.y, 0.0),
            Vec4::new(right.z, up.z, -forward.z, 0.0),
            Vec4::new(-right.dot(eye), -up.dot(eye), forward.dot(eye), 1.0),
        )
    }

    /// A right-handed perspective projection mapping depth to [0, 1].
    ///
    /// `fov_y` is the vertical field of view in radians.
    #[must_use]
    pub fn perspective(fov_y: f32, aspect: f32, near: f32, far: f32) -> Self {
        let height = 1.0 / (fov_y * 0.5).tan();
        let width = height / aspect;
        let depth = far / (near - far);

        Self::from_columns(
            Vec4::new(width, 0.0, 0.0, 0.0),
            Vec4::new(0.0, height, 0.0, 0.0),
            Vec4::new(0.0, 0.0, depth, -1.0),
            Vec4::new(0.0, 0.0, depth * near, 0.0),
        )
    }

    /// A right-handed orthographic projection mapping depth to [0, 1].
    #[must_use]
    pub fn orthographic(left: f32, right: f32, bottom: f32, top: f32, near: f32, far: f32) -> Self {
        let width = 1.0 / (right - left);
        let height = 1.0 / (top - bottom);
        let depth = 1.0 / (near - far);

        Self::from_columns(
            Vec4::new(2.0 * width, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 2.0 * height, 0.0, 0.0),
            Vec4::new(0.0, 0.0, depth, 0.0),
            Vec4::new(
                -(right + left) * width,
                -(top + bottom) * height,
                near * depth,
                1.0,
            ),
        )
    }

    /// The element in row `row`, column `column`.
    #[must_use]
    pub fn get(&self, row: usize, column: usize) -> f32 {
        let column = self.columns[column];
        match row {
            0 => column.x,
            1 => column.y,
            2 => column.z,
            _ => column.w,
        }
    }

    /// The transpose.
    ///
    /// Used when uploading to HLSL, which reads constant-buffer matrices row-major.
    #[must_use]
    pub fn transpose(&self) -> Self {
        Self::from_columns(
            Vec4::new(
                self.columns[0].x,
                self.columns[1].x,
                self.columns[2].x,
                self.columns[3].x,
            ),
            Vec4::new(
                self.columns[0].y,
                self.columns[1].y,
                self.columns[2].y,
                self.columns[3].y,
            ),
            Vec4::new(
                self.columns[0].z,
                self.columns[1].z,
                self.columns[2].z,
                self.columns[3].z,
            ),
            Vec4::new(
                self.columns[0].w,
                self.columns[1].w,
                self.columns[2].w,
                self.columns[3].w,
            ),
        )
    }

    /// The 16 elements in column-major order, ready to memcpy into a constant buffer.
    #[must_use]
    pub fn to_cols_array(&self) -> [f32; 16] {
        let mut out = [0.0; 16];
        for (column, values) in self.columns.iter().zip(out.chunks_exact_mut(4)) {
            values[0] = column.x;
            values[1] = column.y;
            values[2] = column.z;
            values[3] = column.w;
        }
        out
    }

    /// Transforms a point, applying translation and dividing out `w` if the matrix is
    /// projective.
    #[must_use]
    pub fn transform_point(&self, point: Vec3) -> Vec3 {
        let transformed = *self * point.extend(1.0);
        transformed
            .perspective_divide()
            .unwrap_or_else(|| transformed.truncate())
    }

    /// Transforms a direction, ignoring translation.
    #[must_use]
    pub fn transform_vector(&self, vector: Vec3) -> Vec3 {
        (*self * vector.extend(0.0)).truncate()
    }

    /// The determinant.
    #[must_use]
    pub fn determinant(&self) -> f32 {
        self.cofactors().1
    }

    /// The inverse, or `None` for a singular matrix.
    ///
    /// Returning an `Option` rather than a matrix of `NaN`s matters: a singular matrix
    /// comes from a zero scale, which is a legitimate thing for a caller to have and an
    /// illegitimate thing to silently propagate into a view transform.
    #[must_use]
    pub fn inverse(&self) -> Option<Self> {
        let (adjugate, determinant) = self.cofactors();
        if determinant == 0.0 {
            return None;
        }
        let inverse = 1.0 / determinant;
        Some(Self::from_columns(
            adjugate.columns[0] * inverse,
            adjugate.columns[1] * inverse,
            adjugate.columns[2] * inverse,
            adjugate.columns[3] * inverse,
        ))
    }

    /// The adjugate and the determinant, which share every 2×2 sub-determinant.
    ///
    /// Computing them together is not premature optimization: the determinant is the
    /// weighted sum of the first column's cofactors, so a separate implementation would
    /// duplicate the same eighteen products and could drift from it.
    fn cofactors(&self) -> (Self, f32) {
        let m = self.to_cols_array();

        // Pairs of 2x2 determinants from the bottom two rows, then the top two.
        let s0 = m[0] * m[5] - m[4] * m[1];
        let s1 = m[0] * m[9] - m[8] * m[1];
        let s2 = m[0] * m[13] - m[12] * m[1];
        let s3 = m[4] * m[9] - m[8] * m[5];
        let s4 = m[4] * m[13] - m[12] * m[5];
        let s5 = m[8] * m[13] - m[12] * m[9];

        let c5 = m[10] * m[15] - m[14] * m[11];
        let c4 = m[6] * m[15] - m[14] * m[7];
        let c3 = m[6] * m[11] - m[10] * m[7];
        let c2 = m[2] * m[15] - m[14] * m[3];
        let c1 = m[2] * m[11] - m[10] * m[3];
        let c0 = m[2] * m[7] - m[6] * m[3];

        let determinant = s0 * c5 - s1 * c4 + s2 * c3 + s3 * c2 - s4 * c1 + s5 * c0;

        let adjugate = Self::from_columns(
            Vec4::new(
                m[5] * c5 - m[9] * c4 + m[13] * c3,
                -m[1] * c5 + m[9] * c2 - m[13] * c1,
                m[1] * c4 - m[5] * c2 + m[13] * c0,
                -m[1] * c3 + m[5] * c1 - m[9] * c0,
            ),
            Vec4::new(
                -m[4] * c5 + m[8] * c4 - m[12] * c3,
                m[0] * c5 - m[8] * c2 + m[12] * c1,
                -m[0] * c4 + m[4] * c2 - m[12] * c0,
                m[0] * c3 - m[4] * c1 + m[8] * c0,
            ),
            Vec4::new(
                m[7] * s5 - m[11] * s4 + m[15] * s3,
                -m[3] * s5 + m[11] * s2 - m[15] * s1,
                m[3] * s4 - m[7] * s2 + m[15] * s0,
                -m[3] * s3 + m[7] * s1 - m[11] * s0,
            ),
            Vec4::new(
                -m[6] * s5 + m[10] * s4 - m[14] * s3,
                m[2] * s5 - m[10] * s2 + m[14] * s1,
                -m[2] * s4 + m[6] * s2 - m[14] * s0,
                m[2] * s3 - m[6] * s1 + m[10] * s0,
            ),
        );

        (adjugate, determinant)
    }
}

impl Mul<Vec4> for Mat4 {
    type Output = Vec4;

    fn mul(self, vector: Vec4) -> Vec4 {
        self.columns[0] * vector.x
            + self.columns[1] * vector.y
            + self.columns[2] * vector.z
            + self.columns[3] * vector.w
    }
}

impl Mul for Mat4 {
    type Output = Self;

    /// Composes transforms: `a * b` applies `b` first.
    fn mul(self, other: Self) -> Self {
        Self::from_columns(
            self * other.columns[0],
            self * other.columns[1],
            self * other.columns[2],
            self * other.columns[3],
        )
    }
}

/// An axis-aligned bounding box.
///
/// Built by accumulating points, which is what a mesh loader does. An empty box is
/// represented by `min > max` so that the first point accumulated replaces both — the
/// alternative, a separate `is_empty` flag, has to be checked in the inner loop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    /// Lowest corner.
    pub min: Vec3,
    /// Highest corner.
    pub max: Vec3,
}

impl Default for Aabb {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl Aabb {
    /// A box containing nothing.
    pub const EMPTY: Self = Self {
        min: Vec3::splat(f32::INFINITY),
        max: Vec3::splat(f32::NEG_INFINITY),
    };

    /// Builds a box from two corners.
    #[must_use]
    pub const fn new(min: Vec3, max: Vec3) -> Self {
        Self { min, max }
    }

    /// Whether the box contains nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x || self.min.y > self.max.y || self.min.z > self.max.z
    }

    /// Grows the box to contain `point`.
    pub fn insert(&mut self, point: Vec3) {
        self.min = self.min.min(point);
        self.max = self.max.max(point);
    }

    /// The box containing every point given.
    #[must_use]
    pub fn from_points(points: impl IntoIterator<Item = Vec3>) -> Self {
        let mut bounds = Self::EMPTY;
        for point in points {
            bounds.insert(point);
        }
        bounds
    }

    /// The point in the middle, or the origin for an empty box.
    #[must_use]
    pub fn center(&self) -> Vec3 {
        if self.is_empty() {
            Vec3::ZERO
        } else {
            (self.min + self.max) * 0.5
        }
    }

    /// Half the size along each axis, or zero for an empty box.
    #[must_use]
    pub fn half_extents(&self) -> Vec3 {
        if self.is_empty() {
            Vec3::ZERO
        } else {
            (self.max - self.min) * 0.5
        }
    }

    /// Whether the box contains `point`, inclusive of the surface.
    #[must_use]
    pub fn contains(&self, point: Vec3) -> bool {
        point.x >= self.min.x
            && point.y >= self.min.y
            && point.z >= self.min.z
            && point.x <= self.max.x
            && point.y <= self.max.y
            && point.z <= self.max.z
    }
}

/// A bounding sphere: the shape culling tests against.
///
/// A sphere rather than a box because a frustum test against a sphere is four multiplies
/// and a compare per plane and needs no transform of the bounds — a rotated box does.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Sphere {
    /// Centre, in the same space as the geometry it bounds.
    pub center: Vec3,
    /// Radius. Zero for a sphere bounding nothing.
    pub radius: f32,
}

impl Sphere {
    /// A sphere bounding nothing, at the origin.
    pub const EMPTY: Self = Self {
        center: Vec3::ZERO,
        radius: 0.0,
    };

    /// Builds a sphere.
    #[must_use]
    pub const fn new(center: Vec3, radius: f32) -> Self {
        Self { center, radius }
    }

    /// The sphere around a box's centre that contains the whole box.
    ///
    /// Not the smallest sphere containing the points — computing that (Welzl's algorithm)
    /// costs far more and buys a marginally tighter cull. This one is one pass and never
    /// under-estimates, which is the property that matters: an under-estimating bound culls
    /// something that is on screen.
    #[must_use]
    pub fn around(points: impl IntoIterator<Item = Vec3> + Clone) -> Self {
        let bounds = Aabb::from_points(points.clone());
        if bounds.is_empty() {
            return Self::EMPTY;
        }
        let center = bounds.center();
        let radius = points
            .into_iter()
            .map(|point| (point - center).length_squared())
            .fold(0.0_f32, f32::max)
            .sqrt();
        Self { center, radius }
    }

    /// Whether the sphere contains `point`.
    #[must_use]
    pub fn contains(&self, point: Vec3) -> bool {
        (point - self.center).length_squared() <= self.radius * self.radius
    }
}

/// A plane, as a unit normal and a signed distance from the origin.
///
/// Points on the positive side satisfy `dot(normal, point) + distance > 0`. For a frustum
/// plane the normal points **inward**, so "inside" and "positive" are the same thing and
/// the culling test is one comparison per plane.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Plane {
    /// Unit normal, pointing inward for a frustum plane.
    pub normal: Vec3,
    /// Signed distance from the origin along the normal.
    pub distance: f32,
}

impl Plane {
    /// Builds a plane from a normal and a distance, normalizing both together.
    ///
    /// The distance has to be scaled by the same factor as the normal, or the plane moves.
    /// That is the classic error in frustum extraction, and it produces a frustum that
    /// culls correctly near the centre of the screen and wrongly at the edges.
    #[must_use]
    pub fn new(normal: Vec3, distance: f32) -> Self {
        let length = normal.length();
        if length > 0.0 {
            Self {
                normal: normal / length,
                distance: distance / length,
            }
        } else {
            Self { normal, distance }
        }
    }

    /// How far `point` is from the plane, positive on the normal's side.
    #[must_use]
    pub fn signed_distance(&self, point: Vec3) -> f32 {
        self.normal.dot(point) + self.distance
    }
}

/// The six planes of a view frustum, all pointing inward.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Frustum {
    /// Left, right, bottom, top, near, far.
    pub planes: [Plane; 6],
}

impl Frustum {
    /// Extracts the planes from a view-projection matrix (Gribb–Hartmann).
    ///
    /// The rows of `view_projection` are the clip-space plane equations, so each frustum
    /// plane is a sum or difference of two rows. The near plane is row 2 alone rather than
    /// `w + z`, because D3D12 clips depth to **[0, 1]** — the `w + z` form is the OpenGL
    /// [−1, 1] convention, and using it produces a near plane in the wrong place that only
    /// shows up as objects vanishing just in front of the camera.
    #[must_use]
    pub fn from_view_projection(view_projection: &Mat4) -> Self {
        let row = |index: usize| {
            Vec4::new(
                view_projection.get(index, 0),
                view_projection.get(index, 1),
                view_projection.get(index, 2),
                view_projection.get(index, 3),
            )
        };
        let (x, y, z, w) = (row(0), row(1), row(2), row(3));

        let plane = |value: Vec4| Plane::new(value.truncate(), value.w);

        Self {
            planes: [
                plane(w + x),
                plane(w - x),
                plane(w + y),
                plane(w - y),
                plane(z),
                plane(w - z),
            ],
        }
    }

    /// Whether any part of `sphere` is inside the frustum.
    ///
    /// Conservative: a sphere straddling a plane counts as inside. Culling something
    /// visible is a rendering error; drawing something invisible is only work.
    #[must_use]
    pub fn intersects_sphere(&self, sphere: &Sphere) -> bool {
        self.planes
            .iter()
            .all(|plane| plane.signed_distance(sphere.center) >= -sphere.radius)
    }

    /// Whether `point` is inside the frustum.
    #[must_use]
    pub fn contains_point(&self, point: Vec3) -> bool {
        self.planes
            .iter()
            .all(|plane| plane.signed_distance(point) >= 0.0)
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    /// Tolerance for comparisons after a matrix inverse or a chain of trig.
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

    #[test]
    fn cross_products_are_right_handed() {
        // If this is left-handed, every normal points inward and back-face culling
        // removes exactly the triangles that should be visible.
        assert_eq!(Vec3::X.cross(Vec3::Y), Vec3::Z);
        assert_eq!(Vec3::Y.cross(Vec3::Z), Vec3::X);
        assert_eq!(Vec3::Z.cross(Vec3::X), Vec3::Y);
    }

    #[test]
    fn normalizing_a_zero_vector_does_not_produce_nan() {
        assert_eq!(Vec3::ZERO.normalize(), Vec3::ZERO);
        assert!(close(Vec3::new(3.0, 4.0, 0.0).normalize().length(), 1.0));
    }

    #[test]
    fn identity_leaves_everything_alone() {
        let point = Vec3::new(1.0, -2.0, 3.5);
        assert_eq!(Mat4::IDENTITY.transform_point(point), point);
        assert_eq!(Mat4::IDENTITY * Mat4::IDENTITY, Mat4::IDENTITY);
    }

    #[test]
    fn multiplication_is_associative() {
        let a = Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0));
        let b = Mat4::from_quat(Quat::from_axis_angle(Vec3::Y, 0.7));
        let c = Mat4::from_scale(Vec3::new(2.0, 0.5, 3.0));
        assert!(mat_close((a * b) * c, a * (b * c)));
    }

    #[test]
    fn multiplication_applies_the_right_operand_first() {
        // scale then translate: the translation must not be scaled.
        let combined =
            Mat4::from_translation(Vec3::new(10.0, 0.0, 0.0)) * Mat4::from_scale(Vec3::splat(2.0));
        assert!(vec_close(
            combined.transform_point(Vec3::new(1.0, 0.0, 0.0)),
            Vec3::new(12.0, 0.0, 0.0)
        ));
    }

    #[test]
    fn a_matrix_times_its_inverse_is_the_identity() {
        let matrix = Mat4::from_scale_rotation_translation(
            Vec3::new(2.0, 3.0, 0.5),
            Quat::from_yaw_pitch_roll(0.4, -0.9, 0.2),
            Vec3::new(-5.0, 1.0, 12.0),
        );
        let inverse = matrix.inverse().expect("an invertible transform");
        assert!(mat_close(matrix * inverse, Mat4::IDENTITY));
        assert!(mat_close(inverse * matrix, Mat4::IDENTITY));
    }

    #[test]
    fn a_singular_matrix_has_no_inverse() {
        // A zero scale is a legitimate thing to have and an illegitimate thing to invert.
        assert!(
            Mat4::from_scale(Vec3::new(1.0, 0.0, 1.0))
                .inverse()
                .is_none()
        );
    }

    #[test]
    fn the_determinant_agrees_with_the_scale_it_describes() {
        let matrix = Mat4::from_scale(Vec3::new(2.0, 3.0, 4.0));
        assert!(close(matrix.determinant(), 24.0));
        // A rotation preserves volume.
        assert!(close(
            Mat4::from_quat(Quat::from_axis_angle(Vec3::new(1.0, 1.0, 0.0), 1.1)).determinant(),
            1.0
        ));
    }

    #[test]
    fn the_view_matrix_puts_the_camera_at_the_origin_looking_down_minus_z() {
        let eye = Vec3::new(0.0, 0.0, 10.0);
        let view = Mat4::look_at(eye, Vec3::ZERO, Vec3::Y);

        assert!(vec_close(view.transform_point(eye), Vec3::ZERO));
        // A point in front of the camera lands on negative Z in view space.
        let front = view.transform_point(Vec3::ZERO);
        assert!(
            front.z < 0.0,
            "expected negative view-space Z, got {front:?}"
        );
    }

    #[test]
    fn projection_maps_the_near_and_far_planes_to_zero_and_one() {
        // The single most valuable test here: D3D12 clips to [0, 1], and an OpenGL-style
        // [-1, 1] projection renders a scene that is half missing rather than obviously
        // broken.
        let projection = Mat4::perspective(60_f32.to_radians(), 16.0 / 9.0, 0.1, 100.0);

        let near = projection.transform_point(Vec3::new(0.0, 0.0, -0.1));
        let far = projection.transform_point(Vec3::new(0.0, 0.0, -100.0));
        assert!(close(near.z, 0.0), "near plane maps to {}", near.z);
        assert!(close(far.z, 1.0), "far plane maps to {}", far.z);
    }

    #[test]
    fn projection_maps_the_frustum_corners_to_the_edges_of_the_screen() {
        let fov = 90_f32.to_radians();
        let aspect = 2.0;
        let projection = Mat4::perspective(fov, aspect, 1.0, 100.0);

        // At 90° vertical FOV, the half-height at distance 1 is 1, and the half-width is
        // the aspect ratio.
        let top_right = projection.transform_point(Vec3::new(aspect, 1.0, -1.0));
        assert!(close(top_right.x, 1.0), "{top_right:?}");
        assert!(close(top_right.y, 1.0), "{top_right:?}");

        let bottom_left = projection.transform_point(Vec3::new(-aspect, -1.0, -1.0));
        assert!(close(bottom_left.x, -1.0), "{bottom_left:?}");
        assert!(close(bottom_left.y, -1.0), "{bottom_left:?}");
    }

    #[test]
    fn orthographic_maps_its_box_to_the_unit_cube() {
        let projection = Mat4::orthographic(-4.0, 4.0, -2.0, 2.0, 0.5, 50.0);
        let near = projection.transform_point(Vec3::new(4.0, 2.0, -0.5));
        let far = projection.transform_point(Vec3::new(-4.0, -2.0, -50.0));
        assert!(vec_close(near, Vec3::new(1.0, 1.0, 0.0)), "{near:?}");
        assert!(vec_close(far, Vec3::new(-1.0, -1.0, 1.0)), "{far:?}");
    }

    #[test]
    fn a_quaternion_and_its_matrix_rotate_alike() {
        let rotation = Quat::from_yaw_pitch_roll(0.9, 0.3, -0.6);
        let matrix = Mat4::from_quat(rotation);
        let point = Vec3::new(1.0, 2.0, -3.0);
        assert!(vec_close(
            rotation.rotate(point),
            matrix.transform_vector(point)
        ));
    }

    #[test]
    fn a_quarter_turn_about_y_sends_x_to_minus_z() {
        // Right-handed: a positive rotation about +Y turns +X towards −Z.
        let rotation = Quat::from_axis_angle(Vec3::Y, std::f32::consts::FRAC_PI_2);
        assert!(vec_close(rotation.rotate(Vec3::X), -Vec3::Z));
    }

    #[test]
    fn composing_a_rotation_with_its_conjugate_is_the_identity() {
        let rotation = Quat::from_yaw_pitch_roll(1.2, -0.4, 0.8);
        let point = Vec3::new(0.3, -1.7, 4.0);
        assert!(vec_close(
            (rotation * rotation.conjugate()).rotate(point),
            point
        ));
    }

    #[test]
    fn quaternion_multiplication_applies_the_right_operand_first() {
        let yaw = Quat::from_axis_angle(Vec3::Y, std::f32::consts::FRAC_PI_2);
        let pitch = Quat::from_axis_angle(Vec3::X, std::f32::consts::FRAC_PI_2);
        let point = Vec3::Z;
        assert!(vec_close(
            (yaw * pitch).rotate(point),
            yaw.rotate(pitch.rotate(point))
        ));
    }

    #[test]
    fn transposing_twice_returns_the_original() {
        let matrix = Mat4::from_scale_rotation_translation(
            Vec3::new(1.5, 2.0, 0.25),
            Quat::from_yaw_pitch_roll(0.1, 0.2, 0.3),
            Vec3::new(7.0, -8.0, 9.0),
        );
        assert_eq!(matrix.transpose().transpose(), matrix);
        assert_eq!(matrix.transpose().get(1, 3), matrix.get(3, 1));
    }

    #[test]
    fn the_column_array_is_what_a_constant_buffer_expects() {
        let matrix = Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0));
        let array = matrix.to_cols_array();
        // Translation lives in the fourth column, so elements 12..15 — the layout HLSL
        // reads when the constant buffer is declared column_major.
        assert_eq!(&array[12..15], &[1.0, 2.0, 3.0]);
    }

    #[test]
    fn an_empty_box_absorbs_the_first_point_it_is_given() {
        let mut bounds = Aabb::EMPTY;
        assert!(bounds.is_empty());
        bounds.insert(Vec3::new(1.0, 2.0, 3.0));
        assert!(!bounds.is_empty());
        assert_eq!(bounds.min, bounds.max);
        assert_eq!(bounds.center(), Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(bounds.half_extents(), Vec3::ZERO);
    }

    #[test]
    fn a_box_contains_the_points_it_was_built_from() {
        let points = [
            Vec3::new(-1.0, 0.0, 2.0),
            Vec3::new(3.0, -4.0, 0.0),
            Vec3::new(0.0, 5.0, -6.0),
        ];
        let bounds = Aabb::from_points(points);
        assert!(points.iter().all(|point| bounds.contains(*point)));
        assert!(!bounds.contains(Vec3::new(4.0, 0.0, 0.0)));
    }

    #[test]
    fn an_empty_box_has_no_centre_to_report() {
        assert_eq!(Aabb::EMPTY.center(), Vec3::ZERO);
        assert_eq!(Aabb::EMPTY.half_extents(), Vec3::ZERO);
    }

    #[test]
    fn a_bounding_sphere_never_under_estimates() {
        // Under-estimating culls geometry that is on screen, so the test is containment,
        // not tightness.
        let points = [
            Vec3::new(-1.0, -1.0, -1.0),
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(0.0, 3.0, 0.0),
        ];
        let sphere = Sphere::around(points);
        assert!(points.iter().all(|point| sphere.contains(*point)));
    }

    #[test]
    fn a_sphere_around_nothing_is_empty() {
        assert_eq!(Sphere::around(std::iter::empty()), Sphere::EMPTY);
    }

    /// A camera at the origin looking down −Z, 90° vertical, square aspect.
    fn test_frustum() -> Frustum {
        let projection = Mat4::perspective(std::f32::consts::FRAC_PI_2, 1.0, 1.0, 100.0);
        let view = Mat4::look_at(Vec3::ZERO, -Vec3::Z, Vec3::Y);
        Frustum::from_view_projection(&(projection * view))
    }

    #[test]
    fn frustum_planes_are_normalized() {
        // If the normal is normalized without scaling the distance, the plane moves — and
        // the frustum then culls correctly near the centre of the screen and wrongly at the
        // edges, which is much harder to spot than culling nothing at all.
        for plane in test_frustum().planes {
            assert!(close(plane.normal.length(), 1.0), "{plane:?}");
        }
    }

    #[test]
    fn a_point_in_front_of_the_camera_is_inside_the_frustum() {
        let frustum = test_frustum();
        assert!(frustum.contains_point(Vec3::new(0.0, 0.0, -10.0)));
        assert!(!frustum.contains_point(Vec3::new(0.0, 0.0, 10.0)), "behind");
        assert!(
            !frustum.contains_point(Vec3::new(0.0, 0.0, -0.5)),
            "too near"
        );
        assert!(
            !frustum.contains_point(Vec3::new(0.0, 0.0, -200.0)),
            "too far"
        );
        assert!(
            !frustum.contains_point(Vec3::new(100.0, 0.0, -10.0)),
            "far off to the side"
        );
    }

    #[test]
    fn a_sphere_straddling_a_plane_counts_as_visible() {
        // Conservative on purpose: culling something visible is a rendering error, drawing
        // something invisible is only work.
        let frustum = test_frustum();
        let straddling = Sphere::new(Vec3::new(0.0, 0.0, -0.5), 2.0);
        assert!(!frustum.contains_point(straddling.center));
        assert!(frustum.intersects_sphere(&straddling));
    }

    #[test]
    fn a_sphere_well_outside_is_culled() {
        let frustum = test_frustum();
        assert!(!frustum.intersects_sphere(&Sphere::new(Vec3::new(0.0, 0.0, 50.0), 1.0)));
        assert!(!frustum.intersects_sphere(&Sphere::new(Vec3::new(500.0, 0.0, -10.0), 1.0)));
        assert!(!frustum.intersects_sphere(&Sphere::new(Vec3::new(0.0, 0.0, -500.0), 1.0)));
    }

    #[test]
    fn the_near_plane_uses_the_d3d_depth_convention() {
        // The near plane is row 2 alone, not w + z. With the OpenGL form it lands at the
        // wrong distance, and the symptom is objects vanishing just in front of the camera.
        let frustum = test_frustum();
        let near = frustum.planes[4];
        // Looking down -Z with near = 1, the plane's inward normal is -Z and it sits one
        // unit in front of the origin.
        assert!(close(near.normal.z, -1.0), "{near:?}");
        assert!(close(near.distance, -1.0), "{near:?}");
    }

    #[test]
    fn the_vertical_planes_reject_above_and_below() {
        // The horizontal planes were covered; these were not, and a sign error in the
        // `w + y` / `w - y` pair is invisible in a scene whose objects all sit near the
        // ground.
        let frustum = test_frustum();
        assert!(frustum.contains_point(Vec3::new(0.0, 5.0, -10.0)), "inside");
        assert!(
            !frustum.contains_point(Vec3::new(0.0, 40.0, -10.0)),
            "above"
        );
        assert!(
            !frustum.contains_point(Vec3::new(0.0, -40.0, -10.0)),
            "below"
        );
    }

    #[test]
    fn the_far_plane_rejects_just_beyond_it_and_keeps_just_inside() {
        // The near plane has its own test because it uses the D3D convention. The far plane
        // is `w - z` in both conventions, so what is checked here is the distance, not the
        // form.
        let frustum = test_frustum();
        assert!(frustum.contains_point(Vec3::new(0.0, 0.0, -99.0)));
        assert!(!frustum.contains_point(Vec3::new(0.0, 0.0, -101.0)));
        // Straddling: conservative, so it survives.
        assert!(frustum.intersects_sphere(&Sphere::new(Vec3::new(0.0, 0.0, -101.0), 2.0)));
    }

    #[test]
    fn a_translated_camera_culls_relative_to_where_it_stands() {
        // Every other frustum test puts the camera at the origin looking down -Z, which is
        // exactly the configuration in which the view matrix is the identity — so a bug in
        // how the view participates in the extraction would not show. This moves it.
        let projection = Mat4::perspective(std::f32::consts::FRAC_PI_2, 1.0, 1.0, 100.0);
        let eye = Vec3::new(100.0, 20.0, 50.0);
        let view = Mat4::look_at(eye, eye - Vec3::Z, Vec3::Y);
        let frustum = Frustum::from_view_projection(&(projection * view));

        assert!(
            frustum.contains_point(eye - Vec3::new(0.0, 0.0, 10.0)),
            "ten units in front of the moved camera"
        );
        assert!(
            !frustum.contains_point(Vec3::ZERO),
            "the world origin is now behind and far off to the side"
        );
    }

    #[test]
    fn a_rotated_camera_culls_along_its_own_axes() {
        // A quarter turn about +Y sends the view direction from -Z to -X. A frustum that
        // ignored the rotation would still accept the point in front of the *world* axis.
        let projection = Mat4::perspective(std::f32::consts::FRAC_PI_2, 1.0, 1.0, 100.0);
        let view = Mat4::look_at(Vec3::ZERO, -Vec3::X, Vec3::Y);
        let frustum = Frustum::from_view_projection(&(projection * view));

        assert!(frustum.contains_point(Vec3::new(-10.0, 0.0, 0.0)), "ahead");
        assert!(
            !frustum.contains_point(Vec3::new(0.0, 0.0, -10.0)),
            "where the camera used to look"
        );
        assert!(!frustum.contains_point(Vec3::new(10.0, 0.0, 0.0)), "behind");
    }

    #[test]
    fn an_aspect_ratio_widens_the_horizontal_planes_only() {
        // At 16:9 the frustum is wider than tall. A projection that applied the aspect to
        // the wrong axis would pass every test that uses a square aspect.
        let projection = Mat4::perspective(std::f32::consts::FRAC_PI_2, 16.0 / 9.0, 1.0, 100.0);
        let view = Mat4::look_at(Vec3::ZERO, -Vec3::Z, Vec3::Y);
        let frustum = Frustum::from_view_projection(&(projection * view));

        // 90 degrees vertical means the half-height equals the depth; the half-width is
        // that times the aspect ratio.
        assert!(
            frustum.contains_point(Vec3::new(17.0, 0.0, -10.0)),
            "inside"
        );
        assert!(
            !frustum.contains_point(Vec3::new(19.0, 0.0, -10.0)),
            "wider"
        );
        assert!(frustum.contains_point(Vec3::new(0.0, 9.5, -10.0)), "inside");
        assert!(
            !frustum.contains_point(Vec3::new(0.0, 10.5, -10.0)),
            "taller"
        );
    }

    #[test]
    fn a_sphere_at_the_edge_of_view_is_kept() {
        // At 90 degrees vertical FOV and square aspect, the frustum widens by one unit per
        // unit of depth, so this point is exactly on the right-hand plane.
        let frustum = test_frustum();
        assert!(frustum.intersects_sphere(&Sphere::new(Vec3::new(10.0, 0.0, -10.0), 0.1)));
        assert!(!frustum.intersects_sphere(&Sphere::new(Vec3::new(12.0, 0.0, -10.0), 0.1)));
    }

    #[test]
    fn scale_rotation_translation_applies_in_that_order() {
        let scale = Vec3::new(2.0, 2.0, 2.0);
        let rotation = Quat::from_axis_angle(Vec3::Y, std::f32::consts::FRAC_PI_2);
        let translation = Vec3::new(0.0, 5.0, 0.0);

        let combined = Mat4::from_scale_rotation_translation(scale, rotation, translation);
        let expected = Mat4::from_translation(translation)
            * Mat4::from_quat(rotation)
            * Mat4::from_scale(scale);

        assert!(mat_close(combined, expected));
    }
}
