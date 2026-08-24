//! Surface parameters, plus the texture slots this phase adds.
//!
//! This mirrors `engine_renderer::MaterialDesc` — the same five constants — and adds four
//! optional texture references. It is a separate type for the same reason [`crate::MeshVertex`]
//! is: an asset must be describable without a device.
//!
//! # Why four slots and not a graph
//!
//! Base colour, normal, one packed occlusion/roughness/metallic map and emissive is the
//! glTF metallic-roughness model, which is what every DCC tool exports and what an artist
//! will hand over. Matching it exactly means the importer copies rather than translates. A
//! material *graph* is a different feature that buys shader permutations, and the project's
//! stated target is very high frame rates with a small number of pipeline states.
//!
//! The four slots are the renderer's four descriptors, in the same order — see
//! `engine_renderer::material::MaterialTextureSlot`.

use engine_core::math::{Vec3, Vec4};

/// How a surface looks.
#[derive(Debug, Clone, PartialEq)]
pub struct MaterialAsset {
    /// What it is, for diagnostics: `"fire_mage_robe"`.
    pub name: String,
    /// Linear RGBA base colour, multiplied with the base colour texture.
    pub base_color: Vec4,
    /// 0 is mirror-smooth, 1 is fully diffuse.
    pub roughness: f32,
    /// 0 is dielectric, 1 is metal.
    pub metallic: f32,
    /// Colour emitted regardless of lighting.
    pub emissive: Vec3,
    /// Index into [`crate::ModelAsset::textures`], sampled in sRGB.
    ///
    /// `None` means the material is a flat colour, and the renderer binds a 1×1 white
    /// texture rather than compiling a second shader for the case.
    pub base_color_texture: Option<usize>,
    /// Tangent-space normal map. Carried and validated; the Phase 5 shader does not yet
    /// read it, because a normal map without tangents on the vertex is a wrong answer
    /// rather than a cheap one.
    pub normal_texture: Option<usize>,
    /// Packed occlusion (R), roughness (G), metallic (B), the glTF convention. Carried and
    /// validated, not yet sampled.
    pub orm_texture: Option<usize>,
    /// Emissive colour, sampled in sRGB and multiplied with [`Self::emissive`].
    ///
    /// glTF's `emissiveTexture`. Carried and validated; the Phase 5 shader binds it and does
    /// not yet read it, for the same reason as the normal map — the lighting model that
    /// consumes it is a later milestone, and this one exists to freeze the binding.
    pub emissive_texture: Option<usize>,
    /// Below this alpha a fragment is discarded, in the camera pass and the shadow pass
    /// alike. Zero is an opaque material and performs no test.
    ///
    /// This is glTF's `MASK` alpha mode and its `alphaCutoff`. `BLEND` is deliberately not
    /// represented: the renderer draws no sorted transparent geometry, and importing a
    /// blended material as a cutout is a closer answer than importing it as opaque — a leaf
    /// card with a hard edge rather than a rectangle.
    pub alpha_cutoff: f32,
}

impl Default for MaterialAsset {
    fn default() -> Self {
        Self {
            name: "default".to_owned(),
            base_color: Vec4::new(0.8, 0.8, 0.8, 1.0),
            roughness: 0.8,
            metallic: 0.0,
            emissive: Vec3::ZERO,
            base_color_texture: None,
            normal_texture: None,
            orm_texture: None,
            emissive_texture: None,
            alpha_cutoff: 0.0,
        }
    }
}

impl MaterialAsset {
    /// A flat, untextured material of the given linear colour.
    #[must_use]
    pub fn colored(name: impl Into<String>, red: f32, green: f32, blue: f32) -> Self {
        Self {
            name: name.into(),
            base_color: Vec4::new(red, green, blue, 1.0),
            ..Self::default()
        }
    }

    /// The same material with its roughness replaced, clamped to `0..=1`.
    #[must_use]
    pub fn with_roughness(mut self, roughness: f32) -> Self {
        self.roughness = roughness.clamp(0.0, 1.0);
        self
    }

    /// The same material with its metalness replaced, clamped to `0..=1`.
    #[must_use]
    pub fn with_metallic(mut self, metallic: f32) -> Self {
        self.metallic = metallic.clamp(0.0, 1.0);
        self
    }

    /// The same material with an emissive term.
    #[must_use]
    pub fn with_emissive(mut self, emissive: Vec3) -> Self {
        self.emissive = emissive;
        self
    }

    /// Returns the material as a cutout, discarding fragments below `cutoff`.
    ///
    /// What foliage, a fence and a grate are made of. Clamped into [0, 1]: a cutoff above
    /// one discards every fragment, which renders as an object that has vanished with no
    /// error anywhere.
    #[must_use]
    pub fn with_alpha_cutoff(mut self, cutoff: f32) -> Self {
        self.alpha_cutoff = if cutoff.is_finite() {
            cutoff.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self
    }

    /// Whether this material needs an alpha test.
    #[must_use]
    pub fn is_alpha_tested(&self) -> bool {
        self.alpha_cutoff > 0.0
    }

    /// The same material sampling `texture` for base colour.
    #[must_use]
    pub fn with_base_color_texture(mut self, texture: usize) -> Self {
        self.base_color_texture = Some(texture);
        self
    }

    /// The same material sampling `texture` for tangent-space normals.
    #[must_use]
    pub fn with_normal_texture(mut self, texture: usize) -> Self {
        self.normal_texture = Some(texture);
        self
    }

    /// The same material sampling `texture` for occlusion, roughness and metallic.
    #[must_use]
    pub fn with_orm_texture(mut self, texture: usize) -> Self {
        self.orm_texture = Some(texture);
        self
    }

    /// The same material sampling `texture` for emissive colour.
    #[must_use]
    pub fn with_emissive_texture(mut self, texture: usize) -> Self {
        self.emissive_texture = Some(texture);
        self
    }

    /// Every texture index this material names, in slot order.
    ///
    /// The order is the renderer's descriptor order — base colour, normal, ORM, emissive —
    /// so this doubles as the readable statement of which maps a material carries.
    ///
    /// What validation walks to prove no material points past the end of the texture array.
    pub fn texture_indices(&self) -> impl Iterator<Item = (&'static str, usize)> + '_ {
        [
            ("base_color", self.base_color_texture),
            ("normal", self.normal_texture),
            ("orm", self.orm_texture),
            ("emissive", self.emissive_texture),
        ]
        .into_iter()
        .filter_map(|(slot, index)| index.map(|index| (slot, index)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builders_clamp_rather_than_produce_unusable_values() {
        assert_eq!(MaterialAsset::default().with_roughness(3.0).roughness, 1.0);
        assert_eq!(MaterialAsset::default().with_metallic(-1.0).metallic, 0.0);
    }

    #[test]
    fn only_the_slots_that_are_bound_are_reported() {
        let material = MaterialAsset::colored("robe", 0.4, 0.1, 0.1).with_base_color_texture(2);
        let named: Vec<_> = material.texture_indices().collect();
        assert_eq!(named, vec![("base_color", 2)]);
    }

    #[test]
    fn an_untextured_material_names_no_textures() {
        assert_eq!(MaterialAsset::default().texture_indices().count(), 0);
    }
}
