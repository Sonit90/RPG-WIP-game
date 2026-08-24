//! Surface parameters.
//!
//! A material is what a surface looks like: a colour and two scalars. It is deliberately
//! not a shader, a pass, a technique or a permutation system — one pipeline draws every
//! material in Phase 2, and the material only supplies constants to it.
//!
//! Constants live in a device-local buffer written once at creation, because a material's
//! parameters change when the material changes, which is approximately never. Per-frame
//! data belongs in the upload ring; this does not.

use engine_core::math::{Vec3, Vec4};

use crate::device::RenderDevice;
use crate::error::RendererError;
use crate::renderer::TextureHandle;
use crate::resource::GpuBuffer;
use crate::upload::{CONSTANT_BUFFER_ALIGNMENT, StagingUploader};

use windows::Win32::Graphics::Direct3D12::D3D12_RESOURCE_STATE_VERTEX_AND_CONSTANT_BUFFER;

/// How a surface should look.
///
/// `roughness` and `metallic` are here because the lighting model in Milestone 14 needs
/// somewhere to read them from, and adding a field to a constant buffer later means
/// changing the struct, the shader and the alignment padding together. They are not a
/// claim that the renderer is physically based.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MaterialDesc {
    /// Linear RGBA base colour. Alpha is carried but not blended — Phase 2 is opaque only.
    pub base_color: Vec4,
    /// 0 is mirror-smooth, 1 is fully diffuse.
    pub roughness: f32,
    /// 0 is dielectric, 1 is metal. Values between the two are not physical.
    pub metallic: f32,
    /// Colour emitted regardless of lighting.
    pub emissive: Vec3,
    /// How strongly the normal map perturbs the surface. glTF's `normalTexture.scale`.
    ///
    /// The sampled normal's X and Y are multiplied by it before the vector is renormalized:
    /// 1 is the map as authored, 0 is the geometric normal, above 1 exaggerates. A material
    /// with no normal map is unaffected by it, because the flat fallback's X and Y are zero
    /// and scaling zero changes nothing.
    pub normal_scale: f32,
    /// How much of the ORM map's occlusion channel is applied. glTF's
    /// `occlusionTexture.strength`.
    ///
    /// Blended towards "not occluded" by glTF's own formula, `1 + strength * (sampled - 1)`,
    /// so 1 applies the map and 0 ignores it.
    pub occlusion_strength: f32,
    /// Below this alpha a fragment is discarded, in both the camera pass and the shadow pass.
    ///
    /// Zero is an opaque material and performs no test, which is what nearly every surface
    /// in a scene is. A positive value is a *cutout* material — foliage, a fence, a grate —
    /// and it is the whole of the transparency the renderer supports: there is no sorted
    /// blended geometry pass, so a value between 0 and 1 is a hard edge and not a fade.
    ///
    /// It is on the material rather than on the entity because it is a property of the
    /// surface: two instances of the same leaf card cut out at the same threshold, and the
    /// shadow pass needs the same number the camera pass uses or a leaf's shadow is a
    /// rectangle.
    pub alpha_cutoff: f32,
    /// Whether this surface is shaded with the metallic-roughness BRDF.
    ///
    /// Off by default, and off is Lambert: one diffuse lobe, no specular, `roughness` and
    /// `metallic` carried but unread. The BRDF is 0.150 ms of a 0.351 ms frame at 1440p
    /// (`docs/performance/shadows-off-permutation.md`) and nothing in the game is authored
    /// for it yet, so a material pays for it only by asking.
    ///
    /// It selects a *shader permutation*, not a branch: a material without it is drawn with
    /// a pixel shader that does not contain GGX, Smith or Fresnel at all.
    pub pbr: bool,
}

impl Default for MaterialDesc {
    fn default() -> Self {
        Self {
            base_color: Vec4::new(0.8, 0.8, 0.8, 1.0),
            roughness: 0.8,
            metallic: 0.0,
            emissive: Vec3::ZERO,
            // Both are glTF's defaults, and both mean "apply the map as authored". A
            // material that carried zero here would silently disable a map it does have.
            normal_scale: 1.0,
            occlusion_strength: 1.0,
            alpha_cutoff: 0.0,
            // Lambert until a material asks otherwise. See the field.
            pbr: false,
        }
    }
}

