//! The rig, the skinning and the animation vocabulary every procedural humanoid shares.
//!
//! ```text
//! humanoid ──► skeleton()          21 bones, T-pose, feet at y = 0, facing -Z
//!          ├─► SkinBuilder         prisms -> skinned geometry, one builder per material
//!          └─► clips(&Style)       idle, walk, run, jump, fall, land, cast, cast_heavy
//!
//! warrior.rs ─┐
//!             ├─ geometry + textures + materials + a Style. Nothing else.
//! druid.rs   ─┘
//! ```
//!
//! # Why this exists and why the FireMage is not built on it
//!
//! Two characters that share a skeleton must share it *exactly*: a walk cycle is written as
//! rotations of named bones at fixed indices, and two hand-copied bone tables that agree
//! today are two bone tables that disagree the first time one is tuned. So the Warrior and
//! the Druid are one rig, one skinning rule and one set of cycles with different amplitudes.
//!
//! [`super::firemage`] keeps its own copy. It was written first, its clips are authored
//! around a staff held across the body, and its tests assert on the geometry it produces —
//! rebuilding it on this module would be a change to the one character that already works,
//! for no gain the player can see. When an artist's export replaces any of the three, the
//! duplication goes with it.
//!
//! # What a `Style` is allowed to change
//!
//! Amplitudes and rest poses, never structure. Every character walks with the same knee
//! rule, the same two-bobs-per-cycle and the same counter-swinging arms, because those are
//! facts about walking rather than facts about a class. What a Warrior gets is a longer
//! stride and a heavier landing, not a different cycle.

use engine_core::math::{Mat4, Quat, Vec2, Vec3};
use engine_core::transform::Transform;

use crate::animation::{AnimationClip, BoneTrack, Key};
use crate::mesh::{Geometry, SkinVertex, SubMesh};
use crate::procedural::Prism;
use crate::skeleton::{Bone, Skeleton};

/// How tall every model built on this rig is authored, in metres.
///
/// The visual layer scales by `gameplay_height / MODEL_HEIGHT`, so a model that is not the
/// height it declares makes its character the wrong size in the world. Each model measures
/// its own geometry against this in a test.
pub const MODEL_HEIGHT: f32 = 1.80;

// Bone indices. A parent is always a smaller index — `Skeleton::new` enforces it, and the
// order below is what keeps the enforcement from ever firing.
/// The bone everything hangs from.
pub const ROOT: u16 = 0;
/// The pelvis. Every locomotion cycle's bob is a translation of this.
pub const HIPS: u16 = 1;
/// Lower back.
pub const SPINE: u16 = 2;
/// Upper back, and the parent of both shoulders.
pub const CHEST: u16 = 3;
/// Neck.
pub const NECK: u16 = 4;
/// Head.
pub const HEAD: u16 = 5;
/// Left shoulder.
pub const SHOULDER_L: u16 = 6;
/// Left upper arm.
pub const ARM_L: u16 = 7;
/// Left forearm.
pub const FOREARM_L: u16 = 8;
/// Left hand.
pub const HAND_L: u16 = 9;
/// Right shoulder.
pub const SHOULDER_R: u16 = 10;
/// Right upper arm.
pub const ARM_R: u16 = 11;
/// Right forearm.
pub const FOREARM_R: u16 = 12;
/// Right hand — the hand a prop is held in.
pub const HAND_R: u16 = 13;
/// Left thigh.
pub const THIGH_L: u16 = 14;
/// Left shin.
pub const SHIN_L: u16 = 15;
/// Left foot.
pub const FOOT_L: u16 = 16;
/// Right thigh.
pub const THIGH_R: u16 = 17;
/// Right shin.
pub const SHIN_R: u16 = 18;
/// Right foot.
pub const FOOT_R: u16 = 19;
/// Whatever the character is carrying, parented to the right hand.
///
/// Named for the *role* rather than for a sword or a staff, because the rig has no opinion
/// about which: geometry bound rigidly to this bone follows the hand, and that is all a
/// weapon is.
pub const PROP: u16 = 20;

/// How many bones the rig has.
pub const BONE_COUNT: usize = 21;

/// The hips' rest translation, which every hips translation track is written relative to.
pub const HIPS_REST_Y: f32 = 0.92;

/// The bone hierarchy, in bind pose.
///
/// A T-pose, because every animation is authored as a rotation *away* from it and a T-pose
/// is the one configuration where a limb's local axes line up with the model's. Translations
/// are local — relative to the parent — which is why the numbers look small.
///
/// # Errors
///
/// Returns [`crate::AssetError`] if the table above is edited into an inconsistent state:
/// a parent that comes after its child, or a bone with no name.
pub fn skeleton() -> Result<Skeleton, crate::AssetError> {
    let bone = |name: &str, parent: Option<u16>, x: f32, y: f32, z: f32| Bone {
        name: name.to_owned(),
        parent,
        rest: Transform::from_translation(Vec3::new(x, y, z)),
        inverse_bind: Mat4::IDENTITY,
    };

    let mut bones = vec![
        bone("root", None, 0.0, 0.0, 0.0),
        bone("hips", Some(ROOT), 0.0, HIPS_REST_Y, 0.0),
        bone("spine", Some(HIPS), 0.0, 0.16, 0.0),
        bone("chest", Some(SPINE), 0.0, 0.18, 0.0),
        bone("neck", Some(CHEST), 0.0, 0.16, 0.0),
        bone("head", Some(NECK), 0.0, 0.10, 0.0),
        bone("shoulder_l", Some(CHEST), 0.09, 0.13, 0.0),
        bone("arm_l", Some(SHOULDER_L), 0.08, 0.0, 0.0),
        bone("forearm_l", Some(ARM_L), 0.25, 0.0, 0.0),
        bone("hand_l", Some(FOREARM_L), 0.23, 0.0, 0.0),
        bone("shoulder_r", Some(CHEST), -0.09, 0.13, 0.0),
        bone("arm_r", Some(SHOULDER_R), -0.08, 0.0, 0.0),
        bone("forearm_r", Some(ARM_R), -0.25, 0.0, 0.0),
        bone("hand_r", Some(FOREARM_R), -0.23, 0.0, 0.0),
        bone("thigh_l", Some(HIPS), 0.085, -0.03, 0.0),
        bone("shin_l", Some(THIGH_L), 0.0, -0.45, 0.0),
        bone("foot_l", Some(SHIN_L), 0.0, -0.40, 0.0),
        bone("thigh_r", Some(HIPS), -0.085, -0.03, 0.0),
        bone("shin_r", Some(THIGH_R), 0.0, -0.45, 0.0),
        bone("foot_r", Some(SHIN_R), 0.0, -0.40, 0.0),
        bone("prop", Some(HAND_R), 0.0, 0.0, 0.0),
    ];

    // Rest *is* bind for anything authored here, so the inverse binds are derived rather
    // than typed. An importer must not do this — see `Skeleton::inverse_binds_from_rest`.
    let inverses = Skeleton::inverse_binds_from_rest(&bones);
    for (bone, inverse) in bones.iter_mut().zip(inverses) {
        bone.inverse_bind = inverse;
    }
    Skeleton::new(bones)
}

