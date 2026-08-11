//! Bones, the hierarchy they form, and the matrices skinning needs.
//!
//! ```text
//! Skeleton (rest pose, bind pose)   +   Pose (local transforms, this frame)
//!                     │                          │
//!                     └────────────┬─────────────┘
//!                                  ▼
//!                    global[i] = global[parent] · local[i]
//!                    skin[i]   = global[i] · inverse_bind[i]
//!                                  │
//!                                  ▼
//!                       the palette the vertex shader reads
//! ```
//!
//! # Why the inverse bind matrix is stored and not derived
//!
//! A skinned vertex is authored in *bind* space. To move it with a bone, it first has to be
//! expressed relative to that bone — which is what multiplying by the bone's inverse bind
//! matrix does — and only then can the bone's current global transform put it back in the
//! world. Deriving the inverse bind from the rest pose assumes the rest pose *is* the bind
//! pose. That is true for everything this crate authors and is emphatically not true for
//! glTF, which stores them separately precisely because an exporter may differ. So it is a
//! field, and the procedural builders fill it from their own rest pose.
//!
//! # Different characters, different skeletons
//!
//! Nothing here says two skeletons must match. A [`Pose`] is only meaningful next to the
//! [`Skeleton`] it was sampled for, and every function that takes both checks that their
//! bone counts agree. Bones are addressed by index for speed and looked up by name once, at
//! load — [`Skeleton::bone_index`] — which is how a clip authored against one skeleton is
//! bound to another without either of them knowing about the other.

use engine_core::math::Mat4;
use engine_core::transform::Transform;

use crate::error::AssetError;

/// The most bones one skeleton may have.
///
/// The palette is uploaded per skinned draw into the frame's constant ring, and 128 bones is
/// 8 KB of `Mat4`. That is a comfortable constant-buffer size and roughly four times what a
/// stylized humanoid needs, so the limit is a guard against a broken export rather than a
/// constraint on the art.
pub const MAX_BONES: usize = 128;

/// One bone: a name, a parent, where it rests, and how to get into its space.
#[derive(Debug, Clone, PartialEq)]
pub struct Bone {
    /// What the artist called it. The key a [`crate::AnimationClip`] binds through.
    pub name: String,
    /// Index of the parent bone, or `None` for a root.
    ///
    /// Always **less** than this bone's own index. See [`Skeleton::new`].
    pub parent: Option<u16>,
    /// The bone's transform relative to its parent when nothing is animating it.
    pub rest: Transform,
    /// World-to-bone at bind time: the inverse of the bone's global bind transform.
    pub inverse_bind: Mat4,
}

/// A bone hierarchy.
#[derive(Debug, Clone, PartialEq)]
pub struct Skeleton {
    bones: Vec<Bone>,
}

impl Skeleton {
    /// Builds a skeleton from bones already in parent-before-child order.
    ///
    /// # Errors
    ///
    /// Returns [`AssetError::Invalid`] if there are no bones, if there are more than
    /// [`MAX_BONES`], or if any bone's parent index is not strictly less than its own.
    ///
    /// That last rule is what makes [`Skeleton::global_transforms`] a single forward pass
    /// with no recursion, no visited set and no cycle check at runtime. A cycle in a bone
    /// hierarchy is an infinite loop in the most performance-sensitive place there is, so it
    /// is made unrepresentable at construction instead of guarded against every frame.
    pub fn new(bones: Vec<Bone>) -> Result<Self, AssetError> {
        if bones.is_empty() {
            return Err(AssetError::invalid(
                "skeleton",
                "bones",
                "a skeleton needs at least one bone",
            ));
        }
        if bones.len() > MAX_BONES {
            return Err(AssetError::over_budget(
                "skeleton",
                "bone",
                bones.len() as u64,
                MAX_BONES as u64,
            ));
        }
        for (index, bone) in bones.iter().enumerate() {
            if let Some(parent) = bone.parent
                && parent as usize >= index
            {
                return Err(AssetError::invalid(
                    "skeleton",
                    format!("bone {index} '{}'", bone.name),
                    format!("parent {parent} is not before it; bones must be topologically sorted"),
                ));
            }
        }
        Ok(Self { bones })
    }

