//! Training dummies: a post, a straw body, and a crossbar.
//!
//! ```text
//!            sack head            Rigid. No skeleton, no clips, no per-frame
//!               │                 animation cost — because the benchmark case
//!      ┌──── crossbar ────┐       is a hundred of these on screen at once and
//!      │   straw torso    │       the phase brief says to keep them cheap.
//!      pad     post     pad
//!               │
//!             plinth
//! ```
//!
//! # Why a dummy is not a skinned character
//!
//! A dummy is hit, it flashes, it falls over as a colour change, and that is all it does. It
//! has no locomotion and no cast. Giving it a skeleton would add a bone palette upload and a
//! skinned draw per instance, take it out of the instanced batch path that draws all hundred
//! of them in one call, and buy nothing. It stays rigid, and the phase's performance
//! measurements are the argument.
//!
//! # Why the two variants share one builder
//!
//! The single-target dummy and the group dummies differ in height and in the colour of their
//! wrap, and in nothing else. Two hand-written models would be two things to keep in sync;
//! one builder with a [`DummyVariant`] is one.

use engine_core::math::{Vec2, Vec3};

use crate::material::MaterialAsset;
use crate::mesh::{Geometry, MeshVertex, SubMesh};
use crate::model::ModelAsset;
use crate::procedural::Prism;
use crate::texture::{TextureAsset, TextureFormat};

/// How tall the single-target dummy is authored, in metres.
pub const MODEL_HEIGHT: f32 = 1.90;

/// The identifier the single-target dummy registers under.
pub const SINGLE_MODEL_NAME: &str = "training_dummy";
/// The identifier the group dummies register under.
pub const GROUP_MODEL_NAME: &str = "training_dummy_group";

const MATERIAL_WOOD: usize = 0;
const MATERIAL_STRAW: usize = 1;
const MATERIAL_WRAP: usize = 2;

/// Which dummy to build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DummyVariant {
    /// The lone dummy the player starts on: full height, red wrap.
    Single,
    /// One of the group: shorter, blue wrap, so a glance separates the two roles.
    Group,
}

impl DummyVariant {
    /// The identifier this variant registers under.
    #[must_use]
    pub fn model_name(self) -> &'static str {
        match self {
            Self::Single => SINGLE_MODEL_NAME,
            Self::Group => GROUP_MODEL_NAME,
        }
    }

    /// How tall this variant is authored, in metres.
    #[must_use]
    pub fn model_height(self) -> f32 {
        match self {
            Self::Single => MODEL_HEIGHT,
            Self::Group => MODEL_HEIGHT * 0.88,
        }
    }

    /// The linear colour of the wrap.
    fn wrap_color(self) -> (f32, f32, f32) {
        match self {
            Self::Single => (0.62, 0.14, 0.12),
            Self::Group => (0.14, 0.28, 0.55),
        }
    }
}

/// Accumulates prisms into one submesh of rigid geometry.
struct MeshBuilder {
    vertices: Vec<MeshVertex>,
    indices: Vec<u32>,
}

impl MeshBuilder {
    fn new() -> Self {
        Self {
            vertices: Vec::new(),
            indices: Vec::new(),
        }
    }

    fn prism(&mut self, prism: &Prism) {
        let base = self.vertices.len() as u32;
        let (corners, indices) = prism.build(base);
        for (position, normal, uv) in corners {
            self.vertices.push(MeshVertex::new(position, normal, uv));
        }
        self.indices.extend(indices);
    }

    fn is_empty(&self) -> bool {
        self.vertices.is_empty()
    }

    fn into_submesh(self, name: &str, material: usize) -> SubMesh {
        SubMesh::new(
            name,
            Geometry::Static(self.vertices),
            self.indices,
            material,
        )
    }
}

/// How much of the dummy to build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Detail {
    /// Everything: plinth, post, crossbar, torso, head, wrap, two arm pads.
    Full,
    /// Plinth, post, torso, head. No crossbar, no pads, no wrap.
    Reduced,
    /// Post and torso. What a dummy is at 60 metres.
    Silhouette,
}