/// Accumulates prisms into one submesh's worth of skinned geometry.
#[derive(Debug, Default)]
pub struct SkinBuilder {
    vertices: Vec<SkinVertex>,
    indices: Vec<u32>,
}

impl SkinBuilder {
    /// An empty builder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether nothing has been added.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty()
    }

    /// Adds a prism whose lower cap follows `lower_joint` and upper cap follows `upper_joint`.
    ///
    /// The two-joint form is what makes a knee bend instead of shearing: the vertices at the
    /// knee end of the thigh carry a fifth of the shin's motion, so the surface follows the
    /// joint rather than tearing across it. When both joints are the same bone the vertex is
    /// bound rigidly, which is what most of a stylized character wants.
    pub fn prism(&mut self, prism: &Prism, lower_joint: u16, upper_joint: u16) {
        /// How much of the *other* bone a cap's vertices carry.
        const CROSS_WEIGHT: f32 = 0.18;

        let base = self.vertices.len() as u32;
        let (corners, indices) = prism.build(base);
        for (index, (position, normal, uv)) in corners.into_iter().enumerate() {
            // `Prism::build` emits four side quads as [lower, lower, upper, upper], then the
            // upper cap, then the lower cap.
            let at_upper = match index {
                0..16 => index % 4 >= 2,
                16..20 => true,
                _ => false,
            };
            let vertex = if lower_joint == upper_joint {
                SkinVertex::rigid(position, normal, uv, lower_joint)
            } else if at_upper {
                SkinVertex::blended(position, normal, uv, upper_joint, lower_joint, CROSS_WEIGHT)
            } else {
                SkinVertex::blended(position, normal, uv, lower_joint, upper_joint, CROSS_WEIGHT)
            };
            self.vertices.push(vertex.normalized());
        }
        self.indices.extend(indices);
    }

    /// Finishes the builder as one submesh drawn with `material`.
    #[must_use]
    pub fn into_submesh(self, name: &str, material: usize) -> SubMesh {
        SubMesh::new(
            name,
            Geometry::Skinned(self.vertices),
            self.indices,
            material,
        )
    }
}

/// A prism between two points with rectangular caps.
#[must_use]
pub fn limb(lower: Vec3, upper: Vec3, lower_half: Vec2, upper_half: Vec2) -> Prism {
    Prism::new(lower, upper, lower_half.x, upper_half.x).with_halves(lower_half, upper_half)
}

/// Degrees to a rotation, in the yaw/pitch/roll order the animations are written in.
///
/// Pitch is X — a leg or an arm swinging forwards and back. Roll is Z — an arm coming down
/// from the T-pose. Yaw is Y — a torso turning. Writing clips in degrees keeps them
/// readable; the conversion happens once, at build.
#[must_use]
pub fn ypr(yaw: f32, pitch: f32, roll: f32) -> Quat {
    Quat::from_yaw_pitch_roll(yaw.to_radians(), pitch.to_radians(), roll.to_radians())
}

/// A rotation track from `(time, yaw, pitch, roll)` tuples in degrees.
#[must_use]
pub fn rotation(bone: u16, keys: &[(f32, f32, f32, f32)]) -> BoneTrack {
    BoneTrack::new(bone).with_rotation(
        keys.iter()
            .map(|(time, yaw, pitch, roll)| Key::new(*time, ypr(*yaw, *pitch, *roll)))
            .collect(),
    )
}

/// A translation track from `(time, x, y, z)` tuples in metres.
#[must_use]
pub fn translation(bone: u16, keys: &[(f32, f32, f32, f32)]) -> BoneTrack {
    BoneTrack::new(bone).with_translation(
        keys.iter()
            .map(|(time, x, y, z)| Key::new(*time, Vec3::new(*x, *y, *z)))
            .collect(),
    )
}

/// How a character throws its abilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CastStyle {
    /// A weapon comes over the shoulder and comes down. The Warrior.
    Swing,
    /// Both hands rise and push forward. The Druid.
    Channel,
    /// A bow raised in the left hand and drawn to the cheek with the right. The Crypt Archer.
    Bow,
    /// Both fists raised overhead and brought down together in front. The Grave Brute.
    Slam,
    /// A lunge and a raking swipe of the claws. The Crypt Ghoul.
    Claw,
}

/// Everything a character is allowed to change about the shared animation set.
///
/// Amplitudes and rest poses only — see the module documentation for why structure is not on
/// this list.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    /// Roll, in degrees, that brings the left arm down from the T-pose. Negative.
    pub arm_down_l: f32,
    /// The mirror of `arm_down_l` for the right arm. Positive.
    pub arm_down_r: f32,
    /// Seconds one walk cycle takes.
    pub walk_period: f32,
    /// Seconds one run cycle takes.
    pub run_period: f32,
    /// Degrees the thigh swings in a walk. The run scales this.
    pub stride: f32,
    /// Degrees the knee bends in a walk.
    pub knee: f32,
    /// Degrees the arms counter-swing in a walk.
    pub arm_swing: f32,
    /// Metres the hips rise and fall in a walk.
    pub bob: f32,
    /// Degrees of forward lean in a run.
    pub lean: f32,
    /// Seconds one breath of the idle takes, doubled for the whole loop.
    pub breath: f32,
    /// How the character casts.
    pub cast: CastStyle,
}

impl Default for Style {
    /// A neutral human: the FireMage's amplitudes, which is what the numbers were tuned as.
    fn default() -> Self {
        Self {
            arm_down_l: -78.0,
            arm_down_r: 78.0,
            walk_period: 1.0,
            run_period: 0.62,
            stride: 22.0,
            knee: 26.0,
            arm_swing: 18.0,
            bob: 0.022,
            lean: 4.0,
            breath: 1.6,
            cast: CastStyle::Channel,
        }
    }
}

