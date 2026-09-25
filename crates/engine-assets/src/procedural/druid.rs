//! The Druid in human form: antlers, a leaf-green mantle and a living staff.
//!
//! ```text
//!        antler ─ head ─ antler          bind pose is a T-pose:
//!                  │                     arms along ±X, legs along -Y,
//!         ┌───── mantle ─────┐           facing -Z, feet at y = 0.
//!      sleeve            sleeve
//!         │                  │           1.80 m tall, 24 prisms,
//!       hand               hand ─ staff  5 materials, LOD1 at 25 m.
//!            tunic / sash
//!          ┌──────┴──────┐
//!        thigh         thigh
//!          │             │
//!        shin          shin
//!          │             │
//!        wrap          wrap
//! ```
//!
//! # Human form, and why that is the interesting one to build first
//!
//! The class's animal forms would be a different skeleton, a different set of cycles and a
//! different [`CharacterDef`] — a second character in everything but the name, and nothing in
//! the game asks for one yet. In human form the Druid is the *same* rig as the Warrior, which
//! is exactly the property the shared [`super::humanoid`] module exists to demonstrate: two
//! characters that look nothing alike, one skeleton, one set of cycles, different amplitudes.
//!
//! # What makes it read as a Druid at thirty metres
//!
//! **Antlers**, which break the head's outline where the mage's hood smooths it and the
//! Warrior's helm squares it; a **narrow, soft silhouette** — the opposite of the Warrior's
//! shoulders; and the **green glow at the head of the staff**, which is the only emissive
//! surface on the model. All three survive to LOD1.

use engine_core::math::{Vec2, Vec3};

use crate::character::{AnimationSlot, CharacterDef};
use crate::library::ModelId;
use crate::material::MaterialAsset;
use crate::mesh::SubMesh;
use crate::model::ModelAsset;
use crate::procedural::humanoid::{
    self, ARM_L, ARM_R, CHEST, FOOT_L, FOOT_R, FOREARM_L, FOREARM_R, HAND_L, HAND_R, HEAD, HIPS,
    MODEL_HEIGHT, NECK, PROP, SHIN_L, SHIN_R, SHOULDER_L, SHOULDER_R, SPINE, SkinBuilder, Style,
    THIGH_L, THIGH_R, limb,
};
use crate::texture::{TextureAsset, TextureFormat};

/// The identifier this model registers under.
pub const MODEL_NAME: &str = "druid";

/// Material slots, in the order [`druid`] builds them.
const MATERIAL_CLOTH: usize = 0;
const MATERIAL_SKIN: usize = 1;
const MATERIAL_LEATHER: usize = 2;
const MATERIAL_WOOD: usize = 3;
const MATERIAL_GLOW: usize = 4;

/// How much of the model to build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Detail {
    /// Every prism.
    Full,
    /// Tunic, mantle, antlers, one prism per leg, the staff. No sleeves, no hands, no
    /// separate knee, no glow.
    Reduced,
}

/// The tunic, the mantle and the sleeves.
fn cloth(detail: Detail) -> SubMesh {
    let mut builder = SkinBuilder::new();

    // Tunic: knee length rather than the mage's floor-length robe, so the legs read.
    builder.prism(
        &limb(
            Vec3::new(0.0, 0.52, 0.0),
            Vec3::new(0.0, 0.94, 0.0),
            Vec2::new(0.180, 0.150),
            Vec2::new(0.150, 0.115),
        )
        .with_uv_span(0.0, 0.40),
        HIPS,
        HIPS,
    );
    builder.prism(
        &limb(
            Vec3::new(0.0, 0.92, 0.0),
            Vec3::new(0.0, 1.16, 0.0),
            Vec2::new(0.148, 0.112),
            Vec2::new(0.158, 0.118),
        )
        .with_uv_span(0.40, 0.20),
        HIPS,
        SPINE,
    );
    // The mantle over the shoulders: a soft cape rather than a pauldron, and the widest the
    // character gets, which is still narrower than the Warrior's chest.
    builder.prism(
        &limb(
            Vec3::new(0.0, 1.16, 0.0),
            Vec3::new(0.0, 1.44, 0.0),
            Vec2::new(0.158, 0.118),
            Vec2::new(0.182, 0.128),
        )
        .with_uv_span(0.60, 0.40),
        SPINE,
        CHEST,
    );

    if detail == Detail::Full {
        for (side, shoulder, arm, forearm, hand) in [
            (1.0, SHOULDER_L, ARM_L, FOREARM_L, HAND_L),
            (-1.0, SHOULDER_R, ARM_R, FOREARM_R, HAND_R),
        ] {
            builder.prism(
                &limb(
                    Vec3::new(side * 0.05, 1.40, 0.0),
                    Vec3::new(side * 0.28, 1.38, 0.0),
                    Vec2::new(0.098, 0.098),
                    Vec2::new(0.068, 0.068),
                )
                .with_width_axis(Vec3::Y)
                .with_uv_span(0.60, 0.25),
                shoulder,
                arm,
            );
            builder.prism(
                &limb(
                    Vec3::new(side * 0.42, 1.39, 0.0),
                    Vec3::new(side * 0.60, 1.39, 0.0),
                    Vec2::new(0.056, 0.056),
                    Vec2::new(0.070, 0.070),
                )
                .with_width_axis(Vec3::Y)
                .with_uv_span(0.35, 0.25),
                forearm,
                hand,
            );
        }
    }

    builder.into_submesh("cloth", MATERIAL_CLOTH)
}