impl MaterialDesc {
    /// An opaque material of the given linear colour.
    #[must_use]
    pub fn colored(red: f32, green: f32, blue: f32) -> Self {
        Self {
            base_color: Vec4::new(red, green, blue, 1.0),
            ..Self::default()
        }
    }

    /// Returns the description with its roughness replaced, clamped to [0, 1].
    #[must_use]
    pub fn with_roughness(mut self, roughness: f32) -> Self {
        self.roughness = roughness.clamp(0.0, 1.0);
        self
    }

    /// Returns the description with its metallic value replaced, clamped to [0, 1].
    #[must_use]
    pub fn with_metallic(mut self, metallic: f32) -> Self {
        self.metallic = metallic.clamp(0.0, 1.0);
        self
    }

    /// Returns the description with its emissive colour replaced.
    #[must_use]
    pub fn with_emissive(mut self, emissive: Vec3) -> Self {
        self.emissive = emissive;
        self
    }

    /// Returns the description shaded with the metallic-roughness BRDF.
    ///
    /// The one way to opt a material into PBR. Everything else about it is unchanged: the
    /// same constants, the same maps, the same descriptor table - only the pixel shader
    /// permutation the renderer binds for it differs.
    #[must_use]
    pub fn with_pbr(mut self, pbr: bool) -> Self {
        self.pbr = pbr;
        self
    }

    /// Returns the description with its normal-map scale replaced, clamped to non-negative.
    #[must_use]
    pub fn with_normal_scale(mut self, scale: f32) -> Self {
        self.normal_scale = if scale.is_finite() {
            scale.max(0.0)
        } else {
            1.0
        };
        self
    }

    /// Returns the description with its occlusion strength replaced, clamped to [0, 1].
    #[must_use]
    pub fn with_occlusion_strength(mut self, strength: f32) -> Self {
        self.occlusion_strength = if strength.is_finite() {
            strength.clamp(0.0, 1.0)
        } else {
            1.0
        };
        self
    }

    /// Returns the description as a cutout material with the given alpha threshold.
    ///
    /// Clamped into (0, 1]: zero would mean "opaque" and is not a cutout at all, and a
    /// threshold above one discards every fragment, which renders as an object that has
    /// vanished with no error anywhere.
    #[must_use]
    pub fn with_alpha_cutoff(mut self, cutoff: f32) -> Self {
        self.alpha_cutoff = if cutoff.is_finite() {
            cutoff.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self
    }

    /// Whether this material needs the alpha test in the depth passes.
    ///
    /// What selects the shadow pass's cutout pipeline over its opaque one — and an opaque
    /// material must answer `false`, because the opaque pipeline binds no pixel shader at
    /// all and is markedly faster for it.
    #[must_use]
    pub fn is_alpha_tested(&self) -> bool {
        self.alpha_cutoff > 0.0
    }

    /// Rejects values a shader cannot use.
    ///
    /// `NaN` is the one that matters: it propagates through the whole lighting expression
    /// and produces a black or white pixel with no indication of where it came from.
    fn validate(&self) -> Result<(), RendererError> {
        let finite = self.base_color.dot(self.base_color).is_finite()
            && self.roughness.is_finite()
            && self.metallic.is_finite()
            && self.alpha_cutoff.is_finite()
            && self.normal_scale.is_finite()
            && self.occlusion_strength.is_finite()
            && self.emissive.length_squared().is_finite();
        if !finite {
            return Err(RendererError::InvalidConfig {
                reason: "material parameters must be finite".to_owned(),
            });
        }
        if !(0.0..=1.0).contains(&self.roughness) || !(0.0..=1.0).contains(&self.metallic) {
            return Err(RendererError::InvalidConfig {
                reason: format!(
                    "roughness and metallic must be in [0, 1], got {} and {}",
                    self.roughness, self.metallic
                ),
            });
        }
        // glTF states no upper bound on the normal scale and an exact [0, 1] for the
        // occlusion strength, and this follows it: a scale of 2 is an artist exaggerating a
        // map, an occlusion strength of 2 is a number with no meaning.
        if self.normal_scale < 0.0 || !(0.0..=1.0).contains(&self.occlusion_strength) {
            return Err(RendererError::InvalidConfig {
                reason: format!(
                    "the normal scale must be non-negative and the occlusion strength in \
                     [0, 1], got {} and {}",
                    self.normal_scale, self.occlusion_strength
                ),
            });
        }
        Ok(())
    }
}

/// How many textures one material binds, and therefore how many descriptors its table holds.
///
/// Four, permanently: base colour, normal, ORM, emissive. It is a constant rather than a
/// literal because the root signature's range, the descriptor allocation and the fallback
/// set all have to be the same number, and three places agreeing by coincidence is how a
/// table ends up one descriptor short of what the shader reads.
pub const MATERIAL_TEXTURE_COUNT: u32 = 4;

/// The material texture ABI: which shader register each map is bound to.
///
/// **This order is frozen.** The root signature declares one contiguous four-SRV range at
/// `t0`, and the descriptors are written into it in exactly this order. Changing it means
/// changing `shaders/material.hlsli` in the same edit, and `tests/root_signature_mapping.rs`
/// asserts the two agree.
///
/// ```text
/// t0  base colour   sRGB, multiplied with `base_color`
/// t1  normal        tangent-space, linear
/// t2  ORM           R occlusion, G roughness, B metallic — the glTF packing
/// t3  emissive      sRGB
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u32)]
pub enum MaterialTextureSlot {
    /// `t0`. Multiplied with [`MaterialDesc::base_color`].
    BaseColor = 0,
    /// `t1`. Tangent-space normals, applied through the vertex tangent frame.
    Normal = 1,
    /// `t2`. Occlusion in R, roughness in G, metallic in B. Each channel scales the matching
    /// [`MaterialDesc`] field.
    Orm = 2,
    /// `t3`. Multiplied with [`MaterialDesc::emissive`].
    Emissive = 3,
}