/// Every clip a character built on this rig ships with, in [`crate::character::AnimationSlot`]
/// order.
#[must_use]
pub fn clips(style: &Style) -> Vec<AnimationClip> {
    vec![
        idle(style),
        locomotion(
            "walk",
            style,
            style.walk_period,
            style.stride,
            style.knee,
            style.arm_swing,
            style.bob,
            style.lean * 0.4,
        ),
        locomotion(
            "run",
            style,
            style.run_period,
            style.stride * 2.0,
            style.knee * 2.0,
            style.arm_swing * 2.3,
            style.bob * 2.0,
            style.lean * 3.0,
        ),
        jump(style),
        fall(style),
        land(style),
        cast(style),
        cast_heavy(style),
    ]
}

/// Standing: a slow breath, a small sway, and the arms hanging.
fn idle(style: &Style) -> AnimationClip {
    let (half, full) = (style.breath, style.breath * 2.0);
    AnimationClip::new(
        "idle",
        true,
        vec![
            translation(
                HIPS,
                &[
                    (0.0, 0.0, HIPS_REST_Y, 0.0),
                    (half, 0.0, HIPS_REST_Y + 0.014, 0.0),
                    (full, 0.0, HIPS_REST_Y, 0.0),
                ],
            ),
            rotation(
                CHEST,
                &[
                    (0.0, 0.0, 1.5, 0.0),
                    (half, 0.0, -1.5, 0.0),
                    (full, 0.0, 1.5, 0.0),
                ],
            ),
            rotation(
                HEAD,
                &[
                    (0.0, 2.0, 0.0, 0.0),
                    (half, -2.0, 2.0, 0.0),
                    (full, 2.0, 0.0, 0.0),
                ],
            ),
            rotation(
                SHOULDER_L,
                &[
                    (0.0, 0.0, 0.0, style.arm_down_l),
                    (half, 0.0, 3.0, style.arm_down_l - 2.0),
                    (full, 0.0, 0.0, style.arm_down_l),
                ],
            ),
            rotation(
                SHOULDER_R,
                &[
                    (0.0, 0.0, 0.0, style.arm_down_r),
                    (half, 0.0, 3.0, style.arm_down_r + 2.0),
                    (full, 0.0, 0.0, style.arm_down_r),
                ],
            ),
            rotation(FOREARM_L, &[(0.0, 0.0, 0.0, -12.0)]),
            rotation(FOREARM_R, &[(0.0, 0.0, 0.0, 12.0)]),
        ],
    )
}

/// The shape both locomotion cycles share.
///
/// One function rather than two hand-written clips because a walk and a run differ in
/// amplitude and period, not in structure — and two copies of a cycle drift apart the first
/// time one of them is tuned.
#[allow(
    clippy::too_many_arguments,
    reason = "the amplitudes of one cycle; grouping them into a struct would be the same \
              arguments with a name in front of each"
)]
fn locomotion(
    name: &str,
    style: &Style,
    period: f32,
    stride: f32,
    knee: f32,
    arm_swing: f32,
    bob: f32,
    lean: f32,
) -> AnimationClip {
    let half = period * 0.5;
    let quarter = period * 0.25;
    let three_quarters = period * 0.75;

    // The legs are in opposite phase; the arms oppose the leg on their own side.
    let leg = |bone: u16, phase: f32| {
        rotation(
            bone,
            &[
                (0.0, 0.0, phase * stride, 0.0),
                (half, 0.0, -phase * stride, 0.0),
                (period, 0.0, phase * stride, 0.0),
            ],
        )
    };
    // A knee only ever bends one way, so the shin's swing is offset a quarter period and
    // clamped to one sign — a shin that bends forwards is the single most obvious way for a
    // walk cycle to look wrong.
    let shin = |bone: u16, phase: f32| {
        let (a, b) = if phase > 0.0 {
            (quarter, three_quarters)
        } else {
            (three_quarters, quarter)
        };
        BoneTrack::new(bone).with_rotation(vec![
            Key::new(0.0, ypr(0.0, -knee * 0.25, 0.0)),
            Key::new(a, ypr(0.0, -knee, 0.0)),
            Key::new(b, ypr(0.0, -knee * 0.1, 0.0)),
            Key::new(period, ypr(0.0, -knee * 0.25, 0.0)),
        ])
    };
    let arm = |bone: u16, rest: f32, phase: f32| {
        rotation(
            bone,
            &[
                (0.0, 0.0, phase * arm_swing, rest),
                (half, 0.0, -phase * arm_swing, rest),
                (period, 0.0, phase * arm_swing, rest),
            ],
        )
    };

    AnimationClip::new(
        name,
        true,
        vec![
            // Two bobs per cycle: the body rises on each step, not once per pair of steps.
            translation(
                HIPS,
                &[
                    (0.0, 0.0, HIPS_REST_Y - bob, 0.0),
                    (quarter, 0.0, HIPS_REST_Y + bob, 0.0),
                    (half, 0.0, HIPS_REST_Y - bob, 0.0),
                    (three_quarters, 0.0, HIPS_REST_Y + bob, 0.0),
                    (period, 0.0, HIPS_REST_Y - bob, 0.0),
                ],
            ),
            rotation(SPINE, &[(0.0, 0.0, lean, 0.0)]),
            rotation(
                CHEST,
                &[
                    (0.0, -lean * 0.3, 0.0, 0.0),
                    (half, lean * 0.3, 0.0, 0.0),
                    (period, -lean * 0.3, 0.0, 0.0),
                ],
            ),
            leg(THIGH_L, 1.0),
            leg(THIGH_R, -1.0),
            shin(SHIN_L, 1.0),
            shin(SHIN_R, -1.0),
            arm(SHOULDER_L, style.arm_down_l, -1.0),
            arm(SHOULDER_R, style.arm_down_r, 1.0),
            rotation(FOREARM_L, &[(0.0, 0.0, -20.0, -12.0)]),
            rotation(FOREARM_R, &[(0.0, 0.0, -20.0, 12.0)]),
        ],
    )
}