/// Head, neck and hands.
fn skin(detail: Detail) -> SubMesh {
    let mut builder = SkinBuilder::new();
    builder.prism(
        &limb(
            Vec3::new(0.0, 1.42, 0.0),
            Vec3::new(0.0, 1.72, 0.0),
            Vec2::new(0.094, 0.092),
            Vec2::new(0.098, 0.096),
        ),
        NECK,
        HEAD,
    );
    if detail == Detail::Full {
        for (side, hand) in [(1.0, HAND_L), (-1.0, HAND_R)] {
            builder.prism(
                &limb(
                    Vec3::new(side * 0.60, 1.39, 0.0),
                    Vec3::new(side * 0.70, 1.39, 0.0),
                    Vec2::new(0.048, 0.048),
                    Vec2::new(0.040, 0.040),
                )
                .with_width_axis(Vec3::Y),
                hand,
                hand,
            );
        }
    }
    builder.into_submesh("skin", MATERIAL_SKIN)
}

/// The antlers and the staff: everything grown rather than woven.
///
/// The antlers are two forked pairs on the head and the top of the character's silhouette;
/// the staff is a shaft rigidly bound to [`PROP`] under the right hand.
fn wood(detail: Detail) -> SubMesh {
    let mut builder = SkinBuilder::new();

    for side in [1.0_f32, -1.0] {
        // The main beam, leaning out and back from the temple. Its tip stops short of
        // `MODEL_HEIGHT` because the beam is *tilted*: a tilted prism's cap is a tilted
        // square, and its high corner rises above the axis by half the cross-section. The
        // number is what puts that corner exactly on 1.80, which is the height the model
        // declares and a test measures.
        builder.prism(
            &limb(
                Vec3::new(side * 0.075, 1.68, 0.02),
                Vec3::new(side * 0.165, 1.7866, -0.09),
                Vec2::new(0.024, 0.024),
                Vec2::new(0.013, 0.013),
            )
            .with_uv_span(0.0, 0.5),
            HEAD,
            HEAD,
        );
        if detail == Detail::Full {
            // One tine off each beam. Two would be a tree; none reads as a horn.
            builder.prism(
                &limb(
                    Vec3::new(side * 0.120, 1.74, -0.03),
                    Vec3::new(side * 0.135, 1.79, 0.06),
                    Vec2::new(0.014, 0.014),
                    Vec2::new(0.008, 0.008),
                )
                .with_uv_span(0.5, 0.5),
                HEAD,
                HEAD,
            );
        }
    }

    // The staff: shorter than the mage's, and carried rather than brandished.
    builder.prism(
        &limb(
            Vec3::new(-0.64, 0.46, 0.0),
            Vec3::new(-0.64, 1.92, 0.0),
            Vec2::new(0.021, 0.021),
            Vec2::new(0.026, 0.026),
        )
        .with_uv_span(0.0, 1.0),
        PROP,
        PROP,
    );
    // The crook at its head, which is what stops the staff reading as a spear.
    builder.prism(
        &limb(
            Vec3::new(-0.64, 1.92, 0.0),
            Vec3::new(-0.60, 2.00, -0.05),
            Vec2::new(0.026, 0.026),
            Vec2::new(0.018, 0.018),
        )
        .with_uv_span(0.0, 0.4),
        PROP,
        PROP,
    );

    builder.into_submesh("wood", MATERIAL_WOOD)
}

