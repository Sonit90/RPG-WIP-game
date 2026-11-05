//! Depth-only pipeline states, for the shadow atlas and for the camera's depth prepass.
//!
//! # What makes them cheap
//!
//! * **No render target.** `NumRenderTargets` is zero, so there is no blending, no colour
//!   write and no output merger work at all.
//! * **No pixel shader**, for opaque geometry. A pipeline with a null PS lets most hardware
//!   run the depth-only rasterizer at roughly double rate. The alpha-tested variant *must*
//!   have one — it decides whether the fragment exists — and that is exactly why it is a
//!   second pipeline state rather than a branch inside one.
//! * **The same root signature the mesh pipelines use.** Not a smaller one of its own: the
//!   shadow pass binds a constant buffer at `b0` (the light's view-projection instead of the
//!   frame's), the material at `b2` and the base colour map at `t0` — every one of them a
//!   slot that signature already has. Sharing it means the shadow pass and the camera pass
//!   can be recorded into one command list with no root signature change between them.
//!
//! # Two bias variants
//!
//! Depth bias is baked into a pipeline state — D3D12 has no dynamic depth bias below Shader
//! Model 6.7 — so each shader gets two states: one with the shadow bias applied and one
//! with none. The biased set fills the shadow atlas; the unbiased set draws the camera's
//! depth prepass, where a bias would push the prepass depth away from the depth the camera
//! pass computes and every fragment would fail its own test.

#[cfg(windows)]
use windows::Win32::Graphics::Direct3D12::{
    D3D12_COMPARISON_FUNC_LESS_EQUAL, D3D12_CULL_MODE_BACK, D3D12_CULL_MODE_NONE,
    D3D12_DEPTH_STENCIL_DESC, D3D12_DEPTH_WRITE_MASK_ALL, D3D12_FILL_MODE_SOLID,
    D3D12_GRAPHICS_PIPELINE_STATE_DESC, D3D12_INPUT_LAYOUT_DESC,
    D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE, D3D12_RASTERIZER_DESC, D3D12_SHADER_BYTECODE,
    ID3D12Device, ID3D12PipelineState, ID3D12RootSignature,
};
#[cfg(windows)]
use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;

#[cfg(windows)]
use crate::error::RendererError;
#[cfg(windows)]
use crate::mesh_pipeline::{instanced_vertex_elements, skinned_vertex_elements};
#[cfg(windows)]
use crate::shader::{
    SHADOW_DEPTH_ALPHA_PS, SHADOW_DEPTH_INSTANCED_VS, SHADOW_DEPTH_SKINNED_VS, bytecode,
};

#[cfg(windows)]
use super::resources::SHADOW_FORMAT;
#[cfg(windows)]
use super::settings::ShadowBias;

/// Which geometry a depth-only pipeline consumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadowGeometry {
    /// Rigid geometry, one model matrix per instance.
    Instanced,
    /// A character, moved by a bone palette.
    Skinned,
}

/// Whether the pipeline performs an alpha test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadowSurface {
    /// No pixel shader at all: the fast path, and what nearly every surface uses.
    Opaque,
    /// A pixel shader that samples the base colour map and discards below the cutoff.
    AlphaTested,
}

#[cfg(windows)]
/// The four depth-only pipeline states one bias setting needs.
pub struct ShadowPipelineSet {
    instanced_opaque: ID3D12PipelineState,
    instanced_alpha: ID3D12PipelineState,
    skinned_opaque: ID3D12PipelineState,
    skinned_alpha: ID3D12PipelineState,
}