impl MaterialTextureSlot {
    /// Every slot, in descriptor order. Iterating this **is** the ABI.
    pub const ALL: [Self; MATERIAL_TEXTURE_COUNT as usize] =
        [Self::BaseColor, Self::Normal, Self::Orm, Self::Emissive];

    /// The shader register — and the offset into the material's descriptor range.
    #[must_use]
    pub fn register(self) -> u32 {
        self as u32
    }

    /// The texel the slot's shared fallback texture holds, as straight RGBA8.
    ///
    /// The **identity** for the slot, so a material with no such map and a material sampling
    /// the fallback are the same picture:
    ///
    /// * base colour `(255, 255, 255, 255)` — the map multiplies `base_color`.
    /// * normal `(128, 128, 255, 255)` — the flat tangent-space normal. `0.5` unpacks to
    ///   zero through `2x - 1`, giving `+Z`: the interpolated vertex normal, unchanged.
    /// * ORM `(255, 255, 255, 255)` — glTF's convention is that this map *scales* the
    ///   roughness and metallic factors, so white passes [`MaterialDesc::roughness`] and
    ///   [`MaterialDesc::metallic`] through untouched and an occlusion of 1 is "not
    ///   occluded". Baking a default roughness into the texel instead would apply that
    ///   default a second time for every material that overrides it.
    /// * emissive `(255, 255, 255, 255)` — glTF's convention again: the map *scales*
    ///   [`MaterialDesc::emissive`], so white passes the factor through and a material that
    ///   emits nothing does so because its factor is zero, which is the glTF default. A black
    ///   fallback would look equally correct until the first material that sets an emissive
    ///   colour and has no map, which would then emit nothing at all.
    ///
    /// Here rather than in the renderer so the window path and the offscreen path cannot
    /// disagree about what a missing map looks like, which is exactly the sort of difference
    /// a pixel test is supposed to be evidence against.
    #[must_use]
    pub fn fallback_texel(self) -> [u8; 4] {
        match self {
            Self::BaseColor | Self::Orm | Self::Emissive => [255, 255, 255, 255],
            Self::Normal => [128, 128, 255, 255],
        }
    }

    /// What a texture in this slot means, which decides its colour space and its mip filter.
    ///
    /// The one place the mapping lives. A normal map is not merely "not colour": its mips
    /// have to be filtered as directions and renormalized, which is a third case that
    /// `is_srgb` alone cannot express — see [`crate::mipmap::TextureKind`].
    #[must_use]
    pub fn texture_kind(self) -> crate::mipmap::TextureKind {
        match self {
            Self::BaseColor | Self::Emissive => crate::mipmap::TextureKind::Color,
            Self::Normal => crate::mipmap::TextureKind::Normal,
            Self::Orm => crate::mipmap::TextureKind::Data,
        }
    }

