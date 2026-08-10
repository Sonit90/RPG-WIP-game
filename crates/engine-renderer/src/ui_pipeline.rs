//! The batched UI pass: one pipeline, one texture, one draw call for the whole HUD.
//!
//! ```text
//! UiDrawList  ->  UiBatcher  ->  [UiVertex]  ->  upload ring  ->  one DrawInstanced
//!  (commands)     (this file)     one buffer
//! ```
//!
//! Every UI element — a panel, a health bar, an outline, a glyph — is a textured, tinted
//! quad. Glyphs sample their cell in the font atlas; solid shapes sample the one opaque
//! texel in the same atlas. Because they share a texture and a pipeline state, they share a
//! **batch**, and the entire user interface is submitted as a single draw.
//!
//! ## What this replaces
//!
//! Phase 3's overlay emitted one quad per lit font cell — roughly fifteen quads per
//! character — and drew each overlay layer separately. Milestone 2 measured twenty
//! nameplates at 0.15-0.23 ms of CPU and a CPU p95 of 0.97 ms against 0.26 ms without them.
//! Here a character is one quad, so the same text is about a fifteenth of the geometry.
//!
//! ## Ordering
//!
//! Depth testing and depth writing are **off**. The list is painted in order, which is the
//! only ordering strategy that works once anything is translucent: a blended fragment that
//! fails a depth test vanishes, and one that passes writes depth that occludes whatever
//! should have blended over it. See `engine_render_data::ui` for the full reasoning.

use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D12::{
    D3D_ROOT_SIGNATURE_VERSION_1, D3D12_BLEND_DESC, D3D12_BLEND_INV_SRC_ALPHA, D3D12_BLEND_ONE,
    D3D12_BLEND_OP_ADD, D3D12_BLEND_SRC_ALPHA, D3D12_COLOR_WRITE_ENABLE_ALL,
    D3D12_COMPARISON_FUNC_ALWAYS, D3D12_CULL_MODE_NONE, D3D12_DEPTH_STENCIL_DESC,
    D3D12_DEPTH_WRITE_MASK_ZERO, D3D12_DESCRIPTOR_RANGE, D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
    D3D12_FILL_MODE_SOLID, D3D12_FILTER_MIN_MAG_MIP_POINT, D3D12_GRAPHICS_PIPELINE_STATE_DESC,
    D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, D3D12_INPUT_ELEMENT_DESC, D3D12_INPUT_LAYOUT_DESC,
    D3D12_LOGIC_OP_NOOP, D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE, D3D12_RASTERIZER_DESC,
    D3D12_RENDER_TARGET_BLEND_DESC, D3D12_ROOT_DESCRIPTOR, D3D12_ROOT_DESCRIPTOR_TABLE,
    D3D12_ROOT_PARAMETER, D3D12_ROOT_PARAMETER_0, D3D12_ROOT_PARAMETER_TYPE_CBV,
    D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE, D3D12_ROOT_SIGNATURE_DESC,
    D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT, D3D12_SHADER_VISIBILITY_ALL,
    D3D12_SHADER_VISIBILITY_PIXEL, D3D12_STATIC_BORDER_COLOR_TRANSPARENT_BLACK,
    D3D12_STATIC_SAMPLER_DESC, D3D12_TEXTURE_ADDRESS_MODE_CLAMP, D3D12SerializeRootSignature,
    ID3D12Device, ID3D12PipelineState, ID3D12RootSignature,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_R32G32_FLOAT, DXGI_FORMAT_R32G32B32A32_FLOAT, DXGI_SAMPLE_DESC,
};
use windows::core::s;

use engine_render_data::ui::{TextAlign, UiCommand, UiDrawList, UiRect};

use crate::error::RendererError;
use crate::font::{GLYPH_ADVANCE, GLYPH_HEIGHT, GLYPH_WIDTH};
use crate::font_atlas::{FontAtlas, GlyphUv};
use crate::shader::{UI_PS, UI_VS, bytecode};
use crate::swapchain::BACK_BUFFER_FORMAT;

/// Root parameter holding the pixels-to-clip constant buffer.
pub const ROOT_UI_FRAME_CBV: u32 = 0;

