//! Mirroring an animation clip left to right.
//!
//! ```text
//!  strafe_left ──► sample every key time ──► each bone takes its counterpart's motion,
//!                                            reflected across the character's centre plane
//!                                                              │
//!                                                              ▼
//!                                                        strafe_right
//! ```
//!
//! # Why motion and not transforms
//!
//! Copying the counterpart's local rotation onto a bone only works on a rig whose left and
//! right bones are authored as exact mirror images of each other, down to their roll — and
//! a rig from an auto-rigger, or one retargeted from another character, rarely is. So each
//! bone takes the counterpart's *motion away from its rest pose*, in model space, reflects
//! that, and applies it to its own rest pose:
//!
//! ```text
//! mirrored(b) = M · global(c, t) · rest(c)⁻¹ · M · rest(b)
//! ```
//!
//! where `c` is `b`'s counterpart and `M` reflects across the plane through the root bone
//! that divides the character's left from its right. Two reflections compose to a proper
//! rigid transform, so the result is a rotation and a translation like any other pose.

use engine_core::math::{Mat4, Quat, Vec3};

use crate::animation::{AnimationClip, BoneTrack, Key};
use crate::error::AssetError;
use crate::gltf_import::decompose;
use crate::skeleton::Skeleton;

/// The name of the bone on the other side of the character, if `name` says which side it is.
///
/// `LeftArm` ↔ `RightArm`, `mixamorig:LeftHand` ↔ `mixamorig:RightHand`, `hand_l` ↔
/// `hand_r`, `hand.L` ↔ `hand.R`. A bone with no side — the spine, the head — has none and
/// mirrors onto itself.
#[must_use]
pub fn counterpart_name(name: &str) -> Option<String> {
    for (a, b) in [("Left", "Right"), ("left", "right")] {
        if name.contains(a) {
            return Some(name.replacen(a, b, 1));
        }
        if name.contains(b) {
            return Some(name.replacen(b, a, 1));
        }
    }
    for (a, b) in [("_L", "_R"), ("_l", "_r"), (".L", ".R"), (".l", ".r")] {
        if let Some(stem) = name.strip_suffix(a) {
            return Some(format!("{stem}{b}"));
        }
        if let Some(stem) = name.strip_suffix(b) {
            return Some(format!("{stem}{a}"));
        }
    }
    None
}