/// Leaving the ground: a push through the legs and the arms coming up.
fn jump(style: &Style) -> AnimationClip {
    AnimationClip::new(
        "jump",
        false,
        vec![
            translation(
                HIPS,
                &[
                    (0.0, 0.0, HIPS_REST_Y - 0.06, 0.0),
                    (0.12, 0.0, HIPS_REST_Y + 0.03, 0.0),
                    (0.40, 0.0, HIPS_REST_Y + 0.01, 0.0),
                ],
            ),
            rotation(
                THIGH_L,
                &[
                    (0.0, 0.0, 30.0, 0.0),
                    (0.16, 0.0, -12.0, 0.0),
                    (0.40, 0.0, 14.0, 0.0),
                ],
            ),
            rotation(
                THIGH_R,
                &[
                    (0.0, 0.0, 26.0, 0.0),
                    (0.16, 0.0, -14.0, 0.0),
                    (0.40, 0.0, 6.0, 0.0),
                ],
            ),
            rotation(
                SHIN_L,
                &[
                    (0.0, 0.0, -46.0, 0.0),
                    (0.16, 0.0, -6.0, 0.0),
                    (0.40, 0.0, -30.0, 0.0),
                ],
            ),
            rotation(
                SHIN_R,
                &[
                    (0.0, 0.0, -42.0, 0.0),
                    (0.16, 0.0, -8.0, 0.0),
                    (0.40, 0.0, -24.0, 0.0),
                ],
            ),
            rotation(
                SHOULDER_L,
                &[
                    (0.0, 0.0, 0.0, style.arm_down_l),
                    (0.20, 0.0, 0.0, style.arm_down_l + 46.0),
                ],
            ),
            rotation(
                SHOULDER_R,
                &[
                    (0.0, 0.0, 0.0, style.arm_down_r),
                    (0.20, 0.0, 0.0, style.arm_down_r - 46.0),
                ],
            ),
            rotation(SPINE, &[(0.0, 0.0, 12.0, 0.0), (0.20, 0.0, -6.0, 0.0)]),
        ],
    )
}

/// Airborne and descending: legs trailing, arms out for balance. Loops, because a fall has
/// no natural length.
fn fall(style: &Style) -> AnimationClip {
    AnimationClip::new(
        "fall",
        true,
        vec![
            rotation(
                SHOULDER_L,
                &[
                    (0.0, 0.0, 0.0, style.arm_down_l + 34.0),
                    (0.4, 0.0, 6.0, style.arm_down_l + 40.0),
                    (0.8, 0.0, 0.0, style.arm_down_l + 34.0),
                ],
            ),
            rotation(
                SHOULDER_R,
                &[
                    (0.0, 0.0, 0.0, style.arm_down_r - 34.0),
                    (0.4, 0.0, 6.0, style.arm_down_r - 40.0),
                    (0.8, 0.0, 0.0, style.arm_down_r - 34.0),
                ],
            ),
            rotation(THIGH_L, &[(0.0, 0.0, -16.0, 0.0), (0.8, 0.0, -16.0, 0.0)]),
            rotation(THIGH_R, &[(0.0, 0.0, 10.0, 0.0), (0.8, 0.0, 10.0, 0.0)]),
            rotation(SHIN_L, &[(0.0, 0.0, -34.0, 0.0), (0.8, 0.0, -34.0, 0.0)]),
            rotation(SHIN_R, &[(0.0, 0.0, -18.0, 0.0), (0.8, 0.0, -18.0, 0.0)]),
            rotation(SPINE, &[(0.0, 0.0, -8.0, 0.0), (0.8, 0.0, -8.0, 0.0)]),
        ],
    )
}

/// Touching down: a compression through the knees that recovers.
///
/// A heavier character compresses further and takes longer to come back up, which is the one
/// place [`Style::bob`] earns its keep outside a walk.
fn land(style: &Style) -> AnimationClip {
    let depth = 0.13 * (style.bob / 0.022).clamp(0.7, 1.6);
    AnimationClip::new(
        "land",
        false,
        vec![
            translation(
                HIPS,
                &[
                    (0.0, 0.0, HIPS_REST_Y - 0.02, 0.0),
                    (0.08, 0.0, HIPS_REST_Y - depth, 0.0),
                    (0.28, 0.0, HIPS_REST_Y, 0.0),
                ],
            ),
            rotation(
                THIGH_L,
                &[
                    (0.0, 0.0, 8.0, 0.0),
                    (0.08, 0.0, 28.0, 0.0),
                    (0.28, 0.0, 0.0, 0.0),
                ],
            ),
            rotation(
                THIGH_R,
                &[
                    (0.0, 0.0, 8.0, 0.0),
                    (0.08, 0.0, 26.0, 0.0),
                    (0.28, 0.0, 0.0, 0.0),
                ],
            ),
            rotation(
                SHIN_L,
                &[
                    (0.0, 0.0, -14.0, 0.0),
                    (0.08, 0.0, -48.0, 0.0),
                    (0.28, 0.0, 0.0, 0.0),
                ],
            ),
            rotation(
                SHIN_R,
                &[
                    (0.0, 0.0, -14.0, 0.0),
                    (0.08, 0.0, -46.0, 0.0),
                    (0.28, 0.0, 0.0, 0.0),
                ],
            ),
            rotation(
                SPINE,
                &[
                    (0.0, 0.0, 10.0, 0.0),
                    (0.08, 0.0, 18.0, 0.0),
                    (0.28, 0.0, 0.0, 0.0),
                ],
            ),
        ],
    )
}

/// An ordinary ability: a swing for the Warrior, a push of both hands for the Druid.
fn cast(style: &Style) -> AnimationClip {
    let tracks = match style.cast {
        CastStyle::Swing => vec![
            // Over the shoulder, then down and across. The prop bone hangs off the right
            // hand, so the weapon travels with it and needs no track of its own.
            rotation(
                SHOULDER_R,
                &[
                    (0.0, 0.0, 0.0, style.arm_down_r),
                    (0.18, 0.0, -40.0, style.arm_down_r - 86.0),
                    (0.38, 0.0, 30.0, style.arm_down_r - 24.0),
                    (0.70, 0.0, 0.0, style.arm_down_r),
                ],
            ),
            rotation(
                FOREARM_R,
                &[
                    (0.0, 0.0, 0.0, 12.0),
                    (0.18, 0.0, -50.0, 20.0),
                    (0.38, 0.0, -6.0, 8.0),
                    (0.70, 0.0, 0.0, 12.0),
                ],
            ),
            rotation(
                SHOULDER_L,
                &[
                    (0.0, 0.0, 0.0, style.arm_down_l),
                    (0.18, 0.0, 14.0, style.arm_down_l + 16.0),
                    (0.38, 0.0, -22.0, style.arm_down_l + 6.0),
                    (0.70, 0.0, 0.0, style.arm_down_l),
                ],
            ),
            // The turn through the body is where the weight of a swing reads from.
            rotation(
                CHEST,
                &[
                    (0.0, 0.0, 0.0, 0.0),
                    (0.18, 26.0, -8.0, 0.0),
                    (0.38, -24.0, 10.0, 0.0),
                    (0.70, 0.0, 0.0, 0.0),
                ],
            ),
            rotation(
                SPINE,
                &[
                    (0.0, 0.0, 0.0, 0.0),
                    (0.18, 10.0, -4.0, 0.0),
                    (0.38, -10.0, 8.0, 0.0),
                    (0.70, 0.0, 0.0, 0.0),
                ],
            ),
        ],
        CastStyle::Channel => vec![
            rotation(
                SHOULDER_R,
                &[
                    (0.0, 0.0, 0.0, style.arm_down_r),
                    (0.22, 0.0, -70.0, style.arm_down_r - 26.0),
                    (0.42, 0.0, -96.0, style.arm_down_r - 18.0),
                    (0.70, 0.0, 0.0, style.arm_down_r),
                ],
            ),
            rotation(
                SHOULDER_L,
                &[
                    (0.0, 0.0, 0.0, style.arm_down_l),
                    (0.22, 0.0, -70.0, style.arm_down_l + 26.0),
                    (0.42, 0.0, -96.0, style.arm_down_l + 18.0),
                    (0.70, 0.0, 0.0, style.arm_down_l),
                ],
            ),
            rotation(
                CHEST,
                &[
                    (0.0, 0.0, 0.0, 0.0),
                    (0.42, 0.0, 6.0, 0.0),
                    (0.70, 0.0, 0.0, 0.0),
                ],
            ),
            rotation(
                HEAD,
                &[
                    (0.0, 0.0, 0.0, 0.0),
                    (0.42, 0.0, -8.0, 0.0),
                    (0.70, 0.0, 0.0, 0.0),
                ],
            ),
        ],
        CastStyle::Bow => bow_draw(style, false),
        CastStyle::Slam => slam(style, false),
        CastStyle::Claw => claw(style, false),
    };
    AnimationClip::new("cast", false, tracks)
}