    /// Whether the slot's textures are read through an sRGB view.
    ///
    /// Colour is sRGB and data is not. Getting it backwards is the classic washed-out or
    /// muddy import, and it has no other symptom.
    #[must_use]
    pub fn is_srgb(self) -> bool {
        self.texture_kind().is_srgb()
    }

    /// The slot's name, for diagnostics and test failure messages.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::BaseColor => "base_color",
            Self::Normal => "normal",
            Self::Orm => "orm",
            Self::Emissive => "emissive",
        }
    }
}

/// One material's four maps, resolved: every slot names a real texture.
///
/// A named field per map rather than an array, because a renderer that has to know what
/// `texture[2]` means is one silent reordering away from shading roughness with an emissive
/// map. The array form exists only where the ABI genuinely needs it —
/// [`Self::in_descriptor_order`] — and it is built from the names rather than the names
/// being read out of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaterialTextures {
    /// `t0`. sRGB.
    pub base_color: TextureHandle,
    /// `t1`. Linear tangent-space normals.
    pub normal: TextureHandle,
    /// `t2`. Occlusion, roughness, metallic. Linear.
    pub orm: TextureHandle,
    /// `t3`. sRGB.
    pub emissive: TextureHandle,
}

impl MaterialTextures {
    /// The four handles in descriptor order, for writing the table.
    ///
    /// The one place the semantic set becomes positional, so the ABI has exactly one
    /// definition instead of one per call site.
    #[must_use]
    pub fn in_descriptor_order(&self) -> [TextureHandle; MATERIAL_TEXTURE_COUNT as usize] {
        [self.base_color, self.normal, self.orm, self.emissive]
    }

    /// The handle bound to `slot`.
    #[must_use]
    pub fn slot(&self, slot: MaterialTextureSlot) -> TextureHandle {
        match slot {
            MaterialTextureSlot::BaseColor => self.base_color,
            MaterialTextureSlot::Normal => self.normal,
            MaterialTextureSlot::Orm => self.orm,
            MaterialTextureSlot::Emissive => self.emissive,
        }
    }

    /// A set where every slot is the same texture. What the shared fallback set is built
    /// from before each slot is replaced by its own neutral texture.
    #[must_use]
    pub fn uniform(texture: TextureHandle) -> Self {
        Self {
            base_color: texture,
            normal: texture,
            orm: texture,
            emissive: texture,
        }
    }
}

/// What a caller asks for: any subset of the four maps.
///
/// `None` is "this material has no such map", not an error, and resolves to the shared
/// fallback for that slot. Every descriptor in a material's range is always valid — an
/// unbound one is undefined behaviour on read, and the alternative to a fallback would be
/// a shader permutation per absent map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MaterialTextureSet {
    /// sRGB base colour, multiplied with [`MaterialDesc::base_color`].
    pub base_color: Option<TextureHandle>,
    /// Tangent-space normals.
    pub normal: Option<TextureHandle>,
    /// Occlusion (R), roughness (G), metallic (B).
    pub orm: Option<TextureHandle>,
    /// Emissive colour.
    pub emissive: Option<TextureHandle>,
}

impl MaterialTextureSet {
    /// A set with only a base colour map.
    #[must_use]
    pub fn base_color(texture: TextureHandle) -> Self {
        Self {
            base_color: Some(texture),
            ..Self::default()
        }
    }

    /// Fills every empty slot from `fallbacks`.
    #[must_use]
    pub fn resolve(self, fallbacks: &MaterialTextures) -> MaterialTextures {
        MaterialTextures {
            base_color: self.base_color.unwrap_or(fallbacks.base_color),
            normal: self.normal.unwrap_or(fallbacks.normal),
            orm: self.orm.unwrap_or(fallbacks.orm),
            emissive: self.emissive.unwrap_or(fallbacks.emissive),
        }
    }