/// `clip` played on the other side of the body, called `name`.
///
/// Sampled at every moment any of the clip's tracks has a key, so nothing the source keys
/// is lost; between keys the source interpolated anyway. Every bone gets a track, because a
/// bone the source left at rest may be the counterpart of one it moved. Looping, release and
/// authored speed carry over unchanged.
///
/// # Errors
///
/// Returns [`AssetError::Invalid`] if `clip` was not made for `skeleton`.
pub fn mirrored(
    clip: &AnimationClip,
    skeleton: &Skeleton,
    name: impl Into<String>,
) -> Result<AnimationClip, AssetError> {
    let bones = skeleton.bones();
    let counterpart: Vec<usize> = bones
        .iter()
        .enumerate()
        .map(|(index, bone)| {
            counterpart_name(&bone.name)
                .and_then(|other| skeleton.bone_index(&other))
                .map_or(index, usize::from)
        })
        .collect();

    let mut rest = Vec::with_capacity(bones.len());
    skeleton.global_transforms(&skeleton.rest_pose(), &mut rest)?;
    let rest_inverse: Vec<Mat4> = rest
        .iter()
        .map(|matrix| matrix.inverse().unwrap_or(Mat4::IDENTITY))
        .collect();

    // The plane through the root that divides left from right: x = root's x, since the
    // engine's characters face -Z.
    let centre = rest
        .first()
        .map_or(0.0, |root| root.transform_point(Vec3::ZERO).x);
    let reflect = Mat4::from_scale_rotation_translation(
        Vec3::new(-1.0, 1.0, 1.0),
        Quat::IDENTITY,
        Vec3::new(2.0 * centre, 0.0, 0.0),
    );

    let mut times: Vec<f32> = clip
        .tracks
        .iter()
        .flat_map(|track| {
            let translation = track.translation.iter().map(|key| key.time);
            let rotation = track.rotation.iter().map(|key| key.time);
            let scale = track.scale.iter().map(|key| key.time);
            translation.chain(rotation).chain(scale)
        })
        .collect();
    times.push(0.0);
    times.push(clip.duration);
    times.sort_by(f32::total_cmp);
    times.dedup_by(|a, b| (*a - *b).abs() < 1e-4);

    // Sampled as a one-shot, so the last key time reads the last key rather than wrapping
    // round to the first.
    let source = AnimationClip {
        looping: false,
        ..clip.clone()
    };

    let mut tracks: Vec<BoneTrack> = (0..bones.len())
        .map(|bone| BoneTrack::new(bone as u16))
        .collect();
    let mut globals = Vec::with_capacity(bones.len());
    let mut mirrored_globals = vec![Mat4::IDENTITY; bones.len()];
    for time in times {
        let mut pose = skeleton.rest_pose();
        source.sample(time, &mut pose)?;
        skeleton.global_transforms(&pose, &mut globals)?;
        for (bone, (bone_def, track)) in bones.iter().zip(&mut tracks).enumerate() {
            let other = counterpart[bone];
            let global = reflect * globals[other] * rest_inverse[other] * reflect * rest[bone];
            mirrored_globals[bone] = global;
            let local = match bone_def.parent {
                Some(parent) => mirrored_globals[parent as usize]
                    .inverse()
                    .map_or(global, |inverse| inverse * global),
                None => global,
            };
            let local = decompose(local);
            track.translation.push(Key::new(time, local.translation));
            track
                .rotation
                .push(Key::new(time, local.rotation.normalize()));
            track.scale.push(Key::new(time, local.scale));
        }
    }

    let mut result = AnimationClip::new(name, clip.looping, tracks);
    result.duration = clip.duration;
    result.release = clip.release;
    result.speed = clip.speed;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skeleton::Bone;
    use engine_core::transform::Transform;

    fn bone(name: &str, parent: Option<u16>, x: f32) -> Bone {
        Bone {
            name: name.to_owned(),
            parent,
            rest: Transform::from_translation(Vec3::new(x, 0.0, 0.0)),
            inverse_bind: Mat4::IDENTITY,
        }
    }

    /// A root with an arm and a hand on each side.
    fn arms() -> Skeleton {
        let bones = vec![
            bone("root", None, 0.0),
            bone("LeftArm", Some(0), 1.0),
            bone("LeftHand", Some(1), 1.0),
            bone("RightArm", Some(0), -1.0),
            bone("RightHand", Some(3), -1.0),
        ];
        match Skeleton::new(bones) {
            Ok(skeleton) => skeleton,
            Err(error) => panic!("the test skeleton must build: {error}"),
        }
    }

    fn position(skeleton: &Skeleton, clip: &AnimationClip, bone: &str) -> Vec3 {
        let mut pose = skeleton.rest_pose();
        assert!(clip.sample(clip.duration, &mut pose).is_ok());
        let mut globals = Vec::new();
        assert!(skeleton.global_transforms(&pose, &mut globals).is_ok());
        let Some(index) = skeleton.bone_index(bone) else {
            panic!("no bone {bone}");
        };
        globals[index as usize].transform_point(Vec3::ZERO)
    }

    #[test]
    fn a_side_is_named_every_way_rigs_name_one() {
        assert_eq!(
            counterpart_name("mixamorig:LeftHand").as_deref(),
            Some("mixamorig:RightHand")
        );
        assert_eq!(counterpart_name("RightArm").as_deref(), Some("LeftArm"));
        assert_eq!(counterpart_name("hand_l").as_deref(), Some("hand_r"));
        assert_eq!(counterpart_name("hand.R").as_deref(), Some("hand.L"));
        assert_eq!(counterpart_name("mixamorig:Spine"), None);
    }

    #[test]
    fn raising_the_left_arm_mirrors_to_raising_the_right() {
        let skeleton = arms();
        // The left arm swings up a quarter turn about Z: its hand goes from (2, 0) to (1, 1).
        let up = Quat::from_axis_angle(Vec3::Z, std::f32::consts::FRAC_PI_2);
        let clip = AnimationClip::new(
            "raise_left",
            false,
            vec![
                BoneTrack::new(1)
                    .with_rotation(vec![Key::new(0.0, Quat::IDENTITY), Key::new(1.0, up)]),
            ],
        );
        assert!(
            (position(&skeleton, &clip, "LeftHand") - Vec3::new(1.0, 1.0, 0.0)).length() < 1e-4
        );

        let Ok(mirror) = mirrored(&clip, &skeleton, "raise_right") else {
            panic!("the clip must mirror");
        };
        assert_eq!(mirror.name, "raise_right");
        assert!((mirror.duration - 1.0).abs() < 1e-6);
        let right = position(&skeleton, &mirror, "RightHand");
        assert!(
            (right - Vec3::new(-1.0, 1.0, 0.0)).length() < 1e-4,
            "{right:?}"
        );
        let left = position(&skeleton, &mirror, "LeftHand");
        assert!(
            (left - Vec3::new(2.0, 0.0, 0.0)).length() < 1e-4,
            "{left:?}"
        );
    }

    #[test]
    fn mirroring_twice_gives_back_the_original_motion() {
        let skeleton = arms();
        let tilt = Quat::from_axis_angle(Vec3::new(0.3, 0.8, 0.5).normalize(), 0.9);
        let clip = AnimationClip::new(
            "odd",
            false,
            vec![BoneTrack::new(1).with_rotation(vec![Key::new(1.0, tilt)])],
        );
        let Ok(once) = mirrored(&clip, &skeleton, "once") else {
            panic!("the clip must mirror");
        };
        let Ok(twice) = mirrored(&once, &skeleton, "twice") else {
            panic!("the mirror must mirror");
        };
        for bone in ["LeftHand", "RightHand"] {
            let a = position(&skeleton, &clip, bone);
            let b = position(&skeleton, &twice, bone);
            assert!((a - b).length() < 1e-4, "{bone}: {a:?} vs {b:?}");
        }
    }
}