/// Root parameter holding the font-atlas descriptor table.
pub const ROOT_UI_ATLAS_TABLE: u32 = 1;

/// Segments in a full radial sweep.
///
/// Sixteen is smooth enough for a 48-pixel button and cheap enough that a bar of ten cooling
/// abilities costs a hundred and sixty triangles.
const SECTOR_SEGMENTS: usize = 16;

/// Vertices per quad: two triangles, no index buffer.
///
/// An index buffer would save a third of the vertex bytes and cost a second buffer to
/// allocate, upload and bind. At the sizes this pass reaches — tens of thousands of
/// vertices — the copy is not what costs, so the simpler shape wins.
pub const VERTICES_PER_QUAD: usize = 6;

/// One UI vertex: a pixel position, a texture coordinate and a colour.
///
/// `repr(C)` is required — the input layout describes this exact memory layout to the GPU,
/// so Rust must not be free to reorder the fields. 32 bytes, two per cache line.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct UiVertex {
    /// Position in pixels, origin at the client area's top-left.
    pub position: [f32; 2],
    /// Where to sample the font atlas.
    pub uv: [f32; 2],
    /// Linear RGBA tint. Alpha is honoured.
    pub color: [f32; 4],
}

/// The constant buffer the UI vertex shader reads.
///
/// Just the multiply that takes pixels to normalized device coordinates. Computing it once
/// per frame on the CPU means the shader does no divide, and a resize changes two floats
/// rather than every vertex.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct UiFrameConstants {
    /// `2 / width` and `-2 / height`.
    pub pixel_to_clip: [f32; 2],
    /// Padding to the 16-byte constant-buffer row.
    pub padding: [f32; 2],
}

impl UiFrameConstants {
    /// The constants for a viewport of this size.
    ///
    /// A zero dimension produces a zero scale rather than an infinity: a minimized window
    /// reports one, and a NaN in a vertex position takes out the whole draw.
    #[must_use]
    pub fn for_viewport(width: u32, height: u32) -> Self {
        let x = if width == 0 { 0.0 } else { 2.0 / width as f32 };
        let y = if height == 0 {
            0.0
        } else {
            -2.0 / height as f32
        };
        Self {
            pixel_to_clip: [x, y],
            padding: [0.0; 2],
        }
    }
}

/// What the last batch produced.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct UiBatchStats {
    /// Commands consumed.
    pub commands: usize,
    /// Quads emitted.
    pub quads: usize,
    /// Vertices emitted.
    pub vertices: usize,
    /// Glyphs emitted, of which spaces produce none.
    pub glyphs: usize,
    /// Quads dropped because the vertex budget was reached.
    pub dropped: usize,
}

/// Turns a [`UiDrawList`] into one vertex buffer.
///
/// Owns its scratch, which is cleared and never freed, so batching a frame allocates
/// nothing once the peak has been seen.
#[derive(Debug)]
pub struct UiBatcher {
    vertices: Vec<UiVertex>,
    max_quads: usize,
    stats: UiBatchStats,
}

impl UiBatcher {
    /// Creates a batcher that will emit at most `max_quads` quads per frame.
    #[must_use]
    pub fn new(max_quads: usize) -> Self {
        Self {
            vertices: Vec::with_capacity(max_quads * VERTICES_PER_QUAD),
            max_quads,
            stats: UiBatchStats::default(),
        }
    }

    /// The vertices the last [`UiBatcher::build`] produced.
    #[must_use]
    pub fn vertices(&self) -> &[UiVertex] {
        &self.vertices
    }

    /// What the last build produced.
    #[must_use]
    pub fn stats(&self) -> UiBatchStats {
        self.stats
    }

    /// The most quads a frame may emit.
    #[must_use]
    pub fn max_quads(&self) -> usize {
        self.max_quads
    }

