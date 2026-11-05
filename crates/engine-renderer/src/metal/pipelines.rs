//! Render pipeline states and depth-stencil states.
//!
//! Every state is reproduced from the D3D12 descriptors (`pipeline.rs`, `mesh_pipeline.rs`,
//! `particle_pipeline.rs`, `ui_pipeline.rs`) and cached by what distinguishes it, created the
//! first time a frame needs it:
//!
//! ```text
//! mesh / instanced / skinned   (MeshKind, PipelineVariant)  -> state + depth rule + fill
//! particles                    additive, alpha              -> particles.rs
//! UI                           one                          -> ui.rs
//! debug overlay and triangle   one                          -> TrianglePipeline below
//! ```
//!
//! What D3D12 bakes into a PSO and Metal keeps on the encoder (cull mode, winding, fill mode,
//! depth-stencil state) travels with the pipeline here as a [`Raster`] and a [`DepthRule`], so
//! the recorder sets them together with the state and a draw cannot get one without the other.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{
    MTLBlendFactor, MTLBlendOperation, MTLColorWriteMask, MTLCompareFunction, MTLCullMode,
    MTLDepthStencilDescriptor, MTLDevice, MTLPixelFormat, MTLRenderPipelineDescriptor,
    MTLRenderPipelineState, MTLTriangleFillMode,
};

use super::layer::COLOR_FORMAT;
use super::library::{Function, Libraries};
use super::{DepthState, Device, api_error, describe};
use crate::error::RendererError;
use crate::material::SurfaceClass;
use crate::mesh_pipeline::{CharacterPass, DepthMode, PipelineVariant};

/// The depth buffer's format, shared by every pipeline that draws into the camera pass.
pub(crate) const DEPTH_FORMAT: MTLPixelFormat = MTLPixelFormat::Depth32Float;

/// A compiled render pipeline state.
pub(crate) type PipelineState = Retained<ProtocolObject<dyn MTLRenderPipelineState>>;

/// How a pipeline writes colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Blend {
    /// No blending; every channel is written.
    Opaque,
    /// Nothing is written: the first of a translucent character's two passes.
    NoColor,
    /// Straight source alpha, `src.rgb * a + dst.rgb * (1 - a)`; alpha accumulates coverage.
    /// The transparent mesh phase and the UI.
    Straight,
    /// Premultiplied, `src + dst`. Fire and sparks.
    Additive,
    /// Premultiplied, `src + dst * (1 - a)`. Smoke.
    Premultiplied,
}

/// Which depth-stencil state a draw uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DepthRule {
    /// `LESS_EQUAL`, writing depth. Opaque surfaces.
    Write,
    /// `LESS_EQUAL`, writing nothing. Transparent surfaces, particles, and surfaces a depth
    /// prepass already drew.
    Read,
    /// No test and no write: the UI.
    Off,
}

/// The rasterizer state a pipeline draws with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Raster {
    /// Back faces are culled. Off for wireframe, particles and the UI.
    pub(crate) cull_back: bool,
    /// Edges only.
    pub(crate) wireframe: bool,
}

impl Raster {
    /// Solid, back-face culled. Front faces are counter-clockwise, as the D3D12 rasterizer
    /// declares (`FrontCounterClockwise`): both APIs judge winding in the same top-left,
    /// y-down framebuffer.
    pub(crate) const SOLID: Self = Self {
        cull_back: true,
        wireframe: false,
    };
    /// Solid and double-sided.
    pub(crate) const DOUBLE_SIDED: Self = Self {
        cull_back: false,
        wireframe: false,
    };
    /// Wireframe, with no culling: the point is to see every edge.
    pub(crate) const WIREFRAME: Self = Self {
        cull_back: false,
        wireframe: true,
    };

    /// The Metal cull mode.
    pub(crate) fn cull_mode(self) -> MTLCullMode {
        if self.cull_back {
            MTLCullMode::Back
        } else {
            MTLCullMode::None
        }
    }

    /// The Metal fill mode.
    pub(crate) fn fill_mode(self) -> MTLTriangleFillMode {
        if self.wireframe {
            MTLTriangleFillMode::Lines
        } else {
            MTLTriangleFillMode::Fill
        }
    }
}

/// A pipeline state with the encoder state it must be drawn with.
pub(crate) struct Pipeline {
    pub(crate) state: PipelineState,
    pub(crate) depth: DepthRule,
    pub(crate) raster: Raster,
}

/// The three depth-stencil states, created once.
pub(crate) struct DepthStates {
    write: DepthState,
    read: DepthState,
    off: DepthState,
}