/// The big one: wound up further, held longer, and driven forward.
fn cast_heavy(style: &Style) -> AnimationClip {
    let tracks = match style.cast {
        CastStyle::Swing => vec![
            rotation(
                SHOULDER_R,
                &[
                    (0.0, 0.0, 0.0, style.arm_down_r),
                    (0.42, 0.0, -60.0, style.arm_down_r - 110.0),
                    (0.66, 0.0, -50.0, style.arm_down_r - 116.0),
                    (0.86, 0.0, 44.0, style.arm_down_r - 20.0),
                    (1.20, 0.0, 0.0, style.arm_down_r),
                ],
            ),
            rotation(
                SHOULDER_L,
                &[
                    (0.0, 0.0, 0.0, style.arm_down_l),
                    (0.42, 0.0, -40.0, style.arm_down_l + 84.0),
                    (0.66, 0.0, -34.0, style.arm_down_l + 90.0),
                    (0.86, 0.0, 30.0, style.arm_down_l + 14.0),
                    (1.20, 0.0, 0.0, style.arm_down_l),
                ],
            ),
            rotation(
                SPINE,
                &[
                    (0.0, 0.0, 0.0, 0.0),
                    (0.42, 0.0, -20.0, 0.0),
                    (0.86, 0.0, 26.0, 0.0),
                    (1.20, 0.0, 0.0, 0.0),
                ],
            ),
            translation(
                HIPS,
                &[
                    (0.0, 0.0, HIPS_REST_Y, 0.0),
                    (0.42, 0.0, HIPS_REST_Y + 0.03, 0.0),
                    (0.86, 0.0, HIPS_REST_Y - 0.08, 0.0),
                    (1.20, 0.0, HIPS_REST_Y, 0.0),
                ],
            ),
        ],
        CastStyle::Channel => vec![
            rotation(
                SHOULDER_R,
                &[
                    (0.0, 0.0, 0.0, style.arm_down_r),
                    (0.40, 0.0, -30.0, style.arm_down_r - 78.0),
                    (0.72, 0.0, -110.0, style.arm_down_r - 30.0),
                    (1.10, 0.0, 0.0, style.arm_down_r),
                ],
            ),
            rotation(
                SHOULDER_L,
                &[
                    (0.0, 0.0, 0.0, style.arm_down_l),
                    (0.40, 0.0, -30.0, style.arm_down_l + 78.0),
                    (0.72, 0.0, -110.0, style.arm_down_l + 30.0),
                    (1.10, 0.0, 0.0, style.arm_down_l),
                ],
            ),
            rotation(
                SPINE,
                &[
                    (0.0, 0.0, 0.0, 0.0),
                    (0.40, 0.0, -16.0, 0.0),
                    (0.72, 0.0, 20.0, 0.0),
                    (1.10, 0.0, 0.0, 0.0),
                ],
            ),
            translation(
                HIPS,
                &[
                    (0.0, 0.0, HIPS_REST_Y, 0.0),
                    (0.40, 0.0, HIPS_REST_Y + 0.03, 0.0),
                    (0.72, 0.0, HIPS_REST_Y - 0.06, 0.0),
                    (1.10, 0.0, HIPS_REST_Y, 0.0),
                ],
            ),
        ],
        CastStyle::Bow => bow_draw(style, true),
        CastStyle::Slam => slam(style, true),
        CastStyle::Claw => claw(style, true),
    };
    AnimationClip::new("cast_heavy", false, tracks)
}

/// The bow: raised in the left hand, drawn to the cheek with the right, loosed.
///
/// The left arm points forward by turning 90 degrees about the vertical; the right arm
/// reaches forward and its forearm folds back, which is what brings the drawing hand to the
/// shoulder. The heavy version aims into the sky — a volley, a rain of arrows — by rolling
/// both arms up and leaning back.
fn bow_draw(style: &Style, heavy: bool) -> Vec<BoneTrack> {
    let (aim, lean) = if heavy { (38.0, 10.0) } else { (4.0, 0.0) };
    let (draw, loose, end) = if heavy {
        (0.45, 0.80, 1.15)
    } else {
        (0.28, 0.48, 0.80)
    };
    vec![
        rotation(
            SHOULDER_L,
            &[
                (0.0, 0.0, 0.0, style.arm_down_l),
                (draw, 88.0, 0.0, aim),
                (loose, 88.0, 0.0, aim),
                (end, 0.0, 0.0, style.arm_down_l),
            ],
        ),
        rotation(
            FOREARM_L,
            // The twist about the forearm that stands the bow upright: it is carried
            // across the body at rest and has to turn a quarter to be drawn.
            &[
                (0.0, 0.0, 0.0, -12.0),
                (draw, 0.0, 90.0, 0.0),
                (loose, 0.0, 90.0, 0.0),
                (end, 0.0, 0.0, -12.0),
            ],
        ),
        rotation(
            SHOULDER_R,
            &[
                (0.0, 0.0, 0.0, style.arm_down_r),
                (draw * 0.5, -40.0, 0.0, style.arm_down_r - 40.0),
                (draw, -72.0, 0.0, -aim),
                (loose, -72.0, 0.0, -aim),
                (loose + 0.08, -92.0, 0.0, -aim - 10.0),
                (end, 0.0, 0.0, style.arm_down_r),
            ],
        ),
        rotation(
            FOREARM_R,
            &[
                (0.0, 0.0, 0.0, 12.0),
                (draw, 150.0, 0.0, 0.0),
                (loose, 158.0, 0.0, 0.0),
                (loose + 0.08, 60.0, 0.0, 0.0),
                (end, 0.0, 0.0, 12.0),
            ],
        ),
        // Side-on to the target, as an archer stands.
        rotation(
            CHEST,
            &[
                (0.0, 0.0, 0.0, 0.0),
                (draw, 18.0, lean, 0.0),
                (loose, 18.0, lean, 0.0),
                (end, 0.0, 0.0, 0.0),
            ],
        ),
        rotation(
            HEAD,
            &[
                (0.0, 0.0, 0.0, 0.0),
                (draw, -16.0, 0.0, 0.0),
                (end, 0.0, 0.0, 0.0),
            ],
        ),
    ]
}