    /// Expands a draw list into vertices, in paint order.
    ///
    /// Returns the vertices produced. Quads past the budget are counted in
    /// [`UiBatchStats::dropped`] rather than growing the buffer, so a runaway UI degrades
    /// visibly instead of allocating without bound in the frame path.
    pub fn build(&mut self, list: &UiDrawList) -> &[UiVertex] {
        self.vertices.clear();
        let mut stats = UiBatchStats {
            commands: list.len(),
            ..UiBatchStats::default()
        };

        for command in list.commands() {
            match *command {
                UiCommand::Rect { rect, color } => {
                    self.push_solid(rect, color, &mut stats);
                }
                UiCommand::RadialFill {
                    rect,
                    fraction,
                    color,
                } => {
                    self.push_radial(rect, fraction, color, &mut stats);
                }
                UiCommand::Outline {
                    rect,
                    thickness,
                    color,
                } => {
                    // Four bars rather than a stroked rectangle: the top and bottom span the
                    // full width and the sides fill what is left, so the corners are covered
                    // exactly once. Overlapping them would double the alpha at each corner,
                    // which is visible the moment the outline is translucent.
                    let inner_height = (rect.height - thickness * 2.0).max(0.0);
                    self.push_solid(
                        UiRect::new(rect.x, rect.y, rect.width, thickness),
                        color,
                        &mut stats,
                    );
                    self.push_solid(
                        UiRect::new(rect.x, rect.bottom() - thickness, rect.width, thickness),
                        color,
                        &mut stats,
                    );
                    self.push_solid(
                        UiRect::new(rect.x, rect.y + thickness, thickness, inner_height),
                        color,
                        &mut stats,
                    );
                    self.push_solid(
                        UiRect::new(
                            rect.right() - thickness,
                            rect.y + thickness,
                            thickness,
                            inner_height,
                        ),
                        color,
                        &mut stats,
                    );
                }
                UiCommand::Text {
                    origin,
                    range,
                    color,
                    scale,
                    align,
                } => {
                    let text = list.text_of(range);
                    let width = text_width(text, scale);
                    let x = match align {
                        TextAlign::Left => origin.0,
                        TextAlign::Center => origin.0 - width * 0.5,
                        TextAlign::Right => origin.0 - width,
                    };
                    let mut pen = x;
                    for character in text.chars() {
                        if !FontAtlas::is_blank(character) {
                            let rect = UiRect::new(
                                pen,
                                origin.1,
                                GLYPH_WIDTH as f32 * scale,
                                GLYPH_HEIGHT as f32 * scale,
                            );
                            self.push_quad(rect, FontAtlas::glyph_uv(character), color, &mut stats);
                            stats.glyphs += 1;
                        }
                        pen += GLYPH_ADVANCE as f32 * scale;
                    }
                }
            }
        }

        stats.vertices = self.vertices.len();
        self.stats = stats;
        &self.vertices
    }

    /// Appends an untextured rectangle, sampling the atlas's opaque texel.
    fn push_solid(&mut self, rect: UiRect, color: [f32; 4], stats: &mut UiBatchStats) {
        if !rect.is_visible() {
            return;
        }
        self.push_quad(rect, FontAtlas::solid_uv(), color, stats);
    }

    /// Appends a radial sector as a triangle fan from the centre.
    ///
    /// Swept clockwise from twelve o'clock, which is the direction every cooldown in the
    /// genre runs. One triangle per segment, and each triangle is emitted as a degenerate
    /// quad so the whole pass stays one buffer and one draw call.
    fn push_radial(
        &mut self,
        rect: UiRect,
        fraction: f32,
        color: [f32; 4],
        stats: &mut UiBatchStats,
    ) {
        if !rect.is_visible() || fraction <= 0.0 {
            return;
        }
        let fraction = fraction.min(1.0);
        // Enough segments that the arc reads as round at a 48-pixel button, few enough that
        // a full bar of cooling abilities is still a couple of hundred triangles.
        let segments = (SECTOR_SEGMENTS as f32 * fraction).ceil().max(1.0) as usize;
        let centre = (rect.x + rect.width * 0.5, rect.y + rect.height * 0.5);
        let radius = rect.width.max(rect.height) * 0.5;
        let uv = FontAtlas::solid_uv();
        let sweep = fraction * std::f32::consts::TAU;

        for segment in 0..segments {
            if stats.quads >= self.max_quads {
                stats.dropped += 1;
                return;
            }
            let from = sweep * (segment as f32 / segments as f32);
            let to = sweep * ((segment + 1) as f32 / segments as f32);
            let point = |angle: f32| {
                // Zero is straight up and the sweep runs clockwise.
                (
                    centre.0 + angle.sin() * radius,
                    centre.1 - angle.cos() * radius,
                )
            };
            let (ax, ay) = point(from);
            let (bx, by) = point(to);
            let vertex = |position: (f32, f32)| UiVertex {
                position: [position.0, position.1],
                uv: [uv.u0, uv.v0],
                color,
            };
            let a = vertex(centre);
            let b = vertex((ax, ay));
            let c = vertex((bx, by));
            // Six vertices per segment, like a quad, so the buffer stays uniform.
            self.vertices.extend_from_slice(&[a, b, c, a, b, c]);
            stats.quads += 1;
        }
    }