/// Sash, legs and the wraps on the feet.
fn leather(detail: Detail) -> SubMesh {
    let mut builder = SkinBuilder::new();

    if detail == Detail::Full {
        builder.prism(
            &limb(
                Vec3::new(0.0, 0.88, 0.0),
                Vec3::new(0.0, 0.96, 0.0),
                Vec2::new(0.156, 0.120),
                Vec2::new(0.152, 0.116),
            )
            .with_uv_span(0.55, 0.12),
            HIPS,
            HIPS,
        );
    }

    for (side, thigh, shin, foot) in [
        (1.0, THIGH_L, SHIN_L, FOOT_L),
        (-1.0, THIGH_R, SHIN_R, FOOT_R),
    ] {
        let x = side * 0.085;
        if detail == Detail::Full {
            builder.prism(
                &limb(
                    Vec3::new(x, 0.44, 0.0),
                    Vec3::new(x, 0.88, 0.0),
                    Vec2::new(0.058, 0.058),
                    Vec2::new(0.078, 0.078),
                )
                .with_uv_span(0.15, 0.3),
                shin,
                thigh,
            );
            builder.prism(
                &limb(
                    Vec3::new(x, 0.06, 0.0),
                    Vec3::new(x, 0.44, 0.0),
                    Vec2::new(0.050, 0.050),
                    Vec2::new(0.058, 0.058),
                )
                .with_uv_span(0.0, 0.15),
                foot,
                shin,
            );
        } else {
            builder.prism(
                &limb(
                    Vec3::new(x, 0.06, 0.0),
                    Vec3::new(x, 0.88, 0.0),
                    Vec2::new(0.050, 0.050),
                    Vec2::new(0.078, 0.078),
                ),
                shin,
                thigh,
            );
        }
        // Upright, for the reason the other two models give: a tilted cap sinks its leading
        // corners through the floor the character stands on.
        builder.prism(
            &limb(
                Vec3::new(x, 0.0, -0.022),
                Vec3::new(x, 0.12, -0.022),
                Vec2::new(0.062, 0.108),
                Vec2::new(0.054, 0.080),
            )
            .with_uv_span(0.0, 0.12),
            foot,
            foot,
        );
    }

    builder.into_submesh("leather", MATERIAL_LEATHER)
}

/// The light at the head of the staff — the only emissive surface on the character.
fn glow() -> SubMesh {
    let mut builder = SkinBuilder::new();
    builder.prism(
        &limb(
            Vec3::new(-0.60, 1.99, -0.05),
            Vec3::new(-0.60, 2.07, -0.05),
            Vec2::new(0.026, 0.026),
            Vec2::new(0.050, 0.050),
        ),
        PROP,
        PROP,
    );
    builder.prism(
        &limb(
            Vec3::new(-0.60, 2.07, -0.05),
            Vec3::new(-0.60, 2.15, -0.05),
            Vec2::new(0.050, 0.050),
            Vec2::new(0.012, 0.012),
        ),
        PROP,
        PROP,
    );
    builder.into_submesh("glow", MATERIAL_GLOW)
}

/// Cloth: a deep forest green that lightens towards the mantle, with a woven band at the hem.
fn cloth_texture() -> TextureAsset {
    const SIZE: u32 = 256;
    TextureAsset::generated(
        "druid_cloth",
        SIZE,
        SIZE,
        TextureFormat::Rgba8Srgb,
        |u, v| {
            let hem = [0.10, 0.17, 0.10];
            let body = [0.18, 0.32, 0.17];
            let mantle = [0.28, 0.44, 0.22];
            let mut color = if v < 0.5 {
                humanoid::mix(hem, body, v / 0.5)
            } else {
                humanoid::mix(body, mantle, (v - 0.5) / 0.5)
            };
            // The sash band, at the height the sash geometry sits.
            if (0.52..0.58).contains(&v) {
                color = humanoid::mix(color, [0.52, 0.40, 0.18], 0.75);
            }
            // A woven trim at the hem, so the bottom of the tunic has an edge.
            if v < 0.05 {
                color = humanoid::mix(color, [0.46, 0.38, 0.20], 0.6);
            }
            // A diagonal weave: coarser than the mage's vertical one, which is what makes
            // the two robes read as different cloth at the same distance.
            let weave = (((u + v) * SIZE as f32 * 0.35).sin() * 0.5 + 0.5) * 0.07;
            let grain = humanoid::hash01((u * SIZE as f32) as u32, (v * SIZE as f32) as u32) * 0.05;
            humanoid::texel(humanoid::mix(color, [0.0, 0.0, 0.0], weave + grain))
        },
    )
}