/// The slam: both fists up and back over the head, then down together in front.
fn slam(style: &Style, heavy: bool) -> Vec<BoneTrack> {
    let (lift, hold, strike, end) = if heavy {
        (0.50, 0.70, 0.86, 1.30)
    } else {
        (0.30, 0.36, 0.46, 0.90)
    };
    let drop = if heavy { 0.16 } else { 0.08 };
    vec![
        rotation(
            SHOULDER_L,
            &[
                (0.0, 0.0, 0.0, style.arm_down_l),
                (lift, 0.0, 24.0, 88.0),
                (hold, 0.0, 30.0, 92.0),
                (strike, 0.0, 70.0, style.arm_down_l),
                (end, 0.0, 0.0, style.arm_down_l),
            ],
        ),
        rotation(
            SHOULDER_R,
            &[
                (0.0, 0.0, 0.0, style.arm_down_r),
                (lift, 0.0, 24.0, -88.0),
                (hold, 0.0, 30.0, -92.0),
                (strike, 0.0, 70.0, style.arm_down_r),
                (end, 0.0, 0.0, style.arm_down_r),
            ],
        ),
        rotation(
            FOREARM_L,
            &[
                (0.0, 0.0, 0.0, -12.0),
                (lift, 0.0, 0.0, 20.0),
                (strike, 0.0, 0.0, -4.0),
                (end, 0.0, 0.0, -12.0),
            ],
        ),
        rotation(
            FOREARM_R,
            &[
                (0.0, 0.0, 0.0, 12.0),
                (lift, 0.0, 0.0, -20.0),
                (strike, 0.0, 0.0, 4.0),
                (end, 0.0, 0.0, 12.0),
            ],
        ),
        // Back on the lift, over and down on the strike: the weight is in the spine.
        rotation(
            SPINE,
            &[
                (0.0, 0.0, 0.0, 0.0),
                (lift, 0.0, 12.0, 0.0),
                (hold, 0.0, 14.0, 0.0),
                (strike, 0.0, -30.0, 0.0),
                (end, 0.0, 0.0, 0.0),
            ],
        ),
        translation(
            HIPS,
            &[
                (0.0, 0.0, HIPS_REST_Y, 0.0),
                (lift, 0.0, HIPS_REST_Y + 0.03, 0.0),
                (strike, 0.0, HIPS_REST_Y - drop, 0.0),
                (end, 0.0, HIPS_REST_Y, 0.0),
            ],
        ),
        rotation(
            THIGH_L,
            &[
                (0.0, 0.0, 0.0, 0.0),
                (strike, 0.0, 26.0, 0.0),
                (end, 0.0, 0.0, 0.0),
            ],
        ),
        rotation(
            THIGH_R,
            &[
                (0.0, 0.0, 0.0, 0.0),
                (strike, 0.0, 20.0, 0.0),
                (end, 0.0, 0.0, 0.0),
            ],
        ),
        rotation(
            SHIN_L,
            &[
                (0.0, 0.0, 0.0, 0.0),
                (strike, 0.0, -40.0, 0.0),
                (end, 0.0, 0.0, 0.0),
            ],
        ),
        rotation(
            SHIN_R,
            &[
                (0.0, 0.0, 0.0, 0.0),
                (strike, 0.0, -34.0, 0.0),
                (end, 0.0, 0.0, 0.0),
            ],
        ),
    ]
}

/// The claw: a lunge, and a raking swipe across the front — right, then left on the heavy
/// version, which pounces.
fn claw(style: &Style, heavy: bool) -> Vec<BoneTrack> {
    let (wind, rake, end) = if heavy {
        (0.30, 0.52, 0.95)
    } else {
        (0.14, 0.24, 0.50)
    };
    let second = if heavy { rake + 0.14 } else { end };
    let mut tracks = vec![
        rotation(
            SHOULDER_R,
            &[
                (0.0, 0.0, 0.0, style.arm_down_r),
                (wind, 0.0, -10.0, style.arm_down_r - 120.0),
                (rake, 40.0, 80.0, style.arm_down_r - 10.0),
                (end, 0.0, 0.0, style.arm_down_r),
            ],
        ),
        rotation(
            FOREARM_R,
            &[
                (0.0, 0.0, 0.0, 12.0),
                (wind, 0.0, -30.0, 30.0),
                (rake, 0.0, 0.0, 6.0),
                (end, 0.0, 0.0, 12.0),
            ],
        ),
        rotation(
            CHEST,
            &[
                (0.0, 0.0, 0.0, 0.0),
                (wind, 24.0, 0.0, 0.0),
                (rake, -26.0, -10.0, 0.0),
                (end, 0.0, 0.0, 0.0),
            ],
        ),
        // The lunge: down and forward through the hips as the claw comes across.
        translation(
            HIPS,
            &[
                (0.0, 0.0, HIPS_REST_Y, 0.0),
                (wind, 0.0, HIPS_REST_Y - 0.03, 0.0),
                (rake, 0.0, HIPS_REST_Y - 0.08, -0.12),
                (end, 0.0, HIPS_REST_Y, 0.0),
            ],
        ),
    ];
    let left = if heavy {
        rotation(
            SHOULDER_L,
            &[
                (0.0, 0.0, 0.0, style.arm_down_l),
                (wind, 0.0, -10.0, style.arm_down_l + 120.0),
                (rake, 0.0, -10.0, style.arm_down_l + 110.0),
                (second, -40.0, 80.0, style.arm_down_l + 10.0),
                (end, 0.0, 0.0, style.arm_down_l),
            ],
        )
    } else {
        rotation(
            SHOULDER_L,
            &[
                (0.0, 0.0, 0.0, style.arm_down_l),
                (wind, 0.0, 20.0, style.arm_down_l + 20.0),
                (rake, 0.0, -20.0, style.arm_down_l + 10.0),
                (end, 0.0, 0.0, style.arm_down_l),
            ],
        )
    };
    tracks.push(left);
    if heavy {
        tracks.push(rotation(
            SPINE,
            &[
                (0.0, 0.0, 0.0, 0.0),
                (wind, 0.0, 10.0, 0.0),
                (rake, 0.0, -24.0, 0.0),
                (end, 0.0, 0.0, 0.0),
            ],
        ));
    }
    tracks
}

