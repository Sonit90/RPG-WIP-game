//! The Warrior: a plated figure with a sword, on the shared humanoid rig.
//!
//! ```text
//!              helm + head                   bind pose is a T-pose:
//!                   │                        arms along ±X, legs along -Y,
//!      pauldron ─ cuirass ─ pauldron         facing -Z, feet at y = 0.
//!          │                    │
//!         arm                  arm           1.80 m tall, 24 prisms,
//!          │                    │            4 materials, LOD1 at 25 m.
//!       gauntlet             gauntlet
//!            faulds / belt
//!          ┌──────┴──────┐
//!        thigh         thigh ── sword (in the right hand)
//!          │             │
//!        greave        greave
//!          │             │
//!       sabaton        sabaton
//! ```
//!
//! # What makes it read as a Warrior at thirty metres
//!
//! Silhouette, in this order: **shoulders wider than the hips**, a **flat-topped helm** where
//! the mage has a tapering hood, and a **blade held out from the body**. Those three are the
//! only things that survive to LOD1, which is why LOD1 keeps all three and drops everything
//! else. The plate texture and the trim are for the two metres the player spends looking at
//! their own back.
//!
//! Everything here is prisms, for the reason [`super`] gives: this is a technical baseline
//! that lets skinning, animation, materials and LOD be built and measured against a real
//! character, and it is original work with no licence attached to it.

use engine_core::math::{Vec2, Vec3};

use crate::character::{AnimationSlot, CharacterDef};
use crate::library::ModelId;
use crate::material::MaterialAsset;
use crate::mesh::SubMesh;
use crate::model::ModelAsset;
use crate::procedural::humanoid::{
    self, CHEST, FOOT_L, FOOT_R, HAND_L, HAND_R, HEAD, HIPS, MODEL_HEIGHT, NECK, PROP, SHIN_L,
    SHIN_R, SHOULDER_L, SHOULDER_R, SPINE, SkinBuilder, Style, THIGH_L, THIGH_R, limb,
};
use crate::texture::{TextureAsset, TextureFormat};

/// The identifier this model registers under.
pub const MODEL_NAME: &str = "warrior";

/// Material slots, in the order [`warrior`] builds them.
const MATERIAL_PLATE: usize = 0;
const MATERIAL_SKIN: usize = 1;
const MATERIAL_LEATHER: usize = 2;
const MATERIAL_STEEL: usize = 3;

/// How much of the model to build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Detail {
    /// Every prism.
    Full,
    /// The silhouette: cuirass, pauldrons, helm, one prism per leg, the sword. No gauntlets,
    /// no separate knee, no belt trim.
    Reduced,
}