    /// Every handle the caller actually named, with its slot, for range checking.
    pub fn named(&self) -> impl Iterator<Item = (MaterialTextureSlot, TextureHandle)> + '_ {
        [
            (MaterialTextureSlot::BaseColor, self.base_color),
            (MaterialTextureSlot::Normal, self.normal),
            (MaterialTextureSlot::Orm, self.orm),
            (MaterialTextureSlot::Emissive, self.emissive),
        ]
        .into_iter()
        .filter_map(|(slot, handle)| handle.map(|handle| (slot, handle)))
    }
}

/// The material constant buffer, exactly as the shader declares it.
///
/// `repr(C)` and the explicit padding are load-bearing: HLSL packs constant buffers into
/// 16-byte registers and will not straddle one, so `emissive` starts a new register whether
/// or not this struct says so. Writing the padding out means the Rust layout and the HLSL
/// layout are the same thing rather than two things that happen to agree today.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MaterialConstants {
    /// Linear RGBA base colour.
    pub base_color: [f32; 4],
    /// Emissive colour.
    pub emissive: [f32; 3],
    /// Surface roughness. Shares a register with `emissive`.
    pub roughness: f32,
    /// Metalness.
    pub metallic: f32,
    /// Below this alpha a fragment is discarded. Zero is opaque.
    pub alpha_cutoff: f32,
    /// glTF's `normalTexture.scale`.
    ///
    /// In what used to be the first padding float, so the buffer's layout and its size are
    /// exactly what they were — the shader's `cbuffer` changed two names, not one offset.
    pub normal_scale: f32,
    /// glTF's `occlusionTexture.strength`, in what used to be the second padding float.
    pub occlusion_strength: f32,
}

impl From<MaterialDesc> for MaterialConstants {
    fn from(desc: MaterialDesc) -> Self {
        Self {
            base_color: [
                desc.base_color.x,
                desc.base_color.y,
                desc.base_color.z,
                desc.base_color.w,
            ],
            emissive: [desc.emissive.x, desc.emissive.y, desc.emissive.z],
            roughness: desc.roughness,
            metallic: desc.metallic,
            alpha_cutoff: desc.alpha_cutoff,
            normal_scale: desc.normal_scale,
            occlusion_strength: desc.occlusion_strength,
        }
    }
}

/// A material with its constants resident on the GPU.
pub struct Material {
    desc: MaterialDesc,
    constants: GpuBuffer,
}

impl Material {
    /// Uploads a material's constants and waits for the copy.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] for out-of-range or non-finite parameters,
    /// or [`RendererError::Api`] if a D3D12 call fails.
    pub fn new(device: &RenderDevice, desc: MaterialDesc) -> Result<Self, RendererError> {
        let mut uploader = StagingUploader::new(device)?;
        let material = Self::queue(device, &mut uploader, desc)?;
        uploader.flush(device)?;
        Ok(material)
    }

    /// Uploads several materials with a single GPU synchronization.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] for out-of-range or non-finite parameters,
    /// or [`RendererError::Api`] if a D3D12 call fails.
    pub fn new_many(
        device: &RenderDevice,
        descriptions: &[MaterialDesc],
    ) -> Result<Vec<Self>, RendererError> {
        let mut uploader = StagingUploader::new(device)?;
        let mut materials = Vec::with_capacity(descriptions.len());
        for desc in descriptions {
            materials.push(Self::queue(device, &mut uploader, *desc)?);
        }
        uploader.flush(device)?;
        Ok(materials)
    }

    /// Creates the buffer and queues its copy, without flushing.
    fn queue(
        device: &RenderDevice,
        uploader: &mut StagingUploader,
        desc: MaterialDesc,
    ) -> Result<Self, RendererError> {
        desc.validate()?;

        // A constant buffer's *size* must be a 256-byte multiple, not just its address.
        // The struct is 48 bytes; the buffer is 256, and the slack is never read.
        let constants = GpuBuffer::new_default(device, CONSTANT_BUFFER_ALIGNMENT)?;
        let mut padded = [0_u8; CONSTANT_BUFFER_ALIGNMENT];
        let value = MaterialConstants::from(desc);
        // SAFETY: `MaterialConstants` is `repr(C)` plain data with no padding bytes that
        // matter and no pointers, so its bytes are a valid `[u8; size_of::<T>()]`. The
        // destination is larger than the source.
        let bytes = unsafe {
            std::slice::from_raw_parts(
                std::ptr::from_ref(&value).cast::<u8>(),
                size_of::<MaterialConstants>(),
            )
        };
        padded[..bytes.len()].copy_from_slice(bytes);

        uploader.queue_buffer(
            device,
            &constants,
            &padded,
            D3D12_RESOURCE_STATE_VERTEX_AND_CONSTANT_BUFFER,
        )?;

        Ok(Self { desc, constants })
    }