    /// Appends one quad as two triangles.
    fn push_quad(&mut self, rect: UiRect, uv: GlyphUv, color: [f32; 4], stats: &mut UiBatchStats) {
        if stats.quads >= self.max_quads {
            stats.dropped += 1;
            return;
        }

        let (left, top) = (rect.x, rect.y);
        let (right, bottom) = (rect.right(), rect.bottom());
        let corners = [
            UiVertex {
                position: [left, top],
                uv: [uv.u0, uv.v0],
                color,
            },
            UiVertex {
                position: [right, top],
                uv: [uv.u1, uv.v0],
                color,
            },
            UiVertex {
                position: [left, bottom],
                uv: [uv.u0, uv.v1],
                color,
            },
            UiVertex {
                position: [right, bottom],
                uv: [uv.u1, uv.v1],
                color,
            },
        ];
        // Two triangles wound so they survive the rasterizer's front-face rule. Culling is
        // off for this pass, so the winding is documentation rather than load-bearing.
        self.vertices.extend_from_slice(&[
            corners[0], corners[2], corners[1], corners[1], corners[2], corners[3],
        ]);
        stats.quads += 1;
    }
}

/// How wide a run of text is, in pixels.
///
/// The last glyph contributes its width rather than its advance, so a centred string is
/// centred on the ink rather than on the gap after it.
#[must_use]
pub fn text_width(text: &str, scale: f32) -> f32 {
    let count = text.chars().count();
    if count == 0 {
        return 0.0;
    }
    ((count - 1) * GLYPH_ADVANCE + GLYPH_WIDTH) as f32 * scale
}

/// How tall one line of text is, in pixels.
#[must_use]
pub fn text_height(scale: f32) -> f32 {
    GLYPH_HEIGHT as f32 * scale
}

/// Root signature and pipeline state for the UI pass.
#[derive(Debug)]
pub struct UiPipeline {
    root_signature: ID3D12RootSignature,
    state: ID3D12PipelineState,
}

impl UiPipeline {
    /// Creates the UI pipeline.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if the root signature or pipeline state is rejected.
    pub fn new(device: &ID3D12Device) -> Result<Self, RendererError> {
        let root_signature = create_root_signature(device)?;
        let elements = input_elements();
        let desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
            // SAFETY: reinterprets the COM smart pointer as the raw interface pointer the
            // descriptor field expects, without changing the reference count. The signature
            // is kept alive by the local binding for the whole of the creation call below,
            // and by `Self` afterwards.
            pRootSignature: unsafe { std::mem::transmute_copy(&root_signature) },
            VS: bytecode(UI_VS),
            PS: bytecode(UI_PS),
            BlendState: alpha_blend_desc(),
            SampleMask: u32::MAX,
            RasterizerState: ui_rasterizer_desc(),
            DepthStencilState: ui_depth_stencil_desc(),
            InputLayout: D3D12_INPUT_LAYOUT_DESC {
                pInputElementDescs: elements.as_ptr(),
                NumElements: elements.len() as u32,
            },
            PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
            NumRenderTargets: 1,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            ..Default::default()
        };
        let mut desc = desc;
        desc.RTVFormats[0] = BACK_BUFFER_FORMAT;
        // No depth-stencil format: this pass neither tests nor writes depth, and declaring
        // one it never uses would tie the UI to the scene's depth buffer for nothing.

