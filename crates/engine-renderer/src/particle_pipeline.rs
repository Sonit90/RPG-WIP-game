//! The blended particle pass: root signature, two pipeline states, and its constants.
//!
//! ```text
//! opaque scene  ->  particles (additive, then alpha)  ->  overlay  ->  UI
//!                   ^^^^^^^^^ this file
//! ```
//!
//! # Where it sits and why
//!
//! After the opaque scene, because a blended surface has to see what is behind it. Before
//! the interface, because a health bar drawn under a fireball is a health bar the player
//! cannot read.
//!
//! # Depth: tested, not written
//!
//! Particles read the scene's depth buffer, so a spark behind a pillar is hidden by the
//! pillar. They do not write to it, because a particle is not an occluder: writing depth
//! would make the first quad of a burst hide the twenty behind it, and the burst would look
//! like one flat card.
//!
//! # Two pipeline states, one shader
//!
//! The pixel shader outputs **premultiplied** alpha, so the two modes differ only in the
//! blend descriptor: additive is `ONE, ONE`, alpha is `ONE, INV_SRC_ALPHA`. D3D12 bakes the
//! blend state into the PSO, so this is two objects created at startup and one `SetPipeline`
//! between the two draws — not a state change per particle.
//!
//! # No culling
//!
//! The quads are built to face the camera, and a quad rotated about the view axis can end
//! up wound either way. Back-face culling would drop half the particles at random angles.

use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D12::{
    D3D_ROOT_SIGNATURE_VERSION_1, D3D12_BLEND_DESC, D3D12_BLEND_INV_SRC_ALPHA, D3D12_BLEND_ONE,
    D3D12_BLEND_OP_ADD, D3D12_BLEND_ZERO, D3D12_COLOR_WRITE_ENABLE_ALL,
    D3D12_COMPARISON_FUNC_LESS_EQUAL, D3D12_CONSERVATIVE_RASTERIZATION_MODE_OFF,
    D3D12_CULL_MODE_NONE, D3D12_DEPTH_STENCIL_DESC, D3D12_DEPTH_WRITE_MASK_ZERO,
    D3D12_FILL_MODE_SOLID, D3D12_GRAPHICS_PIPELINE_STATE_DESC,
    D3D12_INPUT_CLASSIFICATION_PER_INSTANCE_DATA, D3D12_INPUT_ELEMENT_DESC,
    D3D12_INPUT_LAYOUT_DESC, D3D12_LOGIC_OP_NOOP, D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
    D3D12_RASTERIZER_DESC, D3D12_RENDER_TARGET_BLEND_DESC, D3D12_ROOT_DESCRIPTOR,
    D3D12_ROOT_PARAMETER, D3D12_ROOT_PARAMETER_0, D3D12_ROOT_PARAMETER_TYPE_CBV,
    D3D12_ROOT_SIGNATURE_DESC, D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT,
    D3D12_SHADER_VISIBILITY_VERTEX, D3D12SerializeRootSignature, ID3D12Device, ID3D12PipelineState,
    ID3D12RootSignature,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_R32_FLOAT, DXGI_FORMAT_R32G32B32_FLOAT, DXGI_FORMAT_R32G32B32A32_FLOAT,
    DXGI_SAMPLE_DESC,
};
use windows::core::s;

use engine_core::math::{Mat4, Vec3};
use engine_render_data::ParticleBlend;

use crate::depth::DEPTH_FORMAT;
use crate::error::RendererError;
use crate::shader::{PARTICLE_PS, PARTICLE_VS, bytecode};
use crate::swapchain::BACK_BUFFER_FORMAT;

/// Root parameter slot of the particle frame constants, `b0` in `particle.hlsl`.
pub const ROOT_PARTICLE_FRAME_CBV: u32 = 0;

/// Per-frame constants for the particle pass, matching `cbuffer ParticleFrame`.
///
/// `repr(C)` and padded to whole 16-byte registers, exactly as the scene's are: HLSL packs
/// constants into registers and will not straddle one, so the padding is the shader's
/// layout as much as it is Rust's.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ParticleConstants {
    /// `projection * view`, column-major.
    pub view_projection: [f32; 16],
    /// The camera's right axis in world space.
    pub camera_right: [f32; 3],
    /// Padding to a whole register.
    pub padding: f32,
    /// The camera's up axis in world space.
    pub camera_up: [f32; 3],
    /// Padding to a whole register.
    pub padding_2: f32,
}