    /// The bones, in hierarchy order.
    #[must_use]
    pub fn bones(&self) -> &[Bone] {
        &self.bones
    }

    /// How many bones there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.bones.len()
    }

    /// Whether the skeleton has no bones. Never true for a constructed one.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bones.is_empty()
    }

    /// The index of the bone called `name`, if there is one.
    ///
    /// Linear, and meant to be called at load. A character with 30 bones binding 8 clips is
    /// a few hundred comparisons once; a map would be a hash per lookup and an allocation
    /// per skeleton to save that.
    #[must_use]
    pub fn bone_index(&self, name: &str) -> Option<u16> {
        self.bones
            .iter()
            .position(|bone| bone.name == name)
            .map(|index| index as u16)
    }

    /// The rest pose: every bone at the transform it was authored with.
    #[must_use]
    pub fn rest_pose(&self) -> Pose {
        Pose {
            locals: self.bones.iter().map(|bone| bone.rest).collect(),
        }
    }

    /// Composes each bone's local transform with its parent's, into `out`.
    ///
    /// One forward pass, because [`Skeleton::new`] guaranteed parents come first.
    ///
    /// # Errors
    ///
    /// Returns [`AssetError::Invalid`] if `pose` was sampled for a different skeleton.
    pub fn global_transforms(&self, pose: &Pose, out: &mut Vec<Mat4>) -> Result<(), AssetError> {
        if pose.len() != self.bones.len() {
            return Err(AssetError::invalid(
                "skeleton",
                "pose",
                format!(
                    "the pose has {} bones and the skeleton has {}",
                    pose.len(),
                    self.bones.len()
                ),
            ));
        }
        out.clear();
        out.reserve(self.bones.len());
        for (index, bone) in self.bones.iter().enumerate() {
            let local = pose.locals[index].matrix();
            let global = match bone.parent {
                // Already computed: the parent's index is strictly smaller.
                Some(parent) => out[parent as usize] * local,
                None => local,
            };
            out.push(global);
        }
        Ok(())
    }

    /// Fills `out` with the matrix each bone applies to the vertices bound to it.
    ///
    /// This is the palette the vertex shader indexes with `SkinVertex::joints`. `out` is a
    /// caller-owned buffer that is cleared and refilled, so a character animating every
    /// frame allocates once and never again.
    ///
    /// # Errors
    ///
    /// Returns [`AssetError::Invalid`] if `pose` was sampled for a different skeleton.
    pub fn skinning_matrices(&self, pose: &Pose, out: &mut Vec<Mat4>) -> Result<(), AssetError> {
        self.global_transforms(pose, out)?;
        for (matrix, bone) in out.iter_mut().zip(&self.bones) {
            *matrix = *matrix * bone.inverse_bind;
        }
        Ok(())
    }

    /// The inverse bind matrices implied by treating the rest pose as the bind pose.
    ///
    /// What every procedural builder in this crate wants: it authors vertices in the rest
    /// pose, so bind and rest are the same thing and the inverse binds are derived rather
    /// than typed out. An importer must **not** use this — glTF ships its own, and assuming
    /// they agree is how an imported character arrives inside out.
    ///
    /// A bone whose global bind transform is singular gets the identity, which leaves the
    /// vertices bound to it in bind space rather than sending them to infinity.
    #[must_use]
    pub fn inverse_binds_from_rest(bones: &[Bone]) -> Vec<Mat4> {
        let mut globals: Vec<Mat4> = Vec::with_capacity(bones.len());
        for bone in bones {
            let local = bone.rest.matrix();
            let global = match bone.parent {
                Some(parent) => globals[parent as usize] * local,
                None => local,
            };
            globals.push(global);
        }
        globals
            .iter()
            .map(|global| global.inverse().unwrap_or(Mat4::IDENTITY))
            .collect()
    }
}