        // SAFETY: FFI call with a fully initialized descriptor whose borrowed root
        // signature, bytecode and input elements all outlive it.
        let state: ID3D12PipelineState = unsafe { device.CreateGraphicsPipelineState(&desc) }
            .map_err(|error| RendererError::api("CreateGraphicsPipelineState (ui)", error))?;

        Ok(Self {
            root_signature,
            state,
        })
    }

    /// The root signature.
    #[must_use]
    pub fn root_signature(&self) -> &ID3D12RootSignature {
        &self.root_signature
    }

    /// The pipeline state.
    #[must_use]
    pub fn state(&self) -> &ID3D12PipelineState {
        &self.state
    }
}

/// The UI vertex layout, matching [`UiVertex`] exactly.
#[must_use]
pub fn input_elements() -> [D3D12_INPUT_ELEMENT_DESC; 3] {
    [
        D3D12_INPUT_ELEMENT_DESC {
            SemanticName: s!("POSITION"),
            SemanticIndex: 0,
            Format: DXGI_FORMAT_R32G32_FLOAT,
            InputSlot: 0,
            AlignedByteOffset: 0,
            InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
            InstanceDataStepRate: 0,
        },
        D3D12_INPUT_ELEMENT_DESC {
            SemanticName: s!("TEXCOORD"),
            SemanticIndex: 0,
            Format: DXGI_FORMAT_R32G32_FLOAT,
            InputSlot: 0,
            AlignedByteOffset: 8,
            InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
            InstanceDataStepRate: 0,
        },
        D3D12_INPUT_ELEMENT_DESC {
            SemanticName: s!("COLOR"),
            SemanticIndex: 0,
            Format: DXGI_FORMAT_R32G32B32A32_FLOAT,
            InputSlot: 0,
            AlignedByteOffset: 16,
            InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
            InstanceDataStepRate: 0,
        },
    ]
}

/// Straight alpha blending: `src * a + dst * (1 - a)`.
///
/// Alpha is written as `a + dst * (1 - a)` rather than replaced, so a translucent element
/// drawn over another accumulates coverage instead of punching a hole in it. That only
/// matters if the UI is ever composited rather than drawn straight to the back buffer, and
/// getting it right costs nothing today.
#[must_use]
pub fn alpha_blend_desc() -> D3D12_BLEND_DESC {
    let target = D3D12_RENDER_TARGET_BLEND_DESC {
        BlendEnable: true.into(),
        LogicOpEnable: false.into(),
        SrcBlend: D3D12_BLEND_SRC_ALPHA,
        DestBlend: D3D12_BLEND_INV_SRC_ALPHA,
        BlendOp: D3D12_BLEND_OP_ADD,
        SrcBlendAlpha: D3D12_BLEND_ONE,
        DestBlendAlpha: D3D12_BLEND_INV_SRC_ALPHA,
        BlendOpAlpha: D3D12_BLEND_OP_ADD,
        LogicOp: D3D12_LOGIC_OP_NOOP,
        RenderTargetWriteMask: D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8,
    };
    D3D12_BLEND_DESC {
        RenderTarget: [target; 8],
        ..Default::default()
    }
}

/// Solid fill, no culling: a UI quad has no meaningful facing.
fn ui_rasterizer_desc() -> D3D12_RASTERIZER_DESC {
    D3D12_RASTERIZER_DESC {
        FillMode: D3D12_FILL_MODE_SOLID,
        CullMode: D3D12_CULL_MODE_NONE,
        FrontCounterClockwise: true.into(),
        DepthClipEnable: true.into(),
        ..Default::default()
    }
}

/// No depth test, no depth write: the UI is painted in list order.
fn ui_depth_stencil_desc() -> D3D12_DEPTH_STENCIL_DESC {
    D3D12_DEPTH_STENCIL_DESC {
        DepthEnable: false.into(),
        DepthWriteMask: D3D12_DEPTH_WRITE_MASK_ZERO,
        DepthFunc: D3D12_COMPARISON_FUNC_ALWAYS,
        StencilEnable: false.into(),
        ..Default::default()
    }
}