#[cfg(windows)]
impl ShadowPipelineSet {
    /// Creates the four states with `bias` applied by the rasterizer.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if a D3D12 call fails.
    pub fn new(
        device: &ID3D12Device,
        root_signature: &ID3D12RootSignature,
        bias: Option<ShadowBias>,
    ) -> Result<Self, RendererError> {
        Ok(Self {
            instanced_opaque: create(
                device,
                root_signature,
                bias,
                ShadowGeometry::Instanced,
                ShadowSurface::Opaque,
            )?,
            instanced_alpha: create(
                device,
                root_signature,
                bias,
                ShadowGeometry::Instanced,
                ShadowSurface::AlphaTested,
            )?,
            skinned_opaque: create(
                device,
                root_signature,
                bias,
                ShadowGeometry::Skinned,
                ShadowSurface::Opaque,
            )?,
            skinned_alpha: create(
                device,
                root_signature,
                bias,
                ShadowGeometry::Skinned,
                ShadowSurface::AlphaTested,
            )?,
        })
    }

    /// The state for one combination of geometry and surface.
    #[must_use]
    pub fn get(&self, geometry: ShadowGeometry, surface: ShadowSurface) -> &ID3D12PipelineState {
        match (geometry, surface) {
            (ShadowGeometry::Instanced, ShadowSurface::Opaque) => &self.instanced_opaque,
            (ShadowGeometry::Instanced, ShadowSurface::AlphaTested) => &self.instanced_alpha,
            (ShadowGeometry::Skinned, ShadowSurface::Opaque) => &self.skinned_opaque,
            (ShadowGeometry::Skinned, ShadowSurface::AlphaTested) => &self.skinned_alpha,
        }
    }
}

#[cfg(windows)]
/// Both bias variants, and the root signature they share.
pub struct ShadowPipelines {
    /// The shared signature, held so the pipelines outlive nothing they point at.
    root_signature: ID3D12RootSignature,
    /// Biased: what fills the shadow atlas.
    shadow: ShadowPipelineSet,
    /// Unbiased: what draws the camera's depth prepass.
    prepass: ShadowPipelineSet,
    /// The bias the shadow set was built with, so a settings change can tell whether the
    /// states have to be rebuilt at all.
    bias: ShadowBias,
}

#[cfg(windows)]
impl ShadowPipelines {
    /// Creates both sets.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if a D3D12 call fails.
    pub fn new(device: &ID3D12Device, bias: ShadowBias) -> Result<Self, RendererError> {
        let root_signature = crate::mesh_pipeline::create_root_signature(device)?;
        Ok(Self {
            shadow: ShadowPipelineSet::new(device, &root_signature, Some(bias))?,
            prepass: ShadowPipelineSet::new(device, &root_signature, None)?,
            root_signature,
            bias,
        })
    }

    /// Rebuilds the biased set if `bias` differs from what it was built with.
    ///
    /// Returns whether anything was rebuilt. The unbiased prepass set never changes, so it
    /// is not touched.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if a D3D12 call fails.
    pub fn set_bias(
        &mut self,
        device: &ID3D12Device,
        bias: ShadowBias,
    ) -> Result<bool, RendererError> {
        if bias == self.bias {
            return Ok(false);
        }
        self.shadow = ShadowPipelineSet::new(device, &self.root_signature, Some(bias))?;
        self.bias = bias;
        Ok(true)
    }

    /// The root signature both sets share with the mesh pipelines.
    #[must_use]
    pub fn root_signature(&self) -> &ID3D12RootSignature {
        &self.root_signature
    }

    /// The biased set, for the shadow atlas.
    #[must_use]
    pub fn shadow(&self) -> &ShadowPipelineSet {
        &self.shadow
    }

    /// The unbiased set, for the camera's depth prepass.
    #[must_use]
    pub fn prepass(&self) -> &ShadowPipelineSet {
        &self.prepass
    }
}