/// Every bone's transform relative to its parent, right now.
///
/// Local, not global: an animation clip is authored per bone in its parent's space, blending
/// two clips is only meaningful in that space, and composing to global is the one cheap pass
/// at the end. A pose of globals cannot be blended without being decomposed first.
#[derive(Debug, Clone, PartialEq)]
pub struct Pose {
    locals: Vec<Transform>,
}

impl Pose {
    /// A pose from explicit local transforms.
    #[must_use]
    pub fn from_locals(locals: Vec<Transform>) -> Self {
        Self { locals }
    }

    /// The local transforms, by bone index.
    #[must_use]
    pub fn locals(&self) -> &[Transform] {
        &self.locals
    }

    /// The local transforms, mutably — what a sampler writes into.
    #[must_use]
    pub fn locals_mut(&mut self) -> &mut [Transform] {
        &mut self.locals
    }

    /// How many bones this pose covers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.locals.len()
    }

    /// Whether the pose covers no bones.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.locals.is_empty()
    }

    /// Overwrites this pose with `other`, reusing the allocation.
    ///
    /// `Clone::clone_from` would do this if it were specialized, but the derived
    /// implementation is `*self = source.clone()`, which allocates. The animation player
    /// copies poses three times per character per frame, so it does it through this.
    ///
    /// A source of a different length replaces the buffer rather than being refused: this is
    /// a copy, not a blend, and the result is still exactly `other`.
    pub fn copy_from(&mut self, other: &Self) {
        if self.locals.len() == other.locals.len() {
            self.locals.copy_from_slice(&other.locals);
        } else {
            self.locals.clear();
            self.locals.extend_from_slice(&other.locals);
        }
    }

    /// Interpolates every bone of `self` towards `other`, in place.
    ///
    /// The whole of this phase's blending: a cross-fade between the pose a character was in
    /// and the pose the new animation puts it in. Rotations take the short arc; translation
    /// and scale are linear.
    ///
    /// Poses of different lengths are left alone rather than partially blended — the pair
    /// belongs to different skeletons and a partial blend would be a character with half a
    /// body in one animation.
    pub fn blend_towards(&mut self, other: &Self, t: f32) {
        if self.locals.len() != other.locals.len() {
            return;
        }
        let t = t.clamp(0.0, 1.0);
        for (current, target) in self.locals.iter_mut().zip(&other.locals) {
            current.translation = current.translation.lerp(target.translation, t);
            current.rotation = current.rotation.nlerp(target.rotation, t);
            current.scale = current.scale.lerp(target.scale, t);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::math::{Quat, Vec3};

    fn chain() -> Skeleton {
        // root at the origin, child one metre up, grandchild one metre above that.
        let mut bones = vec![
            Bone {
                name: "root".to_owned(),
                parent: None,
                rest: Transform::IDENTITY,
                inverse_bind: Mat4::IDENTITY,
            },
            Bone {
                name: "spine".to_owned(),
                parent: Some(0),
                rest: Transform::from_translation(Vec3::new(0.0, 1.0, 0.0)),
                inverse_bind: Mat4::IDENTITY,
            },
            Bone {
                name: "head".to_owned(),
                parent: Some(1),
                rest: Transform::from_translation(Vec3::new(0.0, 1.0, 0.0)),
                inverse_bind: Mat4::IDENTITY,
            },
        ];
        let inverses = Skeleton::inverse_binds_from_rest(&bones);
        for (bone, inverse) in bones.iter_mut().zip(inverses) {
            bone.inverse_bind = inverse;
        }
        match Skeleton::new(bones) {
            Ok(skeleton) => skeleton,
            Err(error) => panic!("the test skeleton must build: {error}"),
        }
    }

    #[test]
    fn a_parent_after_its_child_is_rejected_at_construction() {
        // A cycle here is an infinite loop in the hottest pass in the animation system, so
        // it is made unrepresentable rather than checked per frame.
        let bones = vec![Bone {
            name: "child".to_owned(),
            parent: Some(0),
            rest: Transform::IDENTITY,
            inverse_bind: Mat4::IDENTITY,
        }];
        assert!(Skeleton::new(bones).is_err(), "bone 0 cannot parent itself");
    }

    #[test]
    fn an_empty_skeleton_is_rejected() {
        assert!(Skeleton::new(Vec::new()).is_err());
    }

    #[test]
    fn globals_accumulate_down_the_chain() {
        let skeleton = chain();
        let mut globals = Vec::new();
        assert!(
            skeleton
                .global_transforms(&skeleton.rest_pose(), &mut globals)
                .is_ok()
        );
        assert_eq!(globals.len(), 3);
        assert_eq!(
            globals[2].transform_point(Vec3::ZERO),
            Vec3::new(0.0, 2.0, 0.0)
        );
    }

    #[test]
    fn the_rest_pose_skins_to_identity() {
        // The definition of a bind pose: a character standing in it must be undeformed, so
        // every skinning matrix is the identity. This catches an inverse-bind that was
        // derived with the multiplication the wrong way round, which is otherwise only
        // visible as a character exploded across the map.
        let skeleton = chain();
        let mut palette = Vec::new();
        assert!(
            skeleton
                .skinning_matrices(&skeleton.rest_pose(), &mut palette)
                .is_ok()
        );
        for (index, matrix) in palette.iter().enumerate() {
            let moved = matrix.transform_point(Vec3::new(0.3, 1.5, -0.2));
            assert!(
                (moved - Vec3::new(0.3, 1.5, -0.2)).length() < 1e-4,
                "bone {index} moved a bind-pose vertex to {moved:?}"
            );
        }
    }

    #[test]
    fn rotating_a_parent_carries_its_children() {
        let skeleton = chain();
        let mut pose = skeleton.rest_pose();
        pose.locals_mut()[1].rotation = Quat::from_axis_angle(Vec3::Z, std::f32::consts::FRAC_PI_2);

        let mut globals = Vec::new();
        assert!(skeleton.global_transforms(&pose, &mut globals).is_ok());
        // The spine sits at y = 1 and turns a quarter-turn about +Z, which sends the head's
        // local +Y offset to -X.
        let head = globals[2].transform_point(Vec3::ZERO);
        assert!(
            (head - Vec3::new(-1.0, 1.0, 0.0)).length() < 1e-4,
            "{head:?}"
        );
    }

    #[test]
    fn a_pose_from_another_skeleton_is_refused_rather_than_read_out_of_range() {
        let skeleton = chain();
        let short = Pose::from_locals(vec![Transform::IDENTITY]);
        let mut out = Vec::new();
        assert!(skeleton.global_transforms(&short, &mut out).is_err());
        assert!(skeleton.skinning_matrices(&short, &mut out).is_err());
    }

    #[test]
    fn bones_are_found_by_name_once_and_addressed_by_index_after() {
        let skeleton = chain();
        assert_eq!(skeleton.bone_index("head"), Some(2));
        assert_eq!(skeleton.bone_index("tail"), None);
    }

    #[test]
    fn blending_moves_every_bone_and_leaves_a_mismatched_pose_alone() {
        let skeleton = chain();
        let mut pose = skeleton.rest_pose();
        let mut target = skeleton.rest_pose();
        target.locals_mut()[0].translation = Vec3::new(4.0, 0.0, 0.0);

        pose.blend_towards(&target, 0.25);
        assert_eq!(pose.locals()[0].translation.x, 1.0);

        let before = pose.clone();
        pose.blend_towards(&Pose::from_locals(vec![Transform::IDENTITY]), 1.0);
        assert_eq!(pose, before, "a pose of another length must not half-blend");
    }

    #[test]
    fn more_bones_than_the_palette_holds_is_rejected() {
        let bones: Vec<Bone> = (0..MAX_BONES + 1)
            .map(|index| Bone {
                name: format!("bone{index}"),
                parent: index.checked_sub(1).map(|parent| parent as u16),
                rest: Transform::IDENTITY,
                inverse_bind: Mat4::IDENTITY,
            })
            .collect();
        assert!(Skeleton::new(bones).is_err());
    }
}