impl DepthStates {
    /// Creates the states.
    ///
    /// # Errors
    ///
    /// [`RendererError::Api`] if the device refuses one.
    pub(crate) fn new(device: &Device) -> Result<Self, RendererError> {
        let make = |compare: MTLCompareFunction, write: bool| {
            let descriptor = MTLDepthStencilDescriptor::new();
            descriptor.setDepthCompareFunction(compare);
            descriptor.setDepthWriteEnabled(write);
            device
                .newDepthStencilStateWithDescriptor(&descriptor)
                .ok_or_else(|| {
                    api_error(
                        "MTLDevice::newDepthStencilStateWithDescriptor",
                        "the device returned no depth-stencil state",
                    )
                })
        };
        Ok(Self {
            // LESS_EQUAL and not LESS: a prepass writes exactly the depth the camera pass
            // computes, so every fragment arrives equal. See `pipeline::depth_stencil_desc`.
            write: make(MTLCompareFunction::LessEqual, true)?,
            read: make(MTLCompareFunction::LessEqual, false)?,
            off: make(MTLCompareFunction::Always, false)?,
        })
    }

    /// The state for `rule`.
    pub(crate) fn get(&self, rule: DepthRule) -> &DepthState {
        match rule {
            DepthRule::Write => &self.write,
            DepthRule::Read => &self.read,
            DepthRule::Off => &self.off,
        }
    }
}

/// Builds a pipeline state drawing into the camera pass: one BGRA colour target and the depth
/// buffer, even when the pipeline tests no depth, because Metal requires a pipeline's
/// attachment formats to equal the render pass's.
///
/// # Errors
///
/// [`RendererError::Api`] with the compiler's message if the state is rejected.
pub(crate) fn build_state(
    device: &Device,
    vertex: &Function,
    fragment: Option<&Function>,
    blend: Blend,
    what: &'static str,
) -> Result<PipelineState, RendererError> {
    let descriptor = MTLRenderPipelineDescriptor::new();
    descriptor.setVertexFunction(Some(&**vertex));
    descriptor.setFragmentFunction(fragment.map(|function| &**function));
    descriptor.setDepthAttachmentPixelFormat(DEPTH_FORMAT);

    let attachments = descriptor.colorAttachments();
    // SAFETY: slot 0 always exists; a pipeline has up to eight colour attachments.
    let color = unsafe { attachments.objectAtIndexedSubscript(0) };
    color.setPixelFormat(COLOR_FORMAT);
    match blend {
        Blend::Opaque => {}
        Blend::NoColor => color.setWriteMask(MTLColorWriteMask::None),
        Blend::Straight => {
            color.setBlendingEnabled(true);
            color.setSourceRGBBlendFactor(MTLBlendFactor::SourceAlpha);
            color.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
            color.setRgbBlendOperation(MTLBlendOperation::Add);
            color.setSourceAlphaBlendFactor(MTLBlendFactor::One);
            color.setDestinationAlphaBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
            color.setAlphaBlendOperation(MTLBlendOperation::Add);
        }
        Blend::Additive | Blend::Premultiplied => {
            // ONE for the source, because the pixel shader already multiplied by coverage;
            // the destination factor is the whole difference between the two modes.
            let destination = if blend == Blend::Additive {
                MTLBlendFactor::One
            } else {
                MTLBlendFactor::OneMinusSourceAlpha
            };
            color.setBlendingEnabled(true);
            color.setSourceRGBBlendFactor(MTLBlendFactor::One);
            color.setDestinationRGBBlendFactor(destination);
            color.setRgbBlendOperation(MTLBlendOperation::Add);
            color.setSourceAlphaBlendFactor(MTLBlendFactor::One);
            color.setDestinationAlphaBlendFactor(destination);
            color.setAlphaBlendOperation(MTLBlendOperation::Add);
        }
    }

    device
        .newRenderPipelineStateWithDescriptor_error(&descriptor)
        .map_err(|error| {
            api_error(
                "MTLDevice::newRenderPipelineStateWithDescriptor",
                format!("{what}: {}", describe(&error)),
            )
        })
}

/// Which vertex layout and shader file a mesh pipeline uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum MeshKind {
    /// One object per draw, its model matrix in the object constants.
    Static,
    /// Many objects per draw, their transforms in a per-instance stream.
    Instanced,
    /// A skinned character, one bone palette per draw.
    Skinned,
}