/// Builds one level of detail as a list of submeshes.
///
/// `scale` shortens the group variant without a second set of numbers.
fn level(detail: Detail, scale: f32) -> Vec<SubMesh> {
    let mut wood = MeshBuilder::new();
    let mut straw = MeshBuilder::new();
    let mut wrap = MeshBuilder::new();

    let y = |value: f32| value * scale;
    let half = |x: f32, z: f32| Vec2::new(x * scale, z * scale);

    // The post runs the full height and is what the crossbar and the body hang off.
    wood.prism(
        &Prism::new(
            Vec3::new(0.0, y(0.04), 0.0),
            Vec3::new(0.0, y(1.86), 0.0),
            0.075 * scale,
            0.058 * scale,
        )
        .with_uv_span(0.0, 1.0),
    );
    straw.prism(
        &Prism::new(
            Vec3::new(0.0, y(0.74), 0.0),
            Vec3::new(0.0, y(1.46), 0.0),
            0.0,
            0.0,
        )
        .with_halves(half(0.27, 0.21), half(0.22, 0.18))
        .with_uv_span(0.0, 0.6),
    );

    if detail <= Detail::Reduced {
        wood.prism(
            &Prism::new(
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(0.0, y(0.11), 0.0),
                0.34 * scale,
                0.29 * scale,
            )
            .with_uv_span(0.0, 0.1),
        );
        straw.prism(
            &Prism::new(
                Vec3::new(0.0, y(1.50), 0.0),
                Vec3::new(0.0, y(1.80), 0.0),
                0.0,
                0.0,
            )
            .with_halves(half(0.165, 0.150), half(0.135, 0.125))
            .with_uv_span(0.6, 0.4),
        );
    }

    if detail == Detail::Full {
        wood.prism(
            &Prism::new(
                Vec3::new(-0.56 * scale, y(1.34), 0.0),
                Vec3::new(0.56 * scale, y(1.34), 0.0),
                0.052 * scale,
                0.052 * scale,
            )
            .with_width_axis(Vec3::Y),
        );
        // The wrap: a band across the chest, which is where the target marker and every hit
        // flash read from.
        wrap.prism(
            &Prism::new(
                Vec3::new(0.0, y(1.02), 0.0),
                Vec3::new(0.0, y(1.26), 0.0),
                0.0,
                0.0,
            )
            .with_halves(half(0.256, 0.202), half(0.244, 0.194)),
        );
        for side in [-1.0_f32, 1.0] {
            wrap.prism(
                &Prism::new(
                    Vec3::new(side * 0.44 * scale, y(1.34), 0.0),
                    Vec3::new(side * 0.60 * scale, y(1.34), 0.0),
                    0.075 * scale,
                    0.062 * scale,
                )
                .with_width_axis(Vec3::Y),
            );
        }
    }

    let mut submeshes = vec![
        wood.into_submesh("post", MATERIAL_WOOD),
        straw.into_submesh("body", MATERIAL_STRAW),
    ];
    if !wrap.is_empty() {
        submeshes.push(wrap.into_submesh("wrap", MATERIAL_WRAP));
    }
    submeshes
}

/// Deterministic value in `0..1` from a pair of integers.
fn hash01(x: u32, y: u32) -> f32 {
    let mut value = x.wrapping_mul(0x27D4_EB2D) ^ y.wrapping_mul(0x1656_67B1);
    value ^= value >> 15;
    value = value.wrapping_mul(0x2545_F491);
    value ^= value >> 13;
    (value & 0xFFFF) as f32 / 65535.0
}

/// Weathered wood: vertical grain, darker towards the ground.
fn wood_texture() -> TextureAsset {
    const SIZE: u32 = 128;
    TextureAsset::generated(
        "training_dummy_wood",
        SIZE,
        SIZE,
        TextureFormat::Rgba8Srgb,
        |u, v| {
            let grain = ((u * 33.0).sin() * 0.5 + 0.5) * 0.22;
            let dirt = (1.0 - v) * 0.25;
            let noise = hash01((u * SIZE as f32) as u32, (v * SIZE as f32) as u32) * 0.12;
            let base = [0.42, 0.30, 0.19];
            let dark = [0.17, 0.12, 0.08];
            let t = (grain + dirt + noise).clamp(0.0, 1.0);
            [
                ((base[0] + (dark[0] - base[0]) * t) * 255.0) as u8,
                ((base[1] + (dark[1] - base[1]) * t) * 255.0) as u8,
                ((base[2] + (dark[2] - base[2]) * t) * 255.0) as u8,
                255,
            ]
        },
    )
}

/// Straw: a bright, noisy, horizontally-banded surface.
fn straw_texture() -> TextureAsset {
    const SIZE: u32 = 128;
    TextureAsset::generated(
        "training_dummy_straw",
        SIZE,
        SIZE,
        TextureFormat::Rgba8Srgb,
        |u, v| {
            let strands = ((v * 70.0 + (u * 9.0).sin() * 2.0).sin() * 0.5 + 0.5) * 0.35;
            let noise = hash01((u * SIZE as f32) as u32, (v * SIZE as f32) as u32) * 0.20;
            let base = [0.78, 0.66, 0.34];
            let dark = [0.44, 0.34, 0.16];
            let t = (strands + noise).clamp(0.0, 1.0);
            [
                ((base[0] + (dark[0] - base[0]) * t) * 255.0) as u8,
                ((base[1] + (dark[1] - base[1]) * t) * 255.0) as u8,
                ((base[2] + (dark[2] - base[2]) * t) * 255.0) as u8,
                255,
            ]
        },
    )
}