impl ParticleConstants {
    /// Builds the constants from a view-projection and the view matrix's basis.
    ///
    /// The right and up axes are the **rows** of the view matrix's upper-left 3×3, which is
    /// the inverse of its rotation — that is, the camera's axes expressed in world space.
    /// Taking the columns instead gives the world axes expressed in view space, and every
    /// particle would be billboarded against a basis that rotates the wrong way.
    #[must_use]
    pub fn new(view: &Mat4, view_projection: &Mat4) -> Self {
        let right = Vec3::new(view.columns[0].x, view.columns[1].x, view.columns[2].x);
        let up = Vec3::new(view.columns[0].y, view.columns[1].y, view.columns[2].y);
        Self {
            view_projection: view_projection.to_cols_array(),
            camera_right: [right.x, right.y, right.z],
            padding: 0.0,
            camera_up: [up.x, up.y, up.z],
            padding_2: 0.0,
        }
    }
}

/// Root signature and the two blend-mode pipeline states for the particle pass.
pub struct ParticlePipeline {
    root_signature: ID3D12RootSignature,
    additive: ID3D12PipelineState,
    alpha: ID3D12PipelineState,
}

impl ParticlePipeline {
    /// Creates the signature and both pipeline states from the embedded DXIL.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if a D3D12 call fails.
    pub fn new(device: &ID3D12Device) -> Result<Self, RendererError> {
        let root_signature = create_root_signature(device)?;
        let additive = create_state(device, &root_signature, additive_blend_desc())?;
        let alpha = create_state(device, &root_signature, alpha_blend_desc())?;
        log::info!(
            "particle pipeline ready: premultiplied additive and alpha, shader model {}",
            crate::shader::SHADER_MODEL
        );
        Ok(Self {
            root_signature,
            additive,
            alpha,
        })
    }

    /// The root signature to bind before drawing.
    #[must_use]
    pub fn root_signature(&self) -> &ID3D12RootSignature {
        &self.root_signature
    }

    /// The pipeline state for one blend mode.
    #[must_use]
    pub fn state(&self, blend: ParticleBlend) -> &ID3D12PipelineState {
        match blend {
            ParticleBlend::Additive => &self.additive,
            ParticleBlend::Alpha => &self.alpha,
        }
    }
}

/// One CBV, vertex-visible: the pass has no material, no texture and no per-object data.
fn create_root_signature(device: &ID3D12Device) -> Result<ID3D12RootSignature, RendererError> {
    let parameters = [D3D12_ROOT_PARAMETER {
        ParameterType: D3D12_ROOT_PARAMETER_TYPE_CBV,
        Anonymous: D3D12_ROOT_PARAMETER_0 {
            Descriptor: D3D12_ROOT_DESCRIPTOR {
                ShaderRegister: 0,
                RegisterSpace: 0,
            },
        },
        // The pixel shader reads nothing but its interpolants, so the constants never have
        // to be visible to it.
        ShaderVisibility: D3D12_SHADER_VISIBILITY_VERTEX,
    }];

    let desc = D3D12_ROOT_SIGNATURE_DESC {
        NumParameters: parameters.len() as u32,
        pParameters: parameters.as_ptr(),
        NumStaticSamplers: 0,
        pStaticSamplers: std::ptr::null(),
        Flags: D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT,
    };

    let mut blob: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;
    // SAFETY: FFI call. `desc` and its parameter array outlive the call, and both
    // out-parameters are valid.
    let result = unsafe {
        D3D12SerializeRootSignature(
            &desc,
            D3D_ROOT_SIGNATURE_VERSION_1,
            &mut blob,
            Some(&mut errors),
        )
    };
    if let Err(error) = result {
        return Err(RendererError::api(
            "D3D12SerializeRootSignature (particles)",
            error,
        ));
    }
    let Some(blob) = blob else {
        return Err(RendererError::InvalidConfig {
            reason: "particle root signature serialization produced no blob".to_owned(),
        });
    };

    // SAFETY: FFI calls reading the serialized blob's buffer, which stays alive for the
    // duration of the CreateRootSignature call below.
    let bytes = unsafe {
        std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize())
    };
    // SAFETY: FFI call. `bytes` points into the live blob; node mask 0 is the single adapter.
    unsafe { device.CreateRootSignature(0, bytes) }
        .map_err(|error| RendererError::api("CreateRootSignature (particles)", error))
}

/// Creates one pipeline state with the given blend descriptor.
fn create_state(
    device: &ID3D12Device,
    root_signature: &ID3D12RootSignature,
    blend: D3D12_BLEND_DESC,
) -> Result<ID3D12PipelineState, RendererError> {
    let elements = input_elements();
    let mut desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
        InputLayout: D3D12_INPUT_LAYOUT_DESC {
            pInputElementDescs: elements.as_ptr(),
            NumElements: elements.len() as u32,
        },
        // SAFETY: copies the interface pointer without changing its refcount; the copy lives
        // only for this call and the real reference is owned by the caller.
        pRootSignature: unsafe { std::mem::transmute_copy(root_signature) },
        VS: bytecode(PARTICLE_VS),
        PS: bytecode(PARTICLE_PS),
        RasterizerState: rasterizer_desc(),
        BlendState: blend,
        DepthStencilState: depth_desc(),
        DSVFormat: DEPTH_FORMAT,
        SampleMask: u32::MAX,
        PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
        NumRenderTargets: 1,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        ..Default::default()
    };
    desc.RTVFormats[0] = BACK_BUFFER_FORMAT;

    // SAFETY: FFI call with a fully initialized descriptor whose referenced bytecode and
    // input-element array outlive it.
    unsafe {
        {
            let probe = crate::upload_timing::Probe::start();
            let state = device.CreateGraphicsPipelineState(&desc);
            probe.pipeline();
            state
        }
    }
    .map_err(|error| RendererError::api("CreateGraphicsPipelineState (particles)", error))
}