/// Cuirass, faulds, pauldrons and the plate on the limbs.
fn plate(detail: Detail) -> SubMesh {
    let mut builder = SkinBuilder::new();

    // Faulds: the skirt of plate over the hips. Narrower than the mage's robe, because a
    // Warrior's widest measurement has to be the shoulders.
    builder.prism(
        &limb(
            Vec3::new(0.0, 0.68, 0.0),
            Vec3::new(0.0, 0.95, 0.0),
            Vec2::new(0.185, 0.150),
            Vec2::new(0.165, 0.130),
        )
        .with_uv_span(0.0, 0.25),
        HIPS,
        HIPS,
    );
    // Cuirass, in two prisms so the chest can flare where the ribs do.
    builder.prism(
        &limb(
            Vec3::new(0.0, 0.94, 0.0),
            Vec3::new(0.0, 1.16, 0.0),
            Vec2::new(0.170, 0.130),
            Vec2::new(0.200, 0.140),
        )
        .with_uv_span(0.25, 0.25),
        HIPS,
        SPINE,
    );
    builder.prism(
        &limb(
            Vec3::new(0.0, 1.16, 0.0),
            Vec3::new(0.0, 1.42, 0.0),
            Vec2::new(0.200, 0.140),
            Vec2::new(0.225, 0.150),
        )
        .with_uv_span(0.50, 0.30),
        SPINE,
        CHEST,
    );
    // Helm: flat-topped and square, the counterpart of the mage's taper. The head reads as
    // armoured from behind at any distance because the top of the silhouette is a box.
    builder.prism(
        &limb(
            Vec3::new(0.0, 1.56, 0.0),
            Vec3::new(0.0, 1.80, 0.0),
            Vec2::new(0.118, 0.118),
            Vec2::new(0.110, 0.112),
        )
        .with_uv_span(0.80, 0.20),
        HEAD,
        HEAD,
    );

    // Pauldrons: the widest thing on the character, and the reason the class reads as melee.
    // Kept at LOD1 for exactly that reason.
    for (side, shoulder) in [(1.0, SHOULDER_L), (-1.0, SHOULDER_R)] {
        builder.prism(
            &limb(
                Vec3::new(side * 0.20, 1.30, 0.0),
                Vec3::new(side * 0.34, 1.42, 0.0),
                Vec2::new(0.135, 0.135),
                Vec2::new(0.105, 0.105),
            )
            .with_width_axis(Vec3::Y)
            .with_uv_span(0.55, 0.25),
            shoulder,
            shoulder,
        );
    }

    if detail == Detail::Full {
        for (side, shoulder, forearm, hand) in [
            (1.0, SHOULDER_L, humanoid::FOREARM_L, HAND_L),
            (-1.0, SHOULDER_R, humanoid::FOREARM_R, HAND_R),
        ] {
            // Upper arm.
            builder.prism(
                &limb(
                    Vec3::new(side * 0.30, 1.39, 0.0),
                    Vec3::new(side * 0.50, 1.39, 0.0),
                    Vec2::new(0.078, 0.078),
                    Vec2::new(0.066, 0.066),
                )
                .with_width_axis(Vec3::Y)
                .with_uv_span(0.30, 0.20),
                shoulder,
                forearm,
            );
            // Gauntlet.
            builder.prism(
                &limb(
                    Vec3::new(side * 0.50, 1.39, 0.0),
                    Vec3::new(side * 0.66, 1.39, 0.0),
                    Vec2::new(0.066, 0.066),
                    Vec2::new(0.074, 0.074),
                )
                .with_width_axis(Vec3::Y)
                .with_uv_span(0.30, 0.20),
                forearm,
                hand,
            );
        }
    }

    builder.into_submesh("plate", MATERIAL_PLATE)
}

/// The head and the neck: the only bare skin on the model.
fn skin() -> SubMesh {
    let mut builder = SkinBuilder::new();
    builder.prism(
        &limb(
            Vec3::new(0.0, 1.42, 0.0),
            Vec3::new(0.0, 1.60, 0.0),
            Vec2::new(0.096, 0.094),
            Vec2::new(0.106, 0.104),
        ),
        NECK,
        HEAD,
    );
    builder.into_submesh("skin", MATERIAL_SKIN)
}

/// Belt, legs, boots — everything that is strapped on rather than forged.
fn leather(detail: Detail) -> SubMesh {
    let mut builder = SkinBuilder::new();

    if detail == Detail::Full {
        builder.prism(
            &limb(
                Vec3::new(0.0, 0.90, 0.0),
                Vec3::new(0.0, 0.99, 0.0),
                Vec2::new(0.178, 0.138),
                Vec2::new(0.175, 0.135),
            )
            .with_uv_span(0.55, 0.14),
            HIPS,
            HIPS,
        );
    }

    for (side, thigh, shin, foot) in [
        (1.0, THIGH_L, SHIN_L, FOOT_L),
        (-1.0, THIGH_R, SHIN_R, FOOT_R),
    ] {
        let x = side * 0.095;
        if detail == Detail::Full {
            builder.prism(
                &limb(
                    Vec3::new(x, 0.44, 0.0),
                    Vec3::new(x, 0.90, 0.0),
                    Vec2::new(0.075, 0.075),
                    Vec2::new(0.095, 0.095),
                )
                .with_uv_span(0.15, 0.3),
                shin,
                thigh,
            );
            builder.prism(
                &limb(
                    Vec3::new(x, 0.10, 0.0),
                    Vec3::new(x, 0.44, 0.0),
                    Vec2::new(0.068, 0.068),
                    Vec2::new(0.075, 0.075),
                )
                .with_uv_span(0.0, 0.15),
                foot,
                shin,
            );
        } else {
            builder.prism(
                &limb(
                    Vec3::new(x, 0.10, 0.0),
                    Vec3::new(x, 0.90, 0.0),
                    Vec2::new(0.068, 0.068),
                    Vec2::new(0.095, 0.095),
                ),
                shin,
                thigh,
            );
        }
        // Upright rather than tilted forward, for the reason the mage's boot gives: a tilted
        // prism's lower cap is a tilted plane, and its leading corners sink through the floor
        // the character stands on. The forward reach is in the cross-section's depth.
        builder.prism(
            &limb(
                Vec3::new(x, 0.0, -0.030),
                Vec3::new(x, 0.16, -0.030),
                Vec2::new(0.082, 0.130),
                Vec2::new(0.070, 0.098),
            )
            .with_uv_span(0.0, 0.12),
            foot,
            foot,
        );
    }

    builder.into_submesh("leather", MATERIAL_LEATHER)
}