/// Builds a training dummy.
///
/// Three levels of detail, because the dummy is the model that appears a hundred times: the
/// LOD architecture is worth more here than it is on the one character.
#[must_use]
pub fn training_dummy(variant: DummyVariant) -> ModelAsset {
    let scale = variant.model_height() / MODEL_HEIGHT;
    let (red, green, blue) = variant.wrap_color();

    let materials = vec![
        MaterialAsset::colored("training_dummy_wood", 1.0, 1.0, 1.0)
            .with_roughness(0.92)
            .with_base_color_texture(0),
        MaterialAsset::colored("training_dummy_straw", 1.0, 1.0, 1.0)
            .with_roughness(0.88)
            .with_base_color_texture(1),
        MaterialAsset::colored("training_dummy_wrap", red, green, blue).with_roughness(0.80),
    ];

    ModelAsset::rigid(
        variant.model_name(),
        level(Detail::Full, scale),
        materials,
        vec![wood_texture(), straw_texture()],
    )
    .with_lod(level(Detail::Reduced, scale), 22.0)
    .with_lod(level(Detail::Silhouette, scale), 55.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validate::{ValidationLimits, validate_model};

    #[test]
    fn both_variants_validate_and_stay_cheap() {
        for variant in [DummyVariant::Single, DummyVariant::Group] {
            let model = training_dummy(variant);
            let Ok(report) = validate_model(&model, &ValidationLimits::default()) else {
                panic!("{variant:?} must validate");
            };
            assert_eq!(report.lods, 3);
            assert_eq!(report.bones, 0, "a dummy has no skeleton");
            assert_eq!(report.clips, 0, "a dummy has no animation");
            assert!(
                report.triangles <= 120,
                "{variant:?} costs {} triangles, and there are a hundred of them",
                report.triangles
            );
        }
    }

    #[test]
    fn a_dummy_is_rigid_so_it_stays_in_the_instanced_batch_path() {
        let model = training_dummy(DummyVariant::Single);
        assert!(!model.is_skinned());
        for lod in &model.lods {
            assert!(!lod.is_skinned());
        }
    }

    #[test]
    fn each_level_is_cheaper_than_the_one_before_it() {
        let model = training_dummy(DummyVariant::Single);
        let counts: Vec<usize> = model.lods.iter().map(|lod| lod.triangle_count()).collect();
        assert_eq!(counts.len(), 3);
        assert!(counts[0] > counts[1], "{counts:?}");
        assert!(counts[1] > counts[2], "{counts:?}");
    }

    #[test]
    fn the_silhouette_level_is_still_a_recognisable_post_and_body() {
        let model = training_dummy(DummyVariant::Single);
        let Some(lod) = model.lods.last() else {
            panic!("three levels exist");
        };
        assert_eq!(lod.submeshes.len(), 2, "post and body, nothing else");
        assert!(lod.bounds().radius > 0.2);
    }

    #[test]
    fn the_group_variant_is_shorter_and_a_different_colour() {
        // The two roles have to be distinguishable at a glance, which is the whole reason
        // there are two variants rather than one model used twice.
        let single = training_dummy(DummyVariant::Single);
        let group = training_dummy(DummyVariant::Group);

        let top = |model: &ModelAsset| {
            let Some(lod) = model.lod0() else {
                panic!("LOD0 exists");
            };
            lod.submeshes
                .iter()
                .flat_map(|submesh| submesh.geometry.positions())
                .map(|position| position.y)
                .fold(f32::MIN, f32::max)
        };
        assert!(top(&group) < top(&single) - 0.1);
        assert_ne!(
            single.materials[MATERIAL_WRAP].base_color,
            group.materials[MATERIAL_WRAP].base_color
        );
    }

    #[test]
    fn the_single_dummy_is_as_tall_as_it_claims_and_stands_on_the_floor() {
        let model = training_dummy(DummyVariant::Single);
        let Some(lod) = model.lod0() else {
            panic!("LOD0 exists");
        };
        let mut lowest = f32::MAX;
        let mut highest = f32::MIN;
        for position in lod
            .submeshes
            .iter()
            .flat_map(|submesh| submesh.geometry.positions())
        {
            lowest = lowest.min(position.y);
            highest = highest.max(position.y);
        }
        assert!(lowest.abs() < 1e-4, "the plinth sits at y = {lowest}");
        assert!(
            (highest - MODEL_HEIGHT).abs() < 0.1,
            "the dummy tops out at {highest}, and claims {MODEL_HEIGHT}"
        );
    }

    #[test]
    fn the_two_variants_share_their_textures_byte_for_byte() {
        // Two models, one texture pair: the wrap colour is a material constant, not a
        // second image, which is what keeps texture memory flat as dummies are added.
        let single = training_dummy(DummyVariant::Single);
        let group = training_dummy(DummyVariant::Group);
        assert_eq!(single.textures, group.textures);
    }

    #[test]
    fn a_dummy_is_the_same_every_time_it_is_built() {
        assert_eq!(
            training_dummy(DummyVariant::Single),
            training_dummy(DummyVariant::Single)
        );
    }
}