/// The per-instance input elements, matching `engine_render_data::ParticleInstance`.
///
/// Every element is per *instance*: there is no per-vertex stream at all, because the four
/// corners come from `SV_VertexID`.
#[must_use]
pub fn input_elements() -> [D3D12_INPUT_ELEMENT_DESC; 4] {
    debug_assert_eq!(
        size_of::<engine_render_data::ParticleInstance>(),
        36,
        "the offsets below describe a 36-byte particle record"
    );
    [
        D3D12_INPUT_ELEMENT_DESC {
            SemanticName: s!("POSITION"),
            SemanticIndex: 0,
            Format: DXGI_FORMAT_R32G32B32_FLOAT,
            InputSlot: 0,
            AlignedByteOffset: 0,
            InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_INSTANCE_DATA,
            InstanceDataStepRate: 1,
        },
        D3D12_INPUT_ELEMENT_DESC {
            SemanticName: s!("TEXCOORD"),
            SemanticIndex: 0,
            Format: DXGI_FORMAT_R32_FLOAT,
            InputSlot: 0,
            AlignedByteOffset: 12,
            InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_INSTANCE_DATA,
            InstanceDataStepRate: 1,
        },
        D3D12_INPUT_ELEMENT_DESC {
            SemanticName: s!("COLOR"),
            SemanticIndex: 0,
            Format: DXGI_FORMAT_R32G32B32A32_FLOAT,
            InputSlot: 0,
            AlignedByteOffset: 16,
            InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_INSTANCE_DATA,
            InstanceDataStepRate: 1,
        },
        D3D12_INPUT_ELEMENT_DESC {
            SemanticName: s!("TEXCOORD"),
            SemanticIndex: 1,
            Format: DXGI_FORMAT_R32_FLOAT,
            InputSlot: 0,
            AlignedByteOffset: 32,
            InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_INSTANCE_DATA,
            InstanceDataStepRate: 1,
        },
    ]
}

/// Solid fill, no culling: a camera-facing quad can be wound either way.
fn rasterizer_desc() -> D3D12_RASTERIZER_DESC {
    D3D12_RASTERIZER_DESC {
        FillMode: D3D12_FILL_MODE_SOLID,
        CullMode: D3D12_CULL_MODE_NONE,
        FrontCounterClockwise: false.into(),
        DepthClipEnable: true.into(),
        ConservativeRaster: D3D12_CONSERVATIVE_RASTERIZATION_MODE_OFF,
        ..Default::default()
    }
}

/// Depth tested against the scene, never written.
#[must_use]
pub fn depth_desc() -> D3D12_DEPTH_STENCIL_DESC {
    D3D12_DEPTH_STENCIL_DESC {
        DepthEnable: true.into(),
        // Zero: a particle is not an occluder. Writing depth would make the first quad of a
        // burst hide every quad behind it and the burst would read as one flat card.
        DepthWriteMask: D3D12_DEPTH_WRITE_MASK_ZERO,
        // LESS_EQUAL rather than LESS, so a particle sitting exactly on a surface — a
        // scorch on the floor — is not z-fought out of existence.
        DepthFunc: D3D12_COMPARISON_FUNC_LESS_EQUAL,
        StencilEnable: false.into(),
        ..Default::default()
    }
}

/// Premultiplied additive: `src + dst`. Fire, sparks, flashes.
#[must_use]
pub fn additive_blend_desc() -> D3D12_BLEND_DESC {
    blend_with(D3D12_BLEND_ONE)
}

/// Premultiplied alpha: `src + dst * (1 - a)`. Smoke.
#[must_use]
pub fn alpha_blend_desc() -> D3D12_BLEND_DESC {
    blend_with(D3D12_BLEND_INV_SRC_ALPHA)
}