impl MeshKind {
    /// The shader file, the vertex entry and the fragment entry.
    fn entries(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Static => ("mesh", "mesh_vs", "mesh_fs"),
            Self::Instanced => ("mesh_instanced", "mesh_instanced_vs", "mesh_instanced_fs"),
            Self::Skinned => ("mesh_skinned", "mesh_skinned_vs", "mesh_skinned_fs"),
        }
    }
}

/// Every mesh pipeline this renderer has needed so far.
///
/// Created on first use and never released, like `MeshPipelines` on D3D12: a scene uses a
/// handful of the possible variants, and the first frame that needs one pays for it.
pub(crate) struct MeshPipelines {
    cache: HashMap<(MeshKind, PipelineVariant), Pipeline>,
}

impl MeshPipelines {
    /// An empty cache.
    pub(crate) fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }

    /// The pipeline for `kind` and `variant`, created if this is its first use.
    ///
    /// # Errors
    ///
    /// [`RendererError::ShaderCompilation`] or [`RendererError::Api`] if a shader or the state
    /// cannot be created.
    pub(crate) fn get(
        &mut self,
        device: &Device,
        libraries: &mut Libraries,
        kind: MeshKind,
        variant: PipelineVariant,
    ) -> Result<&Pipeline, RendererError> {
        Ok(match self.cache.entry((kind, variant)) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                entry.insert(create_mesh_pipeline(device, libraries, kind, variant)?)
            }
        })
    }
}

/// The blend, depth rule and fragment presence a variant asks for.
///
/// The one place the D3D12 descriptors (`blend_desc_for`, `depth_stencil_desc_for_mode`,
/// `depth_prime_blend_desc`, `transparent_blend_desc`) are turned into Metal state.
fn mesh_state(kind: MeshKind, variant: PipelineVariant) -> (Blend, DepthRule, bool) {
    let surface = variant.surface;
    let lit_blend = if surface.is_blended() {
        Blend::Straight
    } else {
        Blend::Opaque
    };
    // The static pipeline has no depth mode: it writes whenever its surface does.
    let primed = kind != MeshKind::Static && variant.depth == DepthMode::Primed;
    let lit_depth = if surface.writes_depth() && !primed {
        DepthRule::Write
    } else {
        DepthRule::Read
    };
    if kind != MeshKind::Skinned {
        return (lit_blend, lit_depth, true);
    }
    match variant.pass {
        CharacterPass::Lit => (lit_blend, lit_depth, true),
        // Depth only, and no pixel shader unless the cutout needs one.
        CharacterPass::DepthPrime => (
            Blend::NoColor,
            DepthRule::Write,
            surface == SurfaceClass::AlphaTest,
        ),
        CharacterPass::Translucent => (Blend::Straight, DepthRule::Read, true),
    }
}

/// Builds the pipeline for one variant.
fn create_mesh_pipeline(
    device: &Device,
    libraries: &mut Libraries,
    kind: MeshKind,
    variant: PipelineVariant,
) -> Result<Pipeline, RendererError> {
    let (file, vertex_entry, fragment_entry) = kind.entries();
    let (blend, depth, has_fragment) = mesh_state(kind, variant);
    let vertex = libraries.function(file, vertex_entry)?;
    let fragment = if has_fragment {
        Some(libraries.mesh_fragment(
            file,
            fragment_entry,
            variant.features.pbr,
            variant.features.normal_map,
            variant.features.shadows,
        )?)
    } else {
        None
    };
    let state = build_state(device, &vertex, fragment.as_ref(), blend, file)?;
    Ok(Pipeline {
        state,
        depth,
        raster: if variant.wireframe {
            Raster::WIREFRAME
        } else {
            Raster::SOLID
        },
    })
}

/// Position + colour vertices: the debug overlay and the stress triangle grid.
pub(crate) struct TrianglePipeline {
    pub(crate) pipeline: Pipeline,
}

impl TrianglePipeline {
    /// Builds it. Depth is tested and written, as `pipeline::depth_stencil_desc` declares:
    /// the overlay's quads order themselves by depth.
    ///
    /// # Errors
    ///
    /// As [`MeshPipelines::get`].
    pub(crate) fn new(device: &Device, libraries: &mut Libraries) -> Result<Self, RendererError> {
        let vertex = libraries.function("triangle", "triangle_vs")?;
        let fragment = libraries.function("triangle", "triangle_fs")?;
        Ok(Self {
            pipeline: Pipeline {
                state: build_state(device, &vertex, Some(&fragment), Blend::Opaque, "triangle")?,
                depth: DepthRule::Write,
                raster: Raster::SOLID,
            },
        })
    }
}