/// The sword: grip, crossguard and a blade that tapers to a point.
///
/// Rigidly bound to [`PROP`], which hangs off the right hand — so every clip that moves the
/// arm moves the sword, and no clip has to know a sword exists.
fn sword() -> SubMesh {
    let mut builder = SkinBuilder::new();
    // Grip and pommel, running down from the hand.
    builder.prism(
        &limb(
            Vec3::new(-0.66, 1.24, 0.0),
            Vec3::new(-0.66, 1.42, 0.0),
            Vec2::new(0.026, 0.026),
            Vec2::new(0.022, 0.022),
        )
        .with_uv_span(0.0, 0.15),
        PROP,
        PROP,
    );
    // Crossguard, across the blade rather than along it.
    builder.prism(
        &limb(
            Vec3::new(-0.66, 1.42, 0.0),
            Vec3::new(-0.66, 1.47, 0.0),
            Vec2::new(0.105, 0.028),
            Vec2::new(0.095, 0.024),
        )
        .with_uv_span(0.15, 0.10),
        PROP,
        PROP,
    );
    // Blade: wide at the guard, tapering over most of its length.
    builder.prism(
        &limb(
            Vec3::new(-0.66, 1.47, 0.0),
            Vec3::new(-0.66, 2.02, 0.0),
            Vec2::new(0.048, 0.016),
            Vec2::new(0.038, 0.011),
        )
        .with_uv_span(0.25, 0.55),
        PROP,
        PROP,
    );
    // The point.
    builder.prism(
        &limb(
            Vec3::new(-0.66, 2.02, 0.0),
            Vec3::new(-0.66, 2.14, 0.0),
            Vec2::new(0.038, 0.011),
            Vec2::new(0.004, 0.004),
        )
        .with_uv_span(0.80, 0.20),
        PROP,
        PROP,
    );
    builder.into_submesh("sword", MATERIAL_STEEL)
}

/// Plate: a cold steel that lightens towards the shoulders, with a gold trim band and rivets.
///
/// V runs up the body because every prism's `uv_span` places it on this one image — the
/// faulds at v = 0, the helm at v = 1 — which is what lets eight separate prisms share one
/// texture with no unwrap.
fn plate_texture() -> TextureAsset {
    const SIZE: u32 = 256;
    TextureAsset::generated(
        "warrior_plate",
        SIZE,
        SIZE,
        TextureFormat::Rgba8Srgb,
        |u, v| {
            let low = [0.26, 0.28, 0.32];
            let mid = [0.40, 0.43, 0.48];
            let high = [0.55, 0.58, 0.63];
            let mut color = if v < 0.5 {
                humanoid::mix(low, mid, v / 0.5)
            } else {
                humanoid::mix(mid, high, (v - 0.5) / 0.5)
            };
            // The trim band where the cuirass meets the faulds.
            if (0.24..0.29).contains(&v) {
                color = humanoid::mix(color, [0.74, 0.58, 0.24], 0.8);
            }
            // Rivets: a coarse grid of darker dots. Cheap, and it stops a flat prism side
            // from reading as a single lit plane.
            let (rivet_u, rivet_v) = ((u * 8.0).fract(), (v * 12.0).fract());
            let rivet = ((rivet_u - 0.5).powi(2) + (rivet_v - 0.5).powi(2)).sqrt();
            if rivet < 0.14 {
                color = humanoid::mix(color, [0.16, 0.17, 0.20], 0.7);
            }
            let grain = humanoid::hash01((u * SIZE as f32) as u32, (v * SIZE as f32) as u32) * 0.05;
            humanoid::texel(humanoid::mix(color, [0.0, 0.0, 0.0], grain))
        },
    )
}