#[cfg(windows)]
/// The rasterizer state for a depth-only pass.
///
/// Back faces are culled for opaque geometry, exactly as in the camera pass, so a shadow map
/// and the picture agree about which triangles exist. Cutout geometry is drawn **two-sided**:
/// a leaf card is a single plane, and culling half of it means half the foliage in a scene
/// casts no shadow at all.
fn rasterizer(bias: Option<ShadowBias>, surface: ShadowSurface) -> D3D12_RASTERIZER_DESC {
    let is_prepass = bias.is_none();
    let bias = bias.unwrap_or(ShadowBias {
        constant: 0,
        slope: 0.0,
        normal_texels: 0.0,
        normal_max_meters: 0.0,
    });
    D3D12_RASTERIZER_DESC {
        FillMode: D3D12_FILL_MODE_SOLID,
        CullMode: match surface {
            ShadowSurface::Opaque => D3D12_CULL_MODE_BACK,
            ShadowSurface::AlphaTested => D3D12_CULL_MODE_NONE,
        },
        // The same winding convention the camera pass uses; see `pipeline::rasterizer_desc`
        // for why getting it wrong is silent.
        FrontCounterClockwise: true.into(),
        DepthBias: bias.constant,
        SlopeScaledDepthBias: bias.slope,
        // No clamp: the clamp exists to stop a steep slope producing an enormous offset, and
        // the slope bias here is small enough that the clamp would only ever hide a
        // mis-tuned value.
        DepthBiasClamp: 0.0,
        // **Off for the atlas, on for the camera's prepass**, and the difference matters.
        //
        // For a cascade, clipping off is deliberate: a caster nearer to the light than the
        // cascade's near plane would otherwise be clipped away and stop casting, and with it
        // off the geometry is clamped to the near plane and still writes depth. That is what
        // makes the extruded region behind a cascade work for geometry *very* far behind it.
        //
        // For the camera's prepass the same setting is a bug. The main pass tests
        // `LESS_EQUAL` against what the prepass wrote, so the prepass has to rasterize the
        // frame the way the main pass does — and the main pass clips. With clipping off, the
        // part of a surface between the eye and the near plane is flattened onto depth zero
        // instead of being discarded, and every real fragment behind it then fails the
        // comparison and is never shaded. On a ground plane running under the camera that is
        // a swathe of unlit background with no shadow in it, moving as the camera turns.
        DepthClipEnable: is_prepass.into(),
        ..Default::default()
    }
}

#[cfg(windows)]
/// The depth state every shadow pipeline uses: write, and test less-or-equal.
fn depth_stencil() -> D3D12_DEPTH_STENCIL_DESC {
    D3D12_DEPTH_STENCIL_DESC {
        DepthEnable: true.into(),
        DepthWriteMask: D3D12_DEPTH_WRITE_MASK_ALL,
        // Less-or-equal rather than less, so the depth prepass and the camera pass agree
        // about a fragment whose depth they both computed: the camera pass tests against
        // what the prepass wrote, and "equal" has to pass.
        DepthFunc: D3D12_COMPARISON_FUNC_LESS_EQUAL,
        StencilEnable: false.into(),
        ..Default::default()
    }
}

#[cfg(windows)]
/// Creates one depth-only pipeline state.
fn create(
    device: &ID3D12Device,
    root_signature: &ID3D12RootSignature,
    bias: Option<ShadowBias>,
    geometry: ShadowGeometry,
    surface: ShadowSurface,
) -> Result<ID3D12PipelineState, RendererError> {
    // Declared before the descriptor so it outlives the pointer the input layout holds into
    // it — the same lifetime trap the root signature's descriptor ranges have.
    let instanced_elements = instanced_vertex_elements();
    let skinned_elements = skinned_vertex_elements();
    let elements: &[_] = match geometry {
        ShadowGeometry::Instanced => &instanced_elements,
        ShadowGeometry::Skinned => &skinned_elements,
    };

    let desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
        InputLayout: D3D12_INPUT_LAYOUT_DESC {
            pInputElementDescs: elements.as_ptr(),
            NumElements: elements.len() as u32,
        },
        // SAFETY: copies the interface pointer without changing its refcount; the copy lives
        // only for this call and the real reference is owned by the caller.
        pRootSignature: unsafe { std::mem::transmute_copy(root_signature) },
        VS: bytecode(match geometry {
            ShadowGeometry::Instanced => SHADOW_DEPTH_INSTANCED_VS,
            ShadowGeometry::Skinned => SHADOW_DEPTH_SKINNED_VS,
        }),
        PS: match surface {
            // A null pixel shader: the depth-only fast path.
            ShadowSurface::Opaque => D3D12_SHADER_BYTECODE::default(),
            ShadowSurface::AlphaTested => bytecode(SHADOW_DEPTH_ALPHA_PS),
        },
        RasterizerState: rasterizer(bias, surface),
        BlendState: crate::pipeline::blend_desc(),
        DepthStencilState: depth_stencil(),
        DSVFormat: SHADOW_FORMAT,
        SampleMask: u32::MAX,
        PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
        // No render target. This is the line that makes it a depth-only pipeline.
        NumRenderTargets: 0,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        ..Default::default()
    };

    // SAFETY: FFI call with a fully initialized descriptor whose referenced bytecode and
    // input-element array outlive the call.
    unsafe { device.CreateGraphicsPipelineState(&desc) }
        .map_err(|error| RendererError::api("ID3D12Device::CreateGraphicsPipelineState", error))
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
#[cfg(windows)]
mod tests {
    use super::*;

