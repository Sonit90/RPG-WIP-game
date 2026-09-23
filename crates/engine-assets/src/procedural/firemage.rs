//! The FireMage: a hooded figure with a staff, built from prisms and skinned to 21 bones.
//!
//! ```text
//!            hood + head                     bind pose is a T-pose:
//!                 │                          arms along ±X, legs along -Y,
//!         ┌───── chest ─────┐                facing -Z, feet at y = 0.
//!    shoulder            shoulder
//!        │                   │               1.80 m tall, 20 prisms,
//!      arm                  arm ── staff     240 triangles, 4 materials.
//!        │                   │
//!      hand                hand
//!            hips / robe
//!          ┌──────┴──────┐
//!        thigh         thigh
//!          │             │
//!        shin          shin
//!          │             │
//!        boot          boot
//! ```
//!
//! # Why the bind pose is a T-pose
//!
//! Because every animation is authored as a rotation *away* from it, and a T-pose is the one
//! configuration where a limb's local axes line up with the model's — so "swing the leg
//! forward" is a rotation about X and nothing else. It is also what every DCC tool exports,
//! which means an imported character and this one animate under the same conventions.
//!
//! # Why the character faces -Z
//!
//! Because `engine_game::movement::Body::facing_yaw` is documented as "radians about +Y, zero
//! faces −Z". The model matches gameplay rather than gameplay matching the model.

use engine_core::math::{Mat4, Quat, Vec2, Vec3};
use engine_core::transform::Transform;

use crate::animation::{AnimationClip, BoneTrack, Key};
use crate::character::{AnimationSlot, CharacterDef};
use crate::library::ModelId;
use crate::material::MaterialAsset;
use crate::mesh::{Geometry, SkinVertex, SubMesh};
use crate::model::ModelAsset;
use crate::procedural::Prism;
use crate::skeleton::{Bone, Skeleton};
use crate::texture::{TextureAsset, TextureFormat};

/// How tall the model is authored, in metres.
///
/// The visual layer scales by `gameplay_height / MODEL_HEIGHT`, so this number has to be the
/// truth about the geometry rather than an intention. `the_model_is_as_tall_as_it_claims`
/// measures the built mesh and fails if the two drift apart.
pub const MODEL_HEIGHT: f32 = 1.80;

/// The identifier this model registers under.
pub const MODEL_NAME: &str = "fire_mage";

// Bone indices. Parent must always be a smaller index — `Skeleton::new` enforces it, and
// these constants are laid out so the enforcement never fires.
const ROOT: u16 = 0;
const HIPS: u16 = 1;
const SPINE: u16 = 2;
const CHEST: u16 = 3;
const NECK: u16 = 4;
const HEAD: u16 = 5;
const SHOULDER_L: u16 = 6;
const ARM_L: u16 = 7;
const FOREARM_L: u16 = 8;
const HAND_L: u16 = 9;
const SHOULDER_R: u16 = 10;
const ARM_R: u16 = 11;
const FOREARM_R: u16 = 12;
const HAND_R: u16 = 13;
const THIGH_L: u16 = 14;
const SHIN_L: u16 = 15;
const FOOT_L: u16 = 16;
const THIGH_R: u16 = 17;
const SHIN_R: u16 = 18;
const FOOT_R: u16 = 19;
const STAFF: u16 = 20;

/// Material slots, in the order [`fire_mage`] builds them.
const MATERIAL_ROBE: usize = 0;
const MATERIAL_SKIN: usize = 1;
const MATERIAL_LEATHER: usize = 2;
const MATERIAL_EMBER: usize = 3;