/// Skin: a paler, cooler tone than the Warrior's.
fn skin_texture() -> TextureAsset {
    const SIZE: u32 = 64;
    TextureAsset::generated(
        "druid_skin",
        SIZE,
        SIZE,
        TextureFormat::Rgba8Srgb,
        |u, v| {
            let grain = humanoid::hash01((u * SIZE as f32) as u32, (v * SIZE as f32) as u32) * 0.06;
            humanoid::texel(humanoid::mix([0.78, 0.62, 0.50], [0.58, 0.42, 0.34], grain))
        },
    )
}

/// Leather: a light tan, so the wraps read as hide rather than as the Warrior's boots.
fn leather_texture() -> TextureAsset {
    const SIZE: u32 = 128;
    TextureAsset::generated(
        "druid_leather",
        SIZE,
        SIZE,
        TextureFormat::Rgba8Srgb,
        |u, v| {
            let band = ((v * 38.0).sin() * 0.5 + 0.5) * 0.16;
            let grain = humanoid::hash01((u * SIZE as f32) as u32, (v * SIZE as f32) as u32) * 0.10;
            humanoid::texel(humanoid::mix(
                [0.42, 0.31, 0.20],
                [0.22, 0.16, 0.11],
                band + grain,
            ))
        },
    )
}

/// Wood: bark, for the antlers and the staff. Vertical fibre, knotted.
fn wood_texture() -> TextureAsset {
    const SIZE: u32 = 128;
    TextureAsset::generated(
        "druid_wood",
        SIZE,
        SIZE,
        TextureFormat::Rgba8Srgb,
        |u, v| {
            let fibre = ((u * 90.0).sin() * 0.5 + 0.5) * 0.22;
            let knot = humanoid::hash01((u * 16.0) as u32, (v * 24.0) as u32);
            let base = if knot > 0.93 {
                [0.26, 0.19, 0.12]
            } else {
                [0.44, 0.35, 0.24]
            };
            let grain = humanoid::hash01((u * SIZE as f32) as u32, (v * SIZE as f32) as u32) * 0.10;
            humanoid::texel(humanoid::mix(base, [0.16, 0.12, 0.08], fibre * 0.5 + grain))
        },
    )
}

/// The animation style: lighter and quicker than the Warrior, and a channel rather than a
/// swing.
fn style() -> Style {
    Style {
        arm_down_l: -80.0,
        arm_down_r: 80.0,
        walk_period: 0.96,
        run_period: 0.58,
        stride: 23.0,
        knee: 27.0,
        arm_swing: 20.0,
        bob: 0.020,
        lean: 4.0,
        breath: 1.5,
        cast: humanoid::CastStyle::Channel,
    }
}

/// Builds the Druid in human form.
///
/// # Errors
///
/// Returns [`crate::AssetError`] if the shared rig is malformed, which can only happen if
/// [`humanoid::skeleton`] is edited into an inconsistent state.
pub fn druid() -> Result<ModelAsset, crate::AssetError> {
    let materials = vec![
        MaterialAsset::colored("druid_cloth", 1.0, 1.0, 1.0)
            .with_roughness(0.80)
            .with_base_color_texture(0),
        MaterialAsset::colored("druid_skin", 1.0, 1.0, 1.0)
            .with_roughness(0.85)
            .with_base_color_texture(1),
        MaterialAsset::colored("druid_leather", 1.0, 1.0, 1.0)
            .with_roughness(0.72)
            .with_base_color_texture(2),
        MaterialAsset::colored("druid_wood", 1.0, 1.0, 1.0)
            .with_roughness(0.88)
            .with_base_color_texture(3),
        MaterialAsset::colored("druid_glow", 0.55, 1.0, 0.62)
            .with_roughness(0.35)
            .with_emissive(Vec3::new(0.35, 1.40, 0.55)),
    ];
    let textures = vec![
        cloth_texture(),
        skin_texture(),
        leather_texture(),
        wood_texture(),
    ];

    let lod0 = vec![
        cloth(Detail::Full),
        skin(Detail::Full),
        leather(Detail::Full),
        wood(Detail::Full),
        glow(),
    ];
    // LOD1 keeps the antlers, the staff and the tunic — the three things the silhouette is
    // made of — and drops the sleeves, the hands, the tines, the sash, the separate knee and
    // the glow, which is one fewer draw as well as fewer triangles.
    let lod1 = vec![
        cloth(Detail::Reduced),
        skin(Detail::Reduced),
        leather(Detail::Reduced),
        wood(Detail::Reduced),
    ];

    Ok(ModelAsset::rigid(MODEL_NAME, lod0, materials, textures)
        .with_lod(lod1, 25.0)
        .with_animation(humanoid::skeleton()?, humanoid::hero_clips(&style())))
}