/// The shared blend descriptor, differing only in what the destination is scaled by.
fn blend_with(destination: windows::Win32::Graphics::Direct3D12::D3D12_BLEND) -> D3D12_BLEND_DESC {
    let target = D3D12_RENDER_TARGET_BLEND_DESC {
        BlendEnable: true.into(),
        LogicOpEnable: false.into(),
        // ONE, because the pixel shader already multiplied the colour by its coverage.
        SrcBlend: D3D12_BLEND_ONE,
        DestBlend: destination,
        BlendOp: D3D12_BLEND_OP_ADD,
        SrcBlendAlpha: D3D12_BLEND_ONE,
        DestBlendAlpha: destination,
        BlendOpAlpha: D3D12_BLEND_OP_ADD,
        LogicOp: D3D12_LOGIC_OP_NOOP,
        RenderTargetWriteMask: D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8,
    };
    let mut desc = D3D12_BLEND_DESC {
        AlphaToCoverageEnable: false.into(),
        IndependentBlendEnable: false.into(),
        ..Default::default()
    };
    desc.RenderTarget[0] = target;
    // The unused targets stay at a disabled, no-op blend rather than at whatever `Default`
    // happens to zero to.
    for target in desc.RenderTarget.iter_mut().skip(1) {
        target.SrcBlend = D3D12_BLEND_ONE;
        target.DestBlend = D3D12_BLEND_ZERO;
        target.RenderTargetWriteMask = D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8;
    }
    desc
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::math::{Quat, Vec3};
    use engine_core::transform::Transform;

    #[test]
    fn the_constants_are_the_registers_the_shader_declares() {
        assert_eq!(size_of::<ParticleConstants>(), 96);
        assert_eq!(std::mem::offset_of!(ParticleConstants, view_projection), 0);
        assert_eq!(std::mem::offset_of!(ParticleConstants, camera_right), 64);
        assert_eq!(std::mem::offset_of!(ParticleConstants, camera_up), 80);
    }

    #[test]
    fn the_basis_is_the_cameras_axes_in_world_space() {
        // A camera turned a quarter turn about +Y looks down -X, so its right axis points
        // along -Z. Taking the view matrix's columns instead of its rows gives the opposite
        // sense, and every particle would be billboarded against a basis that turns the
        // wrong way — which looks like the whole effect system is mirrored.
        let rotation = Quat::from_yaw_pitch_roll(std::f32::consts::FRAC_PI_2, 0.0, 0.0);
        let camera_to_world = Transform::from_rotation(rotation).matrix();
        let Some(view) = camera_to_world.inverse() else {
            panic!("a rotation matrix is invertible");
        };
        let constants = ParticleConstants::new(&view, &view);

        let right = Vec3::new(
            constants.camera_right[0],
            constants.camera_right[1],
            constants.camera_right[2],
        );
        assert!((right.z + 1.0).abs() < 1e-4, "{right:?}");

        let up = Vec3::new(
            constants.camera_up[0],
            constants.camera_up[1],
            constants.camera_up[2],
        );
        assert!((up.y - 1.0).abs() < 1e-4, "{up:?}");
    }

    #[test]
    fn the_two_modes_differ_only_in_the_destination_factor() {
        let additive = additive_blend_desc().RenderTarget[0];
        let alpha = alpha_blend_desc().RenderTarget[0];
        assert!(additive.BlendEnable.as_bool() && alpha.BlendEnable.as_bool());
        // ONE on the source is what "the shader premultiplied it" means; if this ever
        // becomes SRC_ALPHA the falloff is applied twice and every effect goes dim.
        assert_eq!(additive.SrcBlend, D3D12_BLEND_ONE);
        assert_eq!(alpha.SrcBlend, D3D12_BLEND_ONE);
        assert_eq!(additive.DestBlend, D3D12_BLEND_ONE);
        assert_eq!(alpha.DestBlend, D3D12_BLEND_INV_SRC_ALPHA);
    }

    #[test]
    fn depth_is_tested_and_never_written() {
        let depth = depth_desc();
        assert!(
            depth.DepthEnable.as_bool(),
            "particles must hide behind walls"
        );
        assert_eq!(
            depth.DepthWriteMask, D3D12_DEPTH_WRITE_MASK_ZERO,
            "a particle that writes depth hides the burst behind it"
        );
        assert_eq!(depth.DepthFunc, D3D12_COMPARISON_FUNC_LESS_EQUAL);
    }

    #[test]
    fn every_element_steps_once_per_particle() {
        // A rate of zero here would make every particle in the draw read the first record,
        // which renders the whole burst stacked at one point.
        for element in input_elements() {
            assert_eq!(
                element.InputSlotClass,
                D3D12_INPUT_CLASSIFICATION_PER_INSTANCE_DATA
            );
            assert_eq!(element.InstanceDataStepRate, 1);
            assert_eq!(element.InputSlot, 0);
        }
    }

    #[test]
    fn the_quads_are_not_back_face_culled() {
        // A quad rotated about the view axis can end up wound either way, and culling would
        // drop half the particles at random angles.
        assert_eq!(rasterizer_desc().CullMode, D3D12_CULL_MODE_NONE);
    }
}