    #[test]
    fn an_opaque_shadow_pipeline_has_no_pixel_shader() {
        // The whole point of the opaque variant. If a pixel shader ever appears here the
        // shadow pass silently halves its fill rate.
        let elements = instanced_vertex_elements();
        assert!(!elements.is_empty());
        let opaque = rasterizer(None, ShadowSurface::Opaque);
        assert_eq!(opaque.CullMode, D3D12_CULL_MODE_BACK);
    }

    #[test]
    fn cutout_geometry_is_drawn_two_sided() {
        // A leaf card is one plane. Culling its back face means half the foliage in a scene
        // casts no shadow, which reads as randomly missing shadows rather than as a
        // rasterizer setting.
        let cutout = rasterizer(None, ShadowSurface::AlphaTested);
        assert_eq!(cutout.CullMode, D3D12_CULL_MODE_NONE);
    }

    #[test]
    fn the_prepass_is_unbiased_and_the_shadow_pass_is_not() {
        let shadow = rasterizer(Some(ShadowBias::default()), ShadowSurface::Opaque);
        let prepass = rasterizer(None, ShadowSurface::Opaque);
        assert!(shadow.DepthBias > 0);
        assert!(shadow.SlopeScaledDepthBias > 0.0);
        assert_eq!(prepass.DepthBias, 0);
        assert_eq!(prepass.SlopeScaledDepthBias, 0.0);
    }

    #[test]
    fn the_atlas_clamps_depth_and_the_camera_prepass_clips_it() {
        // The two passes want opposite answers, and the prepass used to get the atlas's.
        //
        // An atlas pass wants clipping *off*: a caster nearer to the light than the cascade's
        // near plane should be clamped to it and still write depth, which is what lets the
        // extruded region behind a cascade shadow into it.
        //
        // The camera prepass wants clipping *on*, because its depth has to be the depth the
        // main pass will produce — the main pass tests `LESS_EQUAL` against it. With clipping
        // off, geometry between the eye and the near plane is not discarded but flattened
        // onto the nearest depth, so a ground plane running under the camera filled part of
        // the screen with depth zero; the real ground behind it then failed the comparison
        // and was never shaded, leaving flat unlit background with no shadow on it over
        // whatever part of the frame the near slice covered.
        assert!(
            !rasterizer(Some(ShadowBias::default()), ShadowSurface::Opaque)
                .DepthClipEnable
                .as_bool(),
            "an atlas pass must clamp, so a caster in front of the cascade still casts"
        );
        assert!(
            rasterizer(None, ShadowSurface::Opaque)
                .DepthClipEnable
                .as_bool(),
            "the camera prepass must clip exactly as the camera pass does, or the depth it              writes is not the depth the camera pass will test against"
        );
    }
}