/// The Druid's character definition: which model, how tall, and which clip plays when.
#[must_use]
pub fn druid_character() -> CharacterDef {
    CharacterDef::new("Druid", ModelId::new(MODEL_NAME), MODEL_HEIGHT)
        .with_speeds(1.85, 5.6)
        .with_clip(AnimationSlot::Idle, "idle")
        .with_clip(AnimationSlot::Walk, "walk")
        .with_clip(AnimationSlot::Run, "run")
        .with_clip(AnimationSlot::Jump, "jump")
        .with_clip(AnimationSlot::Fall, "fall")
        .with_clip(AnimationSlot::Land, "land")
        .with_clip(AnimationSlot::Cast, "cast")
        .with_clip(AnimationSlot::CastHeavy, "cast_heavy")
        .with_clip(AnimationSlot::Death, "death")
        .with_directional_clips()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validate::{ValidationLimits, validate_model};

    fn model() -> ModelAsset {
        match druid() {
            Ok(model) => model,
            Err(error) => panic!("the Druid must build: {error}"),
        }
    }

    #[test]
    fn the_stand_in_can_die() {
        let model = model();
        let Ok(bindings) = druid_character().resolve(&model) else {
            panic!("the Druid's own definition must resolve against its own model");
        };
        let Some(death) = bindings.get(AnimationSlot::Death) else {
            panic!("the Druid stand-in has no death clip");
        };
        assert!(!model.clips[death].looping, "a death plays once and holds");
        assert!(model.clips[death].duration > 0.0);
    }

    #[test]
    fn the_druid_passes_the_same_validation_an_imported_model_would() {
        let Ok(report) = validate_model(&model(), &ValidationLimits::default()) else {
            panic!("the Druid must validate");
        };
        assert_eq!(report.lods, 2);
        assert_eq!(report.materials, 5);
        assert_eq!(report.bones as usize, humanoid::BONE_COUNT);
        // The shared eight and a death.
        assert_eq!(report.clips, 9);
        assert!(
            (100..700).contains(&report.triangles),
            "a stylized prototype character should stay cheap: {} triangles",
            report.triangles
        );
    }

    #[test]
    fn the_antlers_are_the_top_of_the_character_and_it_stands_on_the_floor() {
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

        // The antlers live in the wood submesh with the staff, so the height they reach is
        // measured over the part of it that is bound to the head.
        let Some(wood) = lod.submeshes.iter().find(|submesh| submesh.name == "wood") else {
            panic!("the wood submesh must exist");
        };
        let crate::mesh::Geometry::Skinned(vertices) = &wood.geometry else {
            panic!("the wood must be skinned");
        };
        let antler_top = vertices
            .iter()
            .filter(|vertex| vertex.joints[0] == HEAD)
            .map(|vertex| vertex.position.y)
            .fold(f32::MIN, f32::max);
        assert!(
            (antler_top - MODEL_HEIGHT).abs() < 1e-3,
            "the antlers top out at {antler_top}, not at the declared {MODEL_HEIGHT}"
        );
    }

    #[test]
    fn the_druid_is_narrower_across_the_shoulders_than_the_warrior() {
        // The two silhouettes are meant to be told apart at a distance, and "narrower" is
        // the whole of the Druid's half of that contract.
        let width_of = |model: &ModelAsset, submesh_name: &str| {
            let Some(lod) = model.lod0() else {
                panic!("LOD0 must exist");
            };
            let Some(submesh) = lod
                .submeshes
                .iter()
                .find(|submesh| submesh.name == submesh_name)
            else {
                panic!("{submesh_name} must exist");
            };
            submesh
                .geometry
                .positions()
                .filter(|position| position.y > 1.25 && position.y < 1.50)
                .map(|position| position.x.abs())
                .fold(0.0_f32, f32::max)
        };
        let Ok(warrior) = super::super::warrior::warrior() else {
            panic!("the Warrior must build");
        };
        assert!(
            width_of(&model(), "cloth") < width_of(&warrior, "plate"),
            "the Druid's shoulders are not narrower than the Warrior's"
        );
    }

    #[test]
    fn only_the_staffs_light_is_emissive() {
        let model = model();
        let emissive: Vec<&str> = model
            .materials
            .iter()
            .filter(|material| material.emissive.length_squared() > 0.0)
            .map(|material| material.name.as_str())
            .collect();
        assert_eq!(emissive, ["druid_glow"]);
    }
}