/// Two root parameters and one static sampler.
///
/// The sampler is **static** rather than a descriptor: it never changes, and a static
/// sampler costs no descriptor heap space and no bind. Point filtering, because the atlas is
/// a crisp 1-bit bitmap and bilinear sampling of one would produce grey fringes around
/// letters that were designed to have none.
fn create_root_signature(device: &ID3D12Device) -> Result<ID3D12RootSignature, RendererError> {
    let range = D3D12_DESCRIPTOR_RANGE {
        RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
        NumDescriptors: 1,
        BaseShaderRegister: 0,
        RegisterSpace: 0,
        OffsetInDescriptorsFromTableStart: 0,
    };
    let parameters = [
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_CBV,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                Descriptor: D3D12_ROOT_DESCRIPTOR {
                    ShaderRegister: 0,
                    RegisterSpace: 0,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE {
                    NumDescriptorRanges: 1,
                    pDescriptorRanges: &range,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
        },
    ];

    let sampler = D3D12_STATIC_SAMPLER_DESC {
        Filter: D3D12_FILTER_MIN_MAG_MIP_POINT,
        AddressU: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        AddressV: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        AddressW: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        MipLODBias: 0.0,
        MaxAnisotropy: 1,
        ComparisonFunc: D3D12_COMPARISON_FUNC_ALWAYS,
        BorderColor: D3D12_STATIC_BORDER_COLOR_TRANSPARENT_BLACK,
        MinLOD: 0.0,
        MaxLOD: 0.0,
        ShaderRegister: 0,
        RegisterSpace: 0,
        ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
    };

    let desc = D3D12_ROOT_SIGNATURE_DESC {
        NumParameters: parameters.len() as u32,
        pParameters: parameters.as_ptr(),
        NumStaticSamplers: 1,
        pStaticSamplers: &sampler,
        Flags: D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT,
    };

    let mut blob: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;
    // SAFETY: FFI call. `desc`, the parameter array, the range and the sampler all outlive
    // the call, and both out-parameters are valid.
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
            "D3D12SerializeRootSignature (ui)",
            error,
        ));
    }
    let Some(blob) = blob else {
        return Err(RendererError::InvalidConfig {
            reason: "the UI root signature serialized to no blob".to_owned(),
        });
    };

    // SAFETY: FFI calls reading the serialized blob's buffer, which stays alive for the
    // duration of the call.
    let bytes = unsafe {
        std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize())
    };
    // SAFETY: FFI call. `bytes` points into the live blob and node mask 0 targets the
    // single-adapter case. The return type is fixed by this function's signature, which is
    // what selects the COM interface the generic call produces.
    unsafe { device.CreateRootSignature(0, bytes) }
        .map_err(|error| RendererError::api("CreateRootSignature (ui)", error))
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_render_data::ui::UiDrawList;

    const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

    fn batcher() -> UiBatcher {
        UiBatcher::new(4096)
    }

    #[test]
    fn the_vertex_layout_matches_the_input_elements() {
        // The GPU is told this exact memory layout; a reordered field renders garbage
        // rather than failing.
        assert_eq!(size_of::<UiVertex>(), 32, "2 + 2 + 4 floats, packed");
        assert_eq!(align_of::<UiVertex>(), 4);
        let elements = input_elements();
        assert_eq!(elements[0].AlignedByteOffset, 0);
        assert_eq!(elements[1].AlignedByteOffset, 8);
        assert_eq!(elements[2].AlignedByteOffset, 16);
    }

    #[test]
    fn a_rectangle_becomes_exactly_one_quad() {
        let mut batcher = batcher();
        let mut list = UiDrawList::new();
        list.rect(UiRect::new(10.0, 20.0, 100.0, 8.0), WHITE);

        let vertices = batcher.build(&list).to_vec();
        assert_eq!(batcher.stats().quads, 1);
        assert_eq!(vertices.len(), VERTICES_PER_QUAD);
        // The corners span the rectangle exactly.
        let xs: Vec<f32> = vertices.iter().map(|vertex| vertex.position[0]).collect();
        assert_eq!(xs.iter().cloned().fold(f32::MAX, f32::min), 10.0);
        assert_eq!(xs.iter().cloned().fold(f32::MIN, f32::max), 110.0);
    }

    #[test]
    fn a_character_is_one_quad_rather_than_one_per_lit_cell() {
        // The entire point of the phase, as an assertion. Phase 3 emitted roughly fifteen
        // quads for this; one is the target.
        let mut batcher = batcher();
        let mut list = UiDrawList::new();
        list.text_at((0.0, 0.0), "A", WHITE, 1.0);

        batcher.build(&list);
        assert_eq!(batcher.stats().quads, 1);
        assert_eq!(batcher.stats().glyphs, 1);
    }

    #[test]
    fn a_line_of_text_costs_one_quad_per_visible_character() {
        let mut batcher = batcher();
        let mut list = UiDrawList::new();
        list.text_at((0.0, 0.0), "DUMMY 100%", WHITE, 1.0);

        batcher.build(&list);
        // Ten characters, one of which is a space and draws nothing.
        assert_eq!(batcher.stats().glyphs, 9);
        assert_eq!(batcher.stats().quads, 9);
        assert_eq!(batcher.stats().vertices, 9 * VERTICES_PER_QUAD);
    }

    #[test]
    fn a_space_advances_the_pen_without_emitting_geometry() {
        let mut batcher = batcher();
        let mut list = UiDrawList::new();
        list.text_at((0.0, 0.0), "A B", WHITE, 1.0);
        let vertices = batcher.build(&list).to_vec();

        assert_eq!(batcher.stats().quads, 2);
        let first = vertices[0].position[0];
        let second = vertices[VERTICES_PER_QUAD].position[0];
        assert_eq!(
            second - first,
            (GLYPH_ADVANCE * 2) as f32,
            "the space still moved the pen"
        );
    }

    #[test]
    fn an_outline_is_four_bars_that_meet_at_the_corners_without_overlapping() {
        // Overlapping corners double the alpha, which shows the moment an outline is
        // translucent.
        let mut batcher = batcher();
        let mut list = UiDrawList::new();
        list.outline(UiRect::new(0.0, 0.0, 20.0, 10.0), 2.0, WHITE);
        batcher.build(&list);

        assert_eq!(batcher.stats().quads, 4);
        let vertices = batcher.vertices();
        // The vertical bars start below the top bar and end above the bottom one.
        let left_bar_top = vertices[2 * VERTICES_PER_QUAD].position[1];
        assert_eq!(left_bar_top, 2.0);
    }

    #[test]
    fn text_and_rectangles_share_one_batch() {
        // They sample the same atlas through the same pipeline, so the whole HUD is one
        // draw. If a rectangle ever needed a different texture this would be two.
        let mut batcher = batcher();
        let mut list = UiDrawList::new();
        list.bar(UiRect::new(0.0, 0.0, 100.0, 10.0), 0.5, WHITE, WHITE);
        list.text_at((0.0, 0.0), "HP", WHITE, 1.0);

        let vertices = batcher.build(&list).to_vec();
        assert_eq!(batcher.stats().quads, 4, "two bar rects plus two glyphs");
        assert_eq!(vertices.len(), 4 * VERTICES_PER_QUAD);

        // The solid quads sample a point; the glyphs sample a region. One texture, one draw.
        let solid = FontAtlas::solid_uv();
        assert_eq!(vertices[0].uv, [solid.u0, solid.v0]);
        let glyph = FontAtlas::glyph_uv('H');
        assert_eq!(vertices[2 * VERTICES_PER_QUAD].uv, [glyph.u0, glyph.v0]);
    }

    #[test]
    fn centred_text_is_centred_on_its_ink() {
        let mut batcher = batcher();
        let mut list = UiDrawList::new();
        list.text_aligned((100.0, 0.0), "ABC", WHITE, 1.0, TextAlign::Center);
        batcher.build(&list);

        let width = text_width("ABC", 1.0);
        let left = batcher.vertices()[0].position[0];
        assert_eq!(left, 100.0 - width * 0.5);

        // Right-aligned text ends at the origin.
        list.clear();
        list.text_aligned((100.0, 0.0), "ABC", WHITE, 1.0, TextAlign::Right);
        batcher.build(&list);
        assert_eq!(batcher.vertices()[0].position[0], 100.0 - width);
    }

    #[test]
    fn scale_multiplies_both_the_glyph_and_the_advance() {
        let mut batcher = batcher();
        let mut list = UiDrawList::new();
        list.text_at((0.0, 0.0), "AB", WHITE, 2.0);
        batcher.build(&list);

        let vertices = batcher.vertices();
        // Winding is top-left, bottom-left, top-right, so index 2 is the far corner.
        let first_width = vertices[2].position[0] - vertices[0].position[0];
        assert_eq!(first_width, GLYPH_WIDTH as f32 * 2.0);
        let advance = vertices[VERTICES_PER_QUAD].position[0] - vertices[0].position[0];
        assert_eq!(advance, GLYPH_ADVANCE as f32 * 2.0);
    }

    #[test]
    fn the_quad_budget_drops_work_rather_than_growing_the_buffer_mid_frame() {
        // A runaway UI must degrade visibly, not allocate without bound in the frame path.
        let mut batcher = UiBatcher::new(4);
        let mut list = UiDrawList::new();
        for index in 0..10 {
            list.rect(UiRect::new(index as f32, 0.0, 2.0, 2.0), WHITE);
        }
        batcher.build(&list);

        assert_eq!(batcher.stats().quads, 4);
        assert_eq!(batcher.stats().dropped, 6);
        assert_eq!(batcher.vertices().len(), 4 * VERTICES_PER_QUAD);
    }

    #[test]
    fn rebuilding_reuses_the_vertex_allocation() {
        let mut batcher = batcher();
        let mut list = UiDrawList::new();
        for index in 0..200 {
            list.rect(UiRect::new(index as f32, 0.0, 2.0, 2.0), WHITE);
            list.text_at((0.0, index as f32), "DAMAGE 100", WHITE, 1.0);
        }
        batcher.build(&list);
        let capacity = batcher.vertices.capacity();

        for _ in 0..500 {
            batcher.build(&list);
        }
        assert_eq!(batcher.vertices.capacity(), capacity);
    }

    #[test]
    fn a_zero_sized_viewport_produces_a_zero_scale_rather_than_an_infinity() {
        // A minimized window reports one, and a NaN vertex position takes out the draw.
        let constants = UiFrameConstants::for_viewport(0, 0);
        assert_eq!(constants.pixel_to_clip, [0.0, 0.0]);

        let constants = UiFrameConstants::for_viewport(1920, 1080);
        assert!((constants.pixel_to_clip[0] - 2.0 / 1920.0).abs() < 1e-9);
        assert!((constants.pixel_to_clip[1] + 2.0 / 1080.0).abs() < 1e-9);
    }

    #[test]
    fn the_ui_pass_neither_tests_nor_writes_depth() {
        // Painter's order is the ordering strategy; a depth test would make a blended
        // fragment vanish and a depth write would occlude what should blend over it.
        let depth = ui_depth_stencil_desc();
        assert!(!depth.DepthEnable.as_bool());
        assert_eq!(depth.DepthWriteMask, D3D12_DEPTH_WRITE_MASK_ZERO);
    }

    #[test]
    fn the_ui_blend_state_is_straight_alpha() {
        let blend = alpha_blend_desc();
        let target = blend.RenderTarget[0];
        assert!(target.BlendEnable.as_bool());
        assert_eq!(target.SrcBlend, D3D12_BLEND_SRC_ALPHA);
        assert_eq!(target.DestBlend, D3D12_BLEND_INV_SRC_ALPHA);
    }

    #[test]
    fn an_empty_list_produces_no_vertices_and_no_draw() {
        let mut batcher = batcher();
        let list = UiDrawList::new();
        assert!(batcher.build(&list).is_empty());
        assert_eq!(batcher.stats().quads, 0);
    }
}