/// Skin: a weathered warm tone, a shade ruddier than the mage's.
fn skin_texture() -> TextureAsset {
    const SIZE: u32 = 64;
    TextureAsset::generated(
        "warrior_skin",
        SIZE,
        SIZE,
        TextureFormat::Rgba8Srgb,
        |u, v| {
            let grain = humanoid::hash01((u * SIZE as f32) as u32, (v * SIZE as f32) as u32) * 0.06;
            humanoid::texel(humanoid::mix([0.74, 0.53, 0.41], [0.52, 0.34, 0.26], grain))
        },
    )
}

/// Leather: dark brown with horizontal grain, for the belt, the legs and the boots.
fn leather_texture() -> TextureAsset {
    const SIZE: u32 = 128;
    TextureAsset::generated(
        "warrior_leather",
        SIZE,
        SIZE,
        TextureFormat::Rgba8Srgb,
        |u, v| {
            let band = ((v * 46.0).sin() * 0.5 + 0.5) * 0.18;
            let grain = humanoid::hash01((u * SIZE as f32) as u32, (v * SIZE as f32) as u32) * 0.10;
            humanoid::texel(humanoid::mix(
                [0.24, 0.16, 0.11],
                [0.10, 0.07, 0.05],
                band + grain,
            ))
        },
    )
}

/// The animation style: a heavier walk than the mage's, and a swing rather than a channel.
fn style() -> Style {
    Style {
        // Wider than the mage's, because the pauldrons are: an arm hanging at 78 degrees
        // would put the elbow inside one.
        arm_down_l: -72.0,
        arm_down_r: 72.0,
        walk_period: 1.05,
        run_period: 0.60,
        stride: 24.0,
        knee: 28.0,
        arm_swing: 16.0,
        bob: 0.028,
        lean: 5.0,
        breath: 1.8,
        cast: humanoid::CastStyle::Swing,
    }
}

/// Builds the Warrior.
///
/// # Errors
///
/// Returns [`crate::AssetError`] if the shared rig is malformed, which can only happen if
/// [`humanoid::skeleton`] is edited into an inconsistent state.
pub fn warrior() -> Result<ModelAsset, crate::AssetError> {
    let materials = vec![
        MaterialAsset::colored("warrior_plate", 1.0, 1.0, 1.0)
            .with_roughness(0.42)
            .with_metallic(0.65)
            .with_base_color_texture(0),
        MaterialAsset::colored("warrior_skin", 1.0, 1.0, 1.0)
            .with_roughness(0.85)
            .with_base_color_texture(1),
        MaterialAsset::colored("warrior_leather", 1.0, 1.0, 1.0)
            .with_roughness(0.70)
            .with_base_color_texture(2),
        // The blade is the one surface with no texture: a clean specular edge is what makes
        // a sword read as sharpened, and a grain on it reads as rust.
        MaterialAsset::colored("warrior_steel", 0.78, 0.81, 0.86)
            .with_roughness(0.22)
            .with_metallic(0.90),
    ];
    let textures = vec![plate_texture(), skin_texture(), leather_texture()];

    let lod0 = vec![plate(Detail::Full), skin(), leather(Detail::Full), sword()];
    // LOD1 keeps the three things the silhouette is made of — the pauldrons, the helm and the
    // blade — and drops the arm plate, the gauntlets, the belt and the separate knee.
    let lod1 = vec![
        plate(Detail::Reduced),
        skin(),
        leather(Detail::Reduced),
        sword(),
    ];

    Ok(ModelAsset::rigid(MODEL_NAME, lod0, materials, textures)
        .with_lod(lod1, 25.0)
        .with_animation(humanoid::skeleton()?, humanoid::clips(&style())))
}