    /// The parameters this material was built from.
    #[must_use]
    pub fn desc(&self) -> MaterialDesc {
        self.desc
    }

    /// The constant buffer's GPU address, for a root constant-buffer view.
    ///
    /// A root CBV rather than a descriptor table: with no textures there is nothing else in
    /// the table, and a root CBV costs one address in the root signature instead of a
    /// descriptor allocation and a table bind per draw. When a material gains a texture,
    /// this becomes a table into the persistent descriptor region that Milestone 4 built
    /// for exactly that.
    #[must_use]
    pub fn gpu_address(&self) -> u64 {
        self.constants.gpu_address()
    }

    /// The constant buffer, for diagnostics and for tests that read it back.
    #[must_use]
    pub fn buffer(&self) -> &GpuBuffer {
        &self.constants
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_material_is_a_plain_grey_dielectric() {
        let desc = MaterialDesc::default();
        assert_eq!(desc.metallic, 0.0);
        assert_eq!(desc.emissive, Vec3::ZERO);
        assert_eq!(desc.base_color.w, 1.0);
    }

    #[test]
    fn builders_clamp_rather_than_produce_unusable_values() {
        // A roughness of 2 is not a stylistic choice, it is a typo, and the lighting model
        // has no meaning outside [0, 1].
        assert_eq!(MaterialDesc::default().with_roughness(2.0).roughness, 1.0);
        assert_eq!(MaterialDesc::default().with_metallic(-1.0).metallic, 0.0);
    }

    #[test]
    fn out_of_range_parameters_are_rejected() {
        let too_rough = MaterialDesc {
            roughness: 1.5,
            ..MaterialDesc::default()
        };
        assert!(too_rough.validate().is_err());

        let negative_metal = MaterialDesc {
            metallic: -0.5,
            ..MaterialDesc::default()
        };
        assert!(negative_metal.validate().is_err());
    }

    #[test]
    fn non_finite_parameters_are_rejected() {
        // NaN propagates through the whole lighting expression and produces a black or
        // white pixel with nothing pointing at where it came from.
        let mut nan_color = MaterialDesc::default();
        nan_color.base_color.x = f32::NAN;
        assert!(nan_color.validate().is_err());

        let mut infinite_emissive = MaterialDesc::default();
        infinite_emissive.emissive.y = f32::INFINITY;
        assert!(infinite_emissive.validate().is_err());
    }

    #[test]
    fn the_constant_layout_matches_the_shader_registers() {
        // HLSL packs constants into 16-byte registers and never straddles one, so these
        // offsets are the shader's layout as much as Rust's.
        assert_eq!(size_of::<MaterialConstants>(), 48);
        assert_eq!(std::mem::offset_of!(MaterialConstants, base_color), 0);
        assert_eq!(std::mem::offset_of!(MaterialConstants, emissive), 16);
        assert_eq!(std::mem::offset_of!(MaterialConstants, roughness), 28);
        assert_eq!(std::mem::offset_of!(MaterialConstants, metallic), 32);
        assert!(size_of::<MaterialConstants>() <= CONSTANT_BUFFER_ALIGNMENT);
    }

    #[test]
    fn a_description_converts_field_for_field() {
        let desc = MaterialDesc::colored(0.1, 0.2, 0.3)
            .with_roughness(0.25)
            .with_metallic(0.75)
            .with_emissive(Vec3::new(1.0, 2.0, 3.0));
        let constants = MaterialConstants::from(desc);
        assert_eq!(constants.base_color, [0.1, 0.2, 0.3, 1.0]);
        assert_eq!(constants.emissive, [1.0, 2.0, 3.0]);
        assert_eq!(constants.roughness, 0.25);
        assert_eq!(constants.metallic, 0.75);
    }
}