/// Where in each cast clip the blow lands — the moment the wind-up is placed against the
/// cast's progress, and the moment an instant attack's follow-through plays out from.
///
/// `(cast, cast_heavy)`, in seconds.
#[must_use]
pub fn release_times(style: CastStyle) -> (f32, f32) {
    match style {
        CastStyle::Swing => (0.38, 0.86),
        CastStyle::Channel => (0.42, 0.72),
        CastStyle::Bow => (0.48, 0.80),
        CastStyle::Slam => (0.46, 0.86),
        CastStyle::Claw => (0.24, 0.52),
    }
}

/// Dying: the knees go, the body slumps, and it falls back to lie on the ground.
///
/// The fall is a rotation of the root about the feet, which is what keeps the body lying
/// where the monster stood; the root rises a hand's breadth as it goes, so the back comes to
/// rest on the floor instead of half inside it. One-shot, and it holds its last frame.
#[must_use]
pub fn death(style: &Style) -> AnimationClip {
    let (buckle, down, end) = (0.30, 0.85, 1.05);
    AnimationClip::new(
        "death",
        false,
        vec![
            rotation(
                ROOT,
                &[
                    (0.0, 0.0, 0.0, 0.0),
                    (buckle, 0.0, 6.0, 0.0),
                    (down, 0.0, 84.0, 0.0),
                    (end, 0.0, 88.0, 0.0),
                ],
            ),
            translation(
                ROOT,
                &[
                    (0.0, 0.0, 0.0, 0.0),
                    (buckle, 0.0, 0.0, 0.0),
                    (down, 0.0, 0.10, 0.0),
                    (end, 0.0, 0.12, 0.0),
                ],
            ),
            translation(
                HIPS,
                &[
                    (0.0, 0.0, HIPS_REST_Y, 0.0),
                    (buckle, 0.0, HIPS_REST_Y - 0.16, 0.0),
                    (end, 0.0, HIPS_REST_Y - 0.04, 0.0),
                ],
            ),
            rotation(
                SPINE,
                &[
                    (0.0, 0.0, 0.0, 0.0),
                    (buckle, 0.0, -18.0, 0.0),
                    (end, 0.0, 6.0, 0.0),
                ],
            ),
            rotation(
                HEAD,
                &[
                    (0.0, 0.0, 0.0, 0.0),
                    (buckle, 0.0, -20.0, 0.0),
                    (end, 18.0, 16.0, 0.0),
                ],
            ),
            rotation(
                THIGH_L,
                &[
                    (0.0, 0.0, 0.0, 0.0),
                    (buckle, 0.0, 32.0, 0.0),
                    (end, 0.0, 14.0, 4.0),
                ],
            ),
            rotation(
                THIGH_R,
                &[
                    (0.0, 0.0, 0.0, 0.0),
                    (buckle, 0.0, 26.0, 0.0),
                    (end, 0.0, 6.0, -6.0),
                ],
            ),
            rotation(
                SHIN_L,
                &[
                    (0.0, 0.0, 0.0, 0.0),
                    (buckle, 0.0, -58.0, 0.0),
                    (end, 0.0, -30.0, 0.0),
                ],
            ),
            rotation(
                SHIN_R,
                &[
                    (0.0, 0.0, 0.0, 0.0),
                    (buckle, 0.0, -50.0, 0.0),
                    (end, 0.0, -12.0, 0.0),
                ],
            ),
            rotation(
                SHOULDER_L,
                &[
                    (0.0, 0.0, 0.0, style.arm_down_l),
                    (buckle, 0.0, 20.0, style.arm_down_l + 20.0),
                    (end, 0.0, -10.0, style.arm_down_l + 58.0),
                ],
            ),
            rotation(
                SHOULDER_R,
                &[
                    (0.0, 0.0, 0.0, style.arm_down_r),
                    (buckle, 0.0, 26.0, style.arm_down_r - 16.0),
                    (end, 0.0, -14.0, style.arm_down_r - 64.0),
                ],
            ),
        ],
    )
}

/// How a monster stands: offsets laid over every clip, so a ghoul walks, casts and dies
/// crouched and a brute does all three hunched.
///
/// The rig and the cycles stay shared — the module documentation's rule — and this is the
/// one thing a monster adds: a *rest pose* different from a hero's upright one. It is
/// applied to the finished clips rather than written into each, so every clip agrees about
/// it, including the ones written before the monster existed.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Posture {
    /// Degrees the lower back bends forward.
    pub hunch: f32,
    /// Degrees the upper back bends forward, on top of the hunch.
    pub chest: f32,
    /// Degrees the head tips *back*, to look ahead out of a hunch rather than at the floor.
    pub head_up: f32,
    /// Degrees the thighs come forward in a crouch. The knees bend to keep the feet under
    /// the hips, and the hips drop by exactly what that costs in height.
    pub crouch: f32,
    /// Degrees the arms hang forward of the body.
    pub arms_forward: f32,
}

impl Posture {
    /// The knee angle and the hip drop a crouch of `self.crouch` degrees takes.
    ///
    /// The thigh swings forward by the crouch and the shin back until the foot is under the
    /// hip again, so a crouched monster neither leans off its feet nor slides forward.
    #[must_use]
    pub fn crouch_geometry(&self) -> (f32, f32) {
        const THIGH: f32 = 0.45;
        const SHIN: f32 = 0.40;
        let thigh = self.crouch.clamp(0.0, 60.0).to_radians();
        let shin = ((THIGH / SHIN) * thigh.sin()).clamp(-1.0, 1.0).asin();
        let knee = -(thigh + shin).to_degrees();
        let drop = THIGH * (1.0 - thigh.cos()) + SHIN * (1.0 - shin.cos());
        (knee, drop)
    }