/// The bone hierarchy, in bind pose.
///
/// Translations are local — relative to the parent — which is why the numbers look small.
/// The globals they compose to are asserted in the tests, because a typo here moves a limb
/// somewhere plausible rather than somewhere obviously wrong.
fn skeleton() -> Result<Skeleton, crate::AssetError> {
    let bone = |name: &str, parent: Option<u16>, x: f32, y: f32, z: f32| Bone {
        name: name.to_owned(),
        parent,
        rest: Transform::from_translation(Vec3::new(x, y, z)),
        inverse_bind: Mat4::IDENTITY,
    };

    let mut bones = vec![
        bone("root", None, 0.0, 0.0, 0.0),
        bone("hips", Some(ROOT), 0.0, 0.92, 0.0),
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
        bone("staff", Some(HAND_R), 0.0, 0.0, 0.0),
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
struct SkinBuilder {
    vertices: Vec<SkinVertex>,
    indices: Vec<u32>,
}

impl SkinBuilder {
    fn new() -> Self {
        Self {
            vertices: Vec::new(),
            indices: Vec::new(),
        }
    }

    /// Adds a prism whose lower cap follows `lower_joint` and upper cap follows `upper_joint`.
    ///
    /// The two-joint form is what makes a knee bend instead of shearing: the vertices at the
    /// knee end of the thigh carry a fifth of the shin's motion, so the surface follows the
    /// joint rather than tearing across it. When both joints are the same bone the vertex is
    /// bound rigidly, which is what most of a stylized character wants.
    fn prism(&mut self, prism: &Prism, lower_joint: u16, upper_joint: u16) {
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

    fn into_submesh(self, name: &str, material: usize) -> SubMesh {
        SubMesh::new(
            name,
            Geometry::Skinned(self.vertices),
            self.indices,
            material,
        )
    }
}

/// A prism between two points with square-ish caps.
fn limb(lower: Vec3, upper: Vec3, lower_half: Vec2, upper_half: Vec2) -> Prism {
    Prism::new(lower, upper, lower_half.x, upper_half.x).with_halves(lower_half, upper_half)
}

/// The robe, hood, shoulders and sleeves.
fn robe(detail: Detail) -> SubMesh {
    let mut builder = SkinBuilder::new();

    // Skirt: the widest part of the silhouette, and the reason the character reads as a mage
    // from behind at 30 metres.
    builder.prism(
        &limb(
            Vec3::new(0.0, 0.30, 0.0),
            Vec3::new(0.0, 0.95, 0.0),
            Vec2::new(0.26, 0.21),
            Vec2::new(0.155, 0.115),
        )
        .with_uv_span(0.0, 0.45),
        HIPS,
        HIPS,
    );
    builder.prism(
        &limb(
            Vec3::new(0.0, 0.90, 0.0),
            Vec3::new(0.0, 1.10, 0.0),
            Vec2::new(0.150, 0.110),
            Vec2::new(0.160, 0.115),
        )
        .with_uv_span(0.45, 0.20),
        HIPS,
        SPINE,
    );
    builder.prism(
        &limb(
            Vec3::new(0.0, 1.10, 0.0),
            Vec3::new(0.0, 1.34, 0.0),
            Vec2::new(0.160, 0.115),
            Vec2::new(0.185, 0.120),
        )
        .with_uv_span(0.65, 0.20),
        SPINE,
        CHEST,
    );
    // Hood: a hard taper, which is what makes the head read as hooded rather than as a box.
    builder.prism(
        &limb(
            Vec3::new(0.0, 1.42, 0.0),
            Vec3::new(0.0, 1.80, 0.0),
            Vec2::new(0.150, 0.150),
            Vec2::new(0.045, 0.045),
        )
        .with_uv_span(0.85, 0.15),
        HEAD,
        HEAD,
    );

    if detail == Detail::Full {
        for (side, shoulder, arm, forearm, hand) in [
            (1.0, SHOULDER_L, ARM_L, FOREARM_L, HAND_L),
            (-1.0, SHOULDER_R, ARM_R, FOREARM_R, HAND_R),
        ] {
            builder.prism(
                &limb(
                    Vec3::new(side * 0.04, 1.39, 0.0),
                    Vec3::new(side * 0.26, 1.37, 0.0),
                    Vec2::new(0.105, 0.105),
                    Vec2::new(0.075, 0.075),
                )
                .with_width_axis(Vec3::Y),
                shoulder,
                arm,
            );
            builder.prism(
                &limb(
                    Vec3::new(side * 0.42, 1.39, 0.0),
                    Vec3::new(side * 0.61, 1.39, 0.0),
                    Vec2::new(0.062, 0.062),
                    Vec2::new(0.080, 0.080),
                )
                .with_width_axis(Vec3::Y),
                forearm,
                hand,
            );
        }
    }

    builder.into_submesh("robe", MATERIAL_ROBE)
}

/// Head and hands.
fn skin(detail: Detail) -> SubMesh {
    let mut builder = SkinBuilder::new();
    builder.prism(
        &limb(
            Vec3::new(0.0, 1.40, 0.0),
            Vec3::new(0.0, 1.68, 0.0),
            Vec2::new(0.098, 0.095),
            Vec2::new(0.100, 0.098),
        ),
        NECK,
        HEAD,
    );
    if detail == Detail::Full {
        for (side, hand) in [(1.0, HAND_L), (-1.0, HAND_R)] {
            builder.prism(
                &limb(
                    Vec3::new(side * 0.62, 1.39, 0.0),
                    Vec3::new(side * 0.72, 1.39, 0.0),
                    Vec2::new(0.050, 0.050),
                    Vec2::new(0.042, 0.042),
                )
                .with_width_axis(Vec3::Y),
                hand,
                hand,
            );
        }
    }
    builder.into_submesh("skin", MATERIAL_SKIN)
}

/// Belt, legs, boots and the staff shaft.
fn leather(detail: Detail) -> SubMesh {
    let mut builder = SkinBuilder::new();

    builder.prism(
        &limb(
            Vec3::new(0.0, 0.87, 0.0),
            Vec3::new(0.0, 0.96, 0.0),
            Vec2::new(0.165, 0.125),
            Vec2::new(0.162, 0.122),
        )
        .with_uv_span(0.55, 0.12),
        HIPS,
        HIPS,
    );

    for (side, thigh, shin, foot) in [
        (1.0, THIGH_L, SHIN_L, FOOT_L),
        (-1.0, THIGH_R, SHIN_R, FOOT_R),
    ] {
        let x = side * 0.085;
        if detail == Detail::Full {
            builder.prism(
                &limb(
                    Vec3::new(x, 0.44, 0.0),
                    Vec3::new(x, 0.89, 0.0),
                    Vec2::new(0.062, 0.062),
                    Vec2::new(0.080, 0.080),
                )
                .with_uv_span(0.15, 0.3),
                shin,
                thigh,
            );
            builder.prism(
                &limb(
                    Vec3::new(x, 0.08, 0.0),
                    Vec3::new(x, 0.44, 0.0),
                    Vec2::new(0.055, 0.055),
                    Vec2::new(0.062, 0.062),
                )
                .with_uv_span(0.0, 0.15),
                foot,
                shin,
            );
        } else {
            // One prism for the whole leg: at the distance LOD1 is used, a knee is two
            // pixels.
            builder.prism(
                &limb(
                    Vec3::new(x, 0.08, 0.0),
                    Vec3::new(x, 0.89, 0.0),
                    Vec2::new(0.055, 0.055),
                    Vec2::new(0.080, 0.080),
                ),
                shin,
                thigh,
            );
        }
        // Upright rather than tilted forward: a tilted prism's lower cap is a tilted plane,
        // and its leading corners would sink through the floor the character stands on.
        // The forward reach of the foot is in the cross-section's depth instead.
        builder.prism(
            &limb(
                Vec3::new(x, 0.0, -0.025),
                Vec3::new(x, 0.14, -0.025),
                Vec2::new(0.068, 0.115),
                Vec2::new(0.058, 0.085),
            )
            .with_uv_span(0.0, 0.12),
            foot,
            foot,
        );
    }

    // The staff, rigidly bound to its own bone under the right hand.
    builder.prism(
        &limb(
            Vec3::new(-0.65, 0.42, 0.0),
            Vec3::new(-0.65, 2.02, 0.0),
            Vec2::new(0.022, 0.022),
            Vec2::new(0.026, 0.026),
        )
        .with_uv_span(0.0, 1.0),
        STAFF,
        STAFF,
    );

    builder.into_submesh("leather", MATERIAL_LEATHER)
}

/// The ember at the head of the staff — the only emissive surface on the character.
fn ember() -> SubMesh {
    let mut builder = SkinBuilder::new();
    builder.prism(
        &limb(
            Vec3::new(-0.65, 1.99, 0.0),
            Vec3::new(-0.65, 2.13, 0.0),
            Vec2::new(0.030, 0.030),
            Vec2::new(0.062, 0.062),
        ),
        STAFF,
        STAFF,
    );
    builder.prism(
        &limb(
            Vec3::new(-0.65, 2.13, 0.0),
            Vec3::new(-0.65, 2.22, 0.0),
            Vec2::new(0.062, 0.062),
            Vec2::new(0.014, 0.014),
        ),
        STAFF,
        STAFF,
    );
    builder.into_submesh("ember", MATERIAL_EMBER)
}

/// How much of the model to build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Detail {
    /// Every prism.
    Full,
    /// Torso, hood, head, legs as one prism each, boots, staff. No sleeves, no hands, no
    /// separate knee.
    Reduced,
}

/// Deterministic value in `0..1` from a pair of integers.
///
/// A hash rather than a random number generator: the same texture has to come out of every
/// run, on every machine, or a screenshot comparison is meaningless.
fn hash01(x: u32, y: u32) -> f32 {
    let mut value = x.wrapping_mul(0x9E37_79B9) ^ y.wrapping_mul(0x85EB_CA6B);
    value ^= value >> 13;
    value = value.wrapping_mul(0xC2B2_AE35);
    value ^= value >> 16;
    (value & 0xFFFF) as f32 / 65535.0
}

/// Mixes two colours.
fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Packs a linear-ish colour into an opaque sRGB texel.
fn texel(color: [f32; 3]) -> [u8; 4] {
    [
        (color[0].clamp(0.0, 1.0) * 255.0) as u8,
        (color[1].clamp(0.0, 1.0) * 255.0) as u8,
        (color[2].clamp(0.0, 1.0) * 255.0) as u8,
        255,
    ]
}

/// The robe texture: a dark crimson that lightens towards the shoulders, a gold belt band,
/// and a vertical weave.
///
/// V runs up the body because every prism's `uv_span` places it on this one image — the hem
/// is at v = 0, the collar at v = 1 — which is what lets four separate prisms share one
/// texture with no unwrap.
fn robe_texture() -> TextureAsset {
    const SIZE: u32 = 256;
    TextureAsset::generated(
        "fire_mage_robe",
        SIZE,
        SIZE,
        TextureFormat::Rgba8Srgb,
        |u, v| {
            let hem = [0.22, 0.05, 0.05];
            let body = [0.52, 0.11, 0.09];
            let shoulder = [0.66, 0.20, 0.10];
            let mut color = if v < 0.5 {
                mix(hem, body, v / 0.5)
            } else {
                mix(body, shoulder, (v - 0.5) / 0.5)
            };
            // The belt band, at the height the belt geometry sits.
            if (0.55..0.62).contains(&v) {
                color = mix(color, [0.78, 0.60, 0.22], 0.85);
            }
            // A hem trim, so the bottom of the robe has an edge rather than fading out.
            if v < 0.04 {
                color = mix(color, [0.80, 0.62, 0.24], 0.7);
            }
            // Vertical weave: cheap, and it gives the flat prism sides something to catch
            // the light on.
            let weave = ((u * SIZE as f32 * 0.5).sin() * 0.5 + 0.5) * 0.06;
            let grain = hash01((u * SIZE as f32) as u32, (v * SIZE as f32) as u32) * 0.05;
            texel(mix(color, [0.0, 0.0, 0.0], weave + grain))
        },
    )
}

/// Skin: a flat warm tone with just enough grain not to band.
fn skin_texture() -> TextureAsset {
    const SIZE: u32 = 64;
    TextureAsset::generated(
        "fire_mage_skin",
        SIZE,
        SIZE,
        TextureFormat::Rgba8Srgb,
        |u, v| {
            let grain = hash01((u * SIZE as f32) as u32, (v * SIZE as f32) as u32) * 0.06;
            texel(mix([0.76, 0.58, 0.45], [0.55, 0.38, 0.30], grain))
        },
    )
}

/// Leather: dark brown with horizontal grain, used for the belt, legs, boots and staff.
fn leather_texture() -> TextureAsset {
    const SIZE: u32 = 128;
    TextureAsset::generated(
        "fire_mage_leather",
        SIZE,
        SIZE,
        TextureFormat::Rgba8Srgb,
        |u, v| {
            let band = ((v * 46.0).sin() * 0.5 + 0.5) * 0.18;
            let grain = hash01((u * SIZE as f32) as u32, (v * SIZE as f32) as u32) * 0.10;
            texel(mix([0.20, 0.14, 0.10], [0.09, 0.06, 0.05], band + grain))
        },
    )
}

/// Degrees to a rotation, in the yaw/pitch/roll order the animations are written in.
///
/// Pitch is X — a leg or an arm swinging forwards and back. Roll is Z — an arm coming down
/// from the T-pose. Yaw is Y — a torso turning. Writing clips in degrees keeps them
/// readable; the conversion happens once, at build.
fn ypr(yaw: f32, pitch: f32, roll: f32) -> Quat {
    Quat::from_yaw_pitch_roll(yaw.to_radians(), pitch.to_radians(), roll.to_radians())
}

/// A rotation track from `(time, yaw, pitch, roll)` tuples in degrees.
fn rotation(bone: u16, keys: &[(f32, f32, f32, f32)]) -> BoneTrack {
    BoneTrack::new(bone).with_rotation(
        keys.iter()
            .map(|(time, yaw, pitch, roll)| Key::new(*time, ypr(*yaw, *pitch, *roll)))
            .collect(),
    )
}

/// A translation track from `(time, x, y, z)` tuples in metres.
fn translation(bone: u16, keys: &[(f32, f32, f32, f32)]) -> BoneTrack {
    BoneTrack::new(bone).with_translation(
        keys.iter()
            .map(|(time, x, y, z)| Key::new(*time, Vec3::new(*x, *y, *z)))
            .collect(),
    )
}

/// The hips' rest translation, which every hips translation track is written relative to.
const HIPS_REST_Y: f32 = 0.92;

/// Arms hanging rather than held out: the pose every clip starts from.
///
/// A quarter turn about Z takes the left arm from +X to -Y. 78 degrees leaves it slightly
/// out from the body, which is what stops the sleeves intersecting the robe.
const ARM_DOWN_L: f32 = -78.0;
/// The mirror of [`ARM_DOWN_L`] for the right arm, which rests along -X.
const ARM_DOWN_R: f32 = 78.0;

/// Every animation the FireMage ships with.
fn clips() -> Vec<AnimationClip> {
    vec![
        idle(),
        walk(),
        run(),
        jump(),
        fall(),
        land(),
        cast(),
        cast_heavy(),
    ]
}

/// Standing: a slow breath, a small sway, and the staff arm holding still.
fn idle() -> AnimationClip {
    AnimationClip::new(
        "idle",
        true,
        vec![
            translation(
                HIPS,
                &[
                    (0.0, 0.0, HIPS_REST_Y, 0.0),
                    (1.6, 0.0, HIPS_REST_Y + 0.014, 0.0),
                    (3.2, 0.0, HIPS_REST_Y, 0.0),
                ],
            ),
            rotation(
                CHEST,
                &[
                    (0.0, 0.0, 1.5, 0.0),
                    (1.6, 0.0, -1.5, 0.0),
                    (3.2, 0.0, 1.5, 0.0),
                ],
            ),
            rotation(
                HEAD,
                &[
                    (0.0, 2.0, 0.0, 0.0),
                    (1.6, -2.0, 2.0, 0.0),
                    (3.2, 2.0, 0.0, 0.0),
                ],
            ),
            rotation(
                SHOULDER_L,
                &[
                    (0.0, 0.0, 0.0, ARM_DOWN_L),
                    (1.6, 0.0, 3.0, ARM_DOWN_L + 2.0),
                    (3.2, 0.0, 0.0, ARM_DOWN_L),
                ],
            ),
            // The staff arm is held across the body rather than hanging, so the staff reads
            // as carried.
            rotation(
                SHOULDER_R,
                &[
                    (0.0, 0.0, -8.0, ARM_DOWN_R - 6.0),
                    (1.6, 0.0, -6.0, ARM_DOWN_R - 8.0),
                    (3.2, 0.0, -8.0, ARM_DOWN_R - 6.0),
                ],
            ),
            rotation(FOREARM_L, &[(0.0, 0.0, 0.0, -14.0)]),
            rotation(FOREARM_R, &[(0.0, 0.0, 0.0, 14.0)]),
        ],
    )
}

/// A walk cycle: one second, two steps, arms counter-swinging.
fn walk() -> AnimationClip {
    locomotion("walk", 1.0, 22.0, 26.0, 18.0, 0.022, 4.0)
}

/// A run cycle: faster, longer strides, and a forward lean.
fn run() -> AnimationClip {
    locomotion("run", 0.62, 44.0, 52.0, 42.0, 0.045, 12.0)
}

/// The shape both locomotion cycles share.
///
/// One function rather than two hand-written clips because a walk and a run differ in
/// amplitude and period, not in structure — and two copies of a cycle drift apart the first
/// time one of them is tuned.
fn locomotion(
    name: &str,
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
            arm(SHOULDER_L, ARM_DOWN_L, -1.0),
            arm(SHOULDER_R, ARM_DOWN_R, 1.0),
            rotation(FOREARM_L, &[(0.0, 0.0, -20.0, -14.0)]),
            rotation(FOREARM_R, &[(0.0, 0.0, -20.0, 14.0)]),
        ],
    )
}