/// The Warrior's character definition: which model, how tall, and which clip plays when.
#[must_use]
pub fn warrior_character() -> CharacterDef {
    CharacterDef::new("Warrior", ModelId::new(MODEL_NAME), MODEL_HEIGHT)
        // A shade slower than the mage at both ends: the class closes distance with Charge,
        // not by outrunning anything.
        .with_speeds(1.7, 5.2)
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
        match warrior() {
            Ok(model) => model,
            Err(error) => panic!("the Warrior must build: {error}"),
        }
    }

    #[test]
    fn the_warrior_passes_the_same_validation_an_imported_model_would() {
        let Ok(report) = validate_model(&model(), &ValidationLimits::default()) else {
            panic!("the Warrior must validate");
        };
        assert_eq!(report.lods, 2);
        assert_eq!(report.materials, 4);
        assert_eq!(report.bones as usize, humanoid::BONE_COUNT);
        assert_eq!(report.clips, 8);
        assert!(
            (100..700).contains(&report.triangles),
            "a stylized prototype character should stay cheap: {} triangles",
            report.triangles
        );
    }

    #[test]
    fn the_model_is_as_tall_as_it_claims_and_stands_on_the_floor() {
        let model = model();
        let Some(lod) = model.lod0() else {
            panic!("LOD0 must exist");
        };
        let mut lowest = f32::MAX;
        for submesh in &lod.submeshes {
            for position in submesh.geometry.positions() {
                lowest = lowest.min(position.y);
            }
        }
        assert!(lowest.abs() < 0.02, "the feet are at y = {lowest}");

        // The helm is the top of the *body*; the raised sword is taller and is not the
        // character's height, exactly as the mage's staff is not.
        let Some(plate) = lod.submeshes.iter().find(|submesh| submesh.name == "plate") else {
            panic!("the plate submesh must exist");
        };
        let top = plate
            .geometry
            .positions()
            .map(|position| position.y)
            .fold(f32::MIN, f32::max);
        assert!(
            (top - MODEL_HEIGHT).abs() < 1e-3,
            "the helm tops out at {top}, not at the declared {MODEL_HEIGHT}"
        );
    }

    #[test]
    fn the_shoulders_are_wider_than_the_hips() {
        // The silhouette rule the whole model is arranged around, asserted rather than
        // trusted: a tuning pass that narrowed the pauldrons would take the class's
        // readability with it.
        let model = model();
        let Some(lod) = model.lod0() else {
            panic!("LOD0 must exist");
        };
        let Some(plate) = lod.submeshes.iter().find(|submesh| submesh.name == "plate") else {
            panic!("the plate submesh must exist");
        };
        let mut shoulder_width: f32 = 0.0;
        let mut hip_width: f32 = 0.0;
        for position in plate.geometry.positions() {
            if position.y > 1.25 && position.y < 1.50 {
                shoulder_width = shoulder_width.max(position.x.abs());
            }
            if position.y < 0.95 {
                hip_width = hip_width.max(position.x.abs());
            }
        }
        assert!(
            shoulder_width > hip_width * 1.5,
            "shoulders {shoulder_width} against hips {hip_width}"
        );
    }

    #[test]
    fn the_sword_survives_to_the_far_lod_and_hangs_off_the_hand() {
        let model = model();
        for level in 0..2 {
            let Some(lod) = model.lods.get(level) else {
                panic!("LOD{level} must exist");
            };
            assert!(
                lod.submeshes.iter().any(|submesh| submesh.name == "sword"),
                "LOD{level} lost the sword"
            );
        }
        // Every vertex of the blade is bound to the prop bone and to nothing else, which is
        // what makes it follow the hand without a track of its own.
        let Some(lod) = model.lod0() else {
            panic!("LOD0 must exist");
        };
        let Some(sword) = lod.submeshes.iter().find(|submesh| submesh.name == "sword") else {
            panic!("the sword submesh must exist");
        };
        let crate::mesh::Geometry::Skinned(vertices) = &sword.geometry else {
            panic!("the sword must be skinned");
        };
        for vertex in vertices {
            assert_eq!(vertex.joints[0], PROP);
            assert!((vertex.weights[0] - 1.0).abs() < 1e-3);
        }
    }
}