    /// Lays this posture over every clip.
    pub fn apply(&self, clips: &mut [AnimationClip]) {
        let (knee, drop) = self.crouch_geometry();
        // Bones that point up lean forward with a negative pitch; the thighs, which point
        // down, come forward with a positive one — the same sign the walk cycle uses.
        let offsets = [
            (SPINE, ypr(0.0, -self.hunch, 0.0)),
            (CHEST, ypr(0.0, -self.chest, 0.0)),
            (NECK, ypr(0.0, self.head_up * 0.5, 0.0)),
            (HEAD, ypr(0.0, self.head_up * 0.5, 0.0)),
            (THIGH_L, ypr(0.0, self.crouch, 0.0)),
            (THIGH_R, ypr(0.0, self.crouch, 0.0)),
            (SHIN_L, ypr(0.0, knee, 0.0)),
            (SHIN_R, ypr(0.0, knee, 0.0)),
            (SHOULDER_L, ypr(0.0, self.arms_forward, 0.0)),
            (SHOULDER_R, ypr(0.0, self.arms_forward, 0.0)),
        ];
        for clip in clips.iter_mut() {
            for (bone, offset) in offsets {
                if offset == Quat::IDENTITY {
                    continue;
                }
                match clip.tracks.iter_mut().find(|track| track.bone == bone) {
                    Some(track) if !track.rotation.is_empty() => {
                        for key in &mut track.rotation {
                            key.value = offset * key.value;
                        }
                    }
                    Some(track) => track.rotation = vec![Key::new(0.0, offset)],
                    None => clip
                        .tracks
                        .push(BoneTrack::new(bone).with_rotation(vec![Key::new(0.0, offset)])),
                }
            }
            if drop > 1e-4 {
                let lowered = Vec3::new(0.0, -drop, 0.0);
                match clip.tracks.iter_mut().find(|track| track.bone == HIPS) {
                    Some(track) if !track.translation.is_empty() => {
                        for key in &mut track.translation {
                            key.value += lowered;
                        }
                    }
                    Some(track) => {
                        track.translation =
                            vec![Key::new(0.0, Vec3::new(0.0, HIPS_REST_Y, 0.0) + lowered)];
                    }
                    None => {
                        clip.tracks
                            .push(BoneTrack::new(HIPS).with_translation(vec![Key::new(
                                0.0,
                                Vec3::new(0.0, HIPS_REST_Y, 0.0) + lowered,
                            )]))
                    }
                }
            }
        }
    }
}

/// Every clip a hero stand-in built on this rig ships with: the shared set and a death.
///
/// Only a stand-in: a hero delivered as a Mixamo `.glb` brings its own `death` clip.
#[must_use]
pub fn hero_clips(style: &Style) -> Vec<AnimationClip> {
    let mut clips = clips(style);
    clips.push(death(style));
    clips
}

/// Every clip a monster built on this rig ships with: the hero set, a death, the blow's
/// moment marked in both casts, and the monster's posture over all of it.
#[must_use]
pub fn monster_clips(style: &Style, posture: &Posture) -> Vec<AnimationClip> {
    let (cast_release, heavy_release) = release_times(style.cast);
    let mut clips: Vec<AnimationClip> = clips(style)
        .into_iter()
        .map(|clip| match clip.name.as_str() {
            "cast" => clip.with_release(cast_release),
            "cast_heavy" => clip.with_release(heavy_release),
            _ => clip,
        })
        .collect();
    clips.push(death(style));
    posture.apply(&mut clips);
    clips
}

/// Deterministic value in `0..1` from a pair of integers.
///
/// A hash rather than a random number generator: the same texture has to come out of every
/// run, on every machine, or a screenshot comparison is meaningless.
#[must_use]
pub fn hash01(x: u32, y: u32) -> f32 {
    let mut value = x.wrapping_mul(0x9E37_79B9) ^ y.wrapping_mul(0x85EB_CA6B);
    value ^= value >> 13;
    value = value.wrapping_mul(0xC2B2_AE35);
    value ^= value >> 16;
    (value & 0xFFFF) as f32 / 65535.0
}

/// Mixes two colours.
#[must_use]
pub fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Packs a colour into an opaque sRGB texel.
#[must_use]
pub fn texel(color: [f32; 3]) -> [u8; 4] {
    [
        (color[0].clamp(0.0, 1.0) * 255.0) as u8,
        (color[1].clamp(0.0, 1.0) * 255.0) as u8,
        (color[2].clamp(0.0, 1.0) * 255.0) as u8,
        255,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::character::AnimationSlot;

    #[test]
    fn the_rig_is_the_shape_every_clip_is_written_against() {
        let Ok(skeleton) = skeleton() else {
            panic!("the shared rig must build");
        };
        assert_eq!(skeleton.len(), BONE_COUNT);
        // A parent always precedes its child, which is what lets the pose be composed in one
        // forward pass rather than by walking up the tree per bone.
        for (index, bone) in skeleton.bones().iter().enumerate() {
            if let Some(parent) = bone.parent {
                assert!(usize::from(parent) < index, "{} is out of order", bone.name);
            }
        }
        assert_eq!(skeleton.bones()[usize::from(PROP)].parent, Some(HAND_R));
    }

    #[test]
    fn every_style_fills_every_animation_slot() {
        for cast in [CastStyle::Swing, CastStyle::Channel] {
            let style = Style {
                cast,
                ..Style::default()
            };
            let clips = clips(&style);
            assert_eq!(clips.len(), AnimationSlot::REQUIRED.len());
            for name in [
                "idle",
                "walk",
                "run",
                "jump",
                "fall",
                "land",
                "cast",
                "cast_heavy",
            ] {
                assert!(
                    clips.iter().any(|clip| clip.name == name),
                    "{cast:?} has no {name}"
                );
            }
        }
    }

    #[test]
    fn the_looping_clips_are_the_ones_whose_role_repeats() {
        let clips = clips(&Style::default());
        for clip in &clips {
            let slot = match clip.name.as_str() {
                "idle" => AnimationSlot::Idle,
                "walk" => AnimationSlot::Walk,
                "run" => AnimationSlot::Run,
                "jump" => AnimationSlot::Jump,
                "fall" => AnimationSlot::Fall,
                "land" => AnimationSlot::Land,
                "cast" => AnimationSlot::Cast,
                _ => AnimationSlot::CastHeavy,
            };
            assert_eq!(clip.looping, slot.loops(), "{}", clip.name);
        }
    }

    #[test]
    fn no_clip_names_a_bone_the_rig_does_not_have() {
        for cast in [CastStyle::Swing, CastStyle::Channel] {
            let style = Style {
                cast,
                ..Style::default()
            };
            for clip in clips(&style) {
                for track in &clip.tracks {
                    assert!(
                        usize::from(track.bone) < BONE_COUNT,
                        "{} animates bone {}",
                        clip.name,
                        track.bone
                    );
                }
            }
        }
    }
}