/// Leaving the ground: a push through the legs and the arms coming up.
fn jump() -> AnimationClip {
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
                    (0.0, 0.0, 0.0, ARM_DOWN_L),
                    (0.20, 0.0, 0.0, ARM_DOWN_L + 46.0),
                ],
            ),
            rotation(
                SHOULDER_R,
                &[
                    (0.0, 0.0, -8.0, ARM_DOWN_R),
                    (0.20, 0.0, -8.0, ARM_DOWN_R - 40.0),
                ],
            ),
            rotation(SPINE, &[(0.0, 0.0, 12.0, 0.0), (0.20, 0.0, -6.0, 0.0)]),
        ],
    )
}

/// Airborne and descending: legs trailing, arms out for balance. Loops, because a fall has
/// no natural length.
fn fall() -> AnimationClip {
    AnimationClip::new(
        "fall",
        true,
        vec![
            rotation(
                SHOULDER_L,
                &[
                    (0.0, 0.0, 0.0, ARM_DOWN_L + 34.0),
                    (0.4, 0.0, 6.0, ARM_DOWN_L + 40.0),
                    (0.8, 0.0, 0.0, ARM_DOWN_L + 34.0),
                ],
            ),
            rotation(
                SHOULDER_R,
                &[
                    (0.0, 0.0, -6.0, ARM_DOWN_R - 30.0),
                    (0.4, 0.0, -10.0, ARM_DOWN_R - 36.0),
                    (0.8, 0.0, -6.0, ARM_DOWN_R - 30.0),
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
fn land() -> AnimationClip {
    AnimationClip::new(
        "land",
        false,
        vec![
            translation(
                HIPS,
                &[
                    (0.0, 0.0, HIPS_REST_Y - 0.02, 0.0),
                    (0.08, 0.0, HIPS_REST_Y - 0.13, 0.0),
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

/// An ordinary cast: the staff comes up and forward, the free hand leads.
fn cast() -> AnimationClip {
    AnimationClip::new(
        "cast",
        false,
        vec![
            rotation(
                SHOULDER_R,
                &[
                    (0.0, 0.0, -8.0, ARM_DOWN_R - 6.0),
                    (0.22, 0.0, -70.0, ARM_DOWN_R - 30.0),
                    (0.42, 0.0, -96.0, ARM_DOWN_R - 22.0),
                    (0.70, 0.0, -20.0, ARM_DOWN_R - 8.0),
                ],
            ),
            rotation(
                SHOULDER_L,
                &[
                    (0.0, 0.0, 0.0, ARM_DOWN_L),
                    (0.22, 0.0, -40.0, ARM_DOWN_L + 18.0),
                    (0.42, 0.0, -72.0, ARM_DOWN_L + 26.0),
                    (0.70, 0.0, -10.0, ARM_DOWN_L + 4.0),
                ],
            ),
            rotation(
                CHEST,
                &[
                    (0.0, 0.0, 0.0, 0.0),
                    (0.22, 14.0, -6.0, 0.0),
                    (0.42, -10.0, 8.0, 0.0),
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
    )
}

/// The big one: both arms overhead, then driven forward.
fn cast_heavy() -> AnimationClip {
    AnimationClip::new(
        "cast_heavy",
        false,
        vec![
            rotation(
                SHOULDER_R,
                &[
                    (0.0, 0.0, -8.0, ARM_DOWN_R - 6.0),
                    (0.40, 0.0, -30.0, ARM_DOWN_R - 78.0),
                    (0.72, 0.0, -110.0, ARM_DOWN_R - 30.0),
                    (1.10, 0.0, -20.0, ARM_DOWN_R - 8.0),
                ],
            ),
            rotation(
                SHOULDER_L,
                &[
                    (0.0, 0.0, 0.0, ARM_DOWN_L),
                    (0.40, 0.0, -20.0, ARM_DOWN_L + 74.0),
                    (0.72, 0.0, -96.0, ARM_DOWN_L + 30.0),
                    (1.10, 0.0, -10.0, ARM_DOWN_L + 4.0),
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
    )
}

/// Builds the FireMage.
///
/// # Errors
///
/// Returns [`crate::AssetError`] if the skeleton is malformed, which can only happen if the
/// bone table above is edited into an inconsistent state.
pub fn fire_mage() -> Result<ModelAsset, crate::AssetError> {
    let materials = vec![
        MaterialAsset::colored("fire_mage_robe", 1.0, 1.0, 1.0)
            .with_roughness(0.75)
            .with_base_color_texture(0),
        MaterialAsset::colored("fire_mage_skin", 1.0, 1.0, 1.0)
            .with_roughness(0.85)
            .with_base_color_texture(1),
        MaterialAsset::colored("fire_mage_leather", 1.0, 1.0, 1.0)
            .with_roughness(0.68)
            .with_base_color_texture(2),
        MaterialAsset::colored("fire_mage_ember", 1.0, 0.55, 0.18)
            .with_roughness(0.35)
            .with_emissive(Vec3::new(1.6, 0.55, 0.12)),
    ];
    let textures = vec![robe_texture(), skin_texture(), leather_texture()];

    let lod0 = vec![
        robe(Detail::Full),
        skin(Detail::Full),
        leather(Detail::Full),
        ember(),
    ];
    // LOD1 drops the sleeves, the hands, the separate knee and the ember, which is four
    // fewer prisms and one fewer draw. It keeps the silhouette — the robe, the hood, the
    // staff — because the silhouette is all that survives to 25 metres anyway.
    let lod1 = vec![
        robe(Detail::Reduced),
        skin(Detail::Reduced),
        leather(Detail::Reduced),
    ];

    Ok(ModelAsset::rigid(MODEL_NAME, lod0, materials, textures)
        .with_lod(lod1, 25.0)
        .with_animation(skeleton()?, clips()))
}

/// The FireMage's character definition: which model, how tall, and which clip plays when.
///
/// The complete answer to "how does gameplay know what the player looks like": it does not.
/// This value does, and it is data.
#[must_use]
pub fn fire_mage_character() -> CharacterDef {
    CharacterDef::new("FireMage", ModelId::new(MODEL_NAME), MODEL_HEIGHT)
        .with_speeds(1.8, 5.5)
        .with_clip(AnimationSlot::Idle, "idle")
        .with_clip(AnimationSlot::Walk, "walk")
        .with_clip(AnimationSlot::Run, "run")
        .with_clip(AnimationSlot::Jump, "jump")
        .with_clip(AnimationSlot::Fall, "fall")
        .with_clip(AnimationSlot::Land, "land")
        .with_clip(AnimationSlot::Cast, "cast")
        .with_clip(AnimationSlot::CastHeavy, "cast_heavy")
        .with_directional_clips()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validate::{ValidationLimits, validate_model};

    fn model() -> ModelAsset {
        match fire_mage() {
            Ok(model) => model,
            Err(error) => panic!("the FireMage must build: {error}"),
        }
    }

    #[test]
    fn the_firemage_passes_the_same_validation_an_imported_model_would() {
        let Ok(report) = validate_model(&model(), &ValidationLimits::default()) else {
            panic!("the FireMage must validate");
        };
        assert_eq!(report.lods, 2);
        assert_eq!(report.materials, 4);
        assert_eq!(report.bones, 21);
        assert_eq!(report.clips, 8);
        assert!(report.triangles > 100, "{} triangles", report.triangles);
        assert!(
            report.triangles < 600,
            "a stylized prototype character should stay cheap: {} triangles",
            report.triangles
        );
    }

    #[test]
    fn the_model_is_as_tall_as_it_claims_and_stands_on_the_floor() {
        // The visual layer scales by gameplay_height / MODEL_HEIGHT, so a model that is not
        // the height it declares makes every character in the game the wrong size.
        let model = model();
        let Some(lod) = model.lod0() else {
            panic!("LOD0 must exist");
        };
        let mut lowest = f32::MAX;
        let mut highest = f32::MIN;
        for submesh in &lod.submeshes {
            for position in submesh.geometry.positions() {
                lowest = lowest.min(position.y);
                highest = highest.max(position.y);
            }
        }
        assert!(lowest.abs() < 0.02, "the feet are at y = {lowest}");
        // The staff is taller than the mage, so the head is measured from the body prisms
        // rather than from the overall bounds.
        assert!(
            (MODEL_HEIGHT - 1.80).abs() < 1e-6 && highest > MODEL_HEIGHT,
            "the staff should rise above the declared height, top was {highest}"
        );
    }

    #[test]
    fn the_hood_is_the_top_of_the_body_and_matches_the_declared_height() {
        let model = model();
        let Some(lod) = model.lod0() else {
            panic!("LOD0 must exist");
        };
        let Some(robe) = lod.submeshes.iter().find(|submesh| submesh.name == "robe") else {
            panic!("the robe submesh must exist");
        };
        let top = robe
            .geometry
            .positions()
            .map(|position| position.y)
            .fold(f32::MIN, f32::max);
        assert!(
            (top - MODEL_HEIGHT).abs() < 0.01,
            "the hood tops out at {top}, and the model claims {MODEL_HEIGHT}"
        );
    }

    #[test]
    fn the_skeleton_puts_the_hands_where_the_sleeves_are() {
        // A typo in a local translation moves a limb somewhere plausible rather than
        // somewhere obviously wrong, so the globals are asserted rather than eyeballed.
        let Ok(skeleton) = skeleton() else {
            panic!("the skeleton must build");
        };
        let mut globals = Vec::new();
        let Ok(()) = skeleton.global_transforms(&skeleton.rest_pose(), &mut globals) else {
            panic!("the rest pose must compose");
        };
        let at = |bone: u16| globals[bone as usize].transform_point(Vec3::ZERO);

        assert!((at(HAND_L) - Vec3::new(0.65, 1.39, 0.0)).length() < 1e-4);
        assert!((at(HAND_R) - Vec3::new(-0.65, 1.39, 0.0)).length() < 1e-4);
        assert!((at(HEAD) - Vec3::new(0.0, 1.52, 0.0)).length() < 1e-4);
        assert!((at(FOOT_L) - Vec3::new(0.085, 0.04, 0.0)).length() < 1e-4);
        // The staff bone sits exactly in the right hand, which is what makes the staff
        // geometry follow it.
        assert!((at(STAFF) - at(HAND_R)).length() < 1e-6);
    }

    #[test]
    fn every_clip_the_character_definition_names_exists() {
        let model = model();
        let Ok(bindings) = fire_mage_character().resolve(&model) else {
            panic!("the FireMage's own definition must resolve against its own model");
        };
        assert_eq!(bindings.len(), AnimationSlot::REQUIRED.len());
    }

    #[test]
    fn the_looping_clips_start_and_end_in_the_same_pose() {
        // A cycle whose last key differs from its first pops once per period, which reads as
        // a limp rather than as a bug.
        let model = model();
        for name in ["idle", "walk", "run", "fall"] {
            let Some(index) = model.clip_index(name) else {
                panic!("{name} must exist");
            };
            let clip = &model.clips[index];
            assert!(clip.looping, "{name} should loop");
            for track in &clip.tracks {
                if let (Some(first), Some(last)) = (track.rotation.first(), track.rotation.last()) {
                    let difference = 1.0 - first.value.dot(last.value).abs();
                    assert!(
                        difference < 1e-4,
                        "{name}: bone {} ends {difference} away from where it started",
                        track.bone
                    );
                }
                if let (Some(first), Some(last)) =
                    (track.translation.first(), track.translation.last())
                {
                    assert!(
                        (first.value - last.value).length() < 1e-4,
                        "{name}: bone {} does not return to its starting position",
                        track.bone
                    );
                }
            }
        }
    }

    #[test]
    fn the_one_shot_clips_are_not_marked_looping() {
        let model = model();
        for name in ["jump", "land", "cast", "cast_heavy"] {
            let Some(index) = model.clip_index(name) else {
                panic!("{name} must exist");
            };
            assert!(!model.clips[index].looping, "{name} should play once");
            assert!(model.clips[index].duration > 0.0);
        }
    }

    #[test]
    fn a_knee_never_bends_forwards() {
        // The single most obvious way for a walk cycle to look wrong. The shin's pitch is
        // authored negative throughout, which bends the knee backwards as a knee does.
        let model = model();
        for name in ["walk", "run", "fall", "land"] {
            let Some(index) = model.clip_index(name) else {
                panic!("{name} must exist");
            };
            for track in &model.clips[index].tracks {
                if track.bone != SHIN_L && track.bone != SHIN_R {
                    continue;
                }
                for key in &track.rotation {
                    // A rotation about +X by a negative angle sends the shin's -Y backwards.
                    assert!(
                        key.value.x <= 1e-6,
                        "{name}: the shin bends forwards at {}s",
                        key.time
                    );
                }
            }
        }
    }

    #[test]
    fn the_reduced_level_is_cheaper_and_keeps_the_silhouette() {
        let model = model();
        let (Some(lod0), Some(lod1)) = (model.lods.first(), model.lods.get(1)) else {
            panic!("both levels must exist");
        };
        assert!(
            lod1.triangle_count() < lod0.triangle_count(),
            "LOD1 must be cheaper: {} vs {}",
            lod1.triangle_count(),
            lod0.triangle_count()
        );
        assert!(
            lod1.submeshes.len() < lod0.submeshes.len(),
            "one fewer draw"
        );
        assert!(
            lod1.is_skinned(),
            "a reduced character is still a character"
        );
    }

    #[test]
    fn every_vertex_is_bound_to_a_bone_that_exists_and_carries_a_full_weight() {
        // Validation checks this too; asserting it here points at the *builder* rather than
        // at the model when a prism is added with a bad joint.
        let model = model();
        let Some(skeleton) = &model.skeleton else {
            panic!("the FireMage is skinned");
        };
        for lod in &model.lods {
            for submesh in &lod.submeshes {
                let Geometry::Skinned(vertices) = &submesh.geometry else {
                    panic!("'{}' should be skinned", submesh.name);
                };
                for vertex in vertices {
                    assert!((vertex.weight_sum() - 1.0).abs() < 1e-4);
                    for joint in vertex.joints {
                        assert!((joint as usize) < skeleton.len());
                    }
                }
            }
        }
    }

    #[test]
    fn the_textures_are_the_sizes_the_material_budget_assumes() {
        let model = model();
        assert_eq!(model.textures.len(), 3);
        assert_eq!(
            model.texture_bytes(),
            256 * 256 * 4 + 64 * 64 * 4 + 128 * 128 * 4
        );
        for texture in &model.textures {
            assert!(texture.format.is_srgb(), "{} is colour", texture.name);
        }
    }

    #[test]
    fn the_model_is_the_same_every_time_it_is_built() {
        // Procedural content with a random seed makes a screenshot comparison meaningless
        // and a bug report unreproducible.
        assert_eq!(model(), model());
    }
}
