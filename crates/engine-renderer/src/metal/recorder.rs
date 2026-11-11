//! The camera pass's render command encoder, with the redundant-bind elimination and the
//! counters `CommandContext` provides on D3D12.
//!
//! Metal keeps buffer, texture and pipeline bindings on the encoder until they change, so the
//! recorder remembers what is bound and skips a bind that would repeat it. The counts it keeps
//! are the ones `CommandStats` documents: draws, instances, triangles, pipeline changes, and
//! how many binds the sort saved.

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{
    MTLBuffer, MTLCommandEncoder, MTLDepthClipMode, MTLIndexType, MTLPrimitiveType,
    MTLRenderCommandEncoder, MTLScissorRect, MTLTexture, MTLViewport, MTLWinding,
};

use super::pipelines::{DepthRule, DepthStates, Pipeline, Raster};
use crate::command::CommandStats;

/// Vertex buffer slot of the per-vertex stream (`BINDINGS.md`: `16 + input slot`).
pub(crate) const VERTEX_STREAM: usize = 16;
/// Vertex buffer slot of the per-instance stream.
pub(crate) const INSTANCE_STREAM: usize = 17;
/// Buffer slot of the frame constants (and the UI's and particles' own frame block).
pub(crate) const FRAME_SLOT: usize = 0;
/// Buffer slot of the object constants.
pub(crate) const OBJECT_SLOT: usize = 1;
/// Buffer slot of the material constants.
pub(crate) const MATERIAL_SLOT: usize = 2;
/// Buffer slot of the bone palette.
pub(crate) const SKIN_SLOT: usize = 3;
/// Fragment buffer slot of the clustered light masks.
pub(crate) const CLUSTER_SLOT: usize = 4;

/// The encoder type the bindings hand out.
pub(crate) type Encoder = Retained<ProtocolObject<dyn MTLRenderCommandEncoder>>;

/// Identity of an Objective-C object, for "is this already bound".
fn identity<T: ?Sized>(object: &T) -> usize {
    std::ptr::from_ref(object).cast::<()>() as usize
}

/// An open render pass and what is bound in it.
#[derive(Default)]
pub(crate) struct Recorder {
    encoder: Option<Encoder>,
    stats: CommandStats,
    pipeline: usize,
    depth: Option<DepthRule>,
    raster: Option<Raster>,
    /// The mesh buffer and the material constants last bound, by identity.
    mesh: usize,
    material: usize,
    /// The textures last bound to fragment slots 0..4.
    textures: [usize; 4],
}

impl Recorder {
    /// Starts recording into `encoder`, with its viewport covering `width` x `height`.
    pub(crate) fn begin(&mut self, encoder: Encoder, width: u32, height: u32) {
        *self = Self::default();
        encoder.setViewport(MTLViewport {
            originX: 0.0,
            originY: 0.0,
            width: f64::from(width),
            height: f64::from(height),
            znear: 0.0,
            zfar: 1.0,
        });
        encoder.setScissorRect(MTLScissorRect {
            x: 0,
            y: 0,
            width: width as usize,
            height: height as usize,
        });
        encoder.setFrontFacingWinding(MTLWinding::CounterClockwise);
        self.encoder = Some(encoder);
    }

    /// Whether a pass is open.
    pub(crate) fn is_recording(&self) -> bool {
        self.encoder.is_some()
    }

    /// What the open pass has counted so far.
    pub(crate) fn stats(&self) -> CommandStats {
        self.stats
    }

    /// Restricts drawing to the square at (`x`, `y`) of the target: the viewport and the
    /// scissor together, as D3D12's `set_viewport_rect` does. One shadow view's tile.
    pub(crate) fn set_viewport_rect(&mut self, x: u32, y: u32, width: u32, height: u32) {
        let Some(encoder) = &self.encoder else {
            return;
        };
        encoder.setViewport(MTLViewport {
            originX: f64::from(x),
            originY: f64::from(y),
            width: f64::from(width),
            height: f64::from(height),
            znear: 0.0,
            zfar: 1.0,
        });
        encoder.setScissorRect(MTLScissorRect {
            x: x as usize,
            y: y as usize,
            width: width as usize,
            height: height as usize,
        });
    }

    /// Sets the rasterizer's depth offset for the rest of the pass. D3D12 bakes this into the
    /// pipeline state; Metal keeps it on the encoder.
    pub(crate) fn set_depth_bias(&mut self, bias: f32, slope_scale: f32, clamp: f32) {
        if let Some(encoder) = &self.encoder {
            encoder.setDepthBias_slopeScale_clamp(bias, slope_scale, clamp);
        }
    }

    /// Chooses between clipping at the near and far planes and clamping to them
    /// (D3D12 `DepthClipEnable` false).
    pub(crate) fn set_depth_clamp(&mut self, clamp: bool) {
        if let Some(encoder) = &self.encoder {
            encoder.setDepthClipMode(if clamp {
                MTLDepthClipMode::Clamp
            } else {
                MTLDepthClipMode::Clip
            });
        }
    }

    /// Ends the pass and returns what it counted.
    pub(crate) fn finish(&mut self) -> CommandStats {
        if let Some(encoder) = self.encoder.take() {
            encoder.endEncoding();
        }
        std::mem::take(&mut self.stats)
    }

    /// Binds a pipeline with its depth-stencil state and rasterizer state, skipping what is
    /// already bound.
    pub(crate) fn set_pipeline(&mut self, pipeline: &Pipeline, depth_states: &DepthStates) {
        let Some(encoder) = &self.encoder else {
            return;
        };
        let id = identity(&*pipeline.state);
        if self.pipeline == id {
            self.stats.redundant_pipeline_binds += 1;
        } else {
            encoder.setRenderPipelineState(&pipeline.state);
            self.pipeline = id;
            self.stats.pipeline_changes += 1;
        }
        if self.depth != Some(pipeline.depth) {
            encoder.setDepthStencilState(Some(depth_states.get(pipeline.depth)));
            self.depth = Some(pipeline.depth);
        }
        if self.raster != Some(pipeline.raster) {
            encoder.setCullMode(pipeline.raster.cull_mode());
            encoder.setTriangleFillMode(pipeline.raster.fill_mode());
            self.raster = Some(pipeline.raster);
        }
    }

    /// Binds `buffer` at `offset` to the vertex stage's `slot`.
    pub(crate) fn set_vertex_buffer(
        &mut self,
        slot: usize,
        buffer: &ProtocolObject<dyn MTLBuffer>,
        offset: usize,
    ) {
        let Some(encoder) = &self.encoder else {
            return;
        };
        self.stats.vertex_buffer_binds += 1;
        if slot == VERTEX_STREAM {
            let id = identity(buffer);
            if self.mesh == id {
                self.stats.redundant_vertex_buffer_binds += 1;
            } else {
                self.stats.mesh_changes += 1;
                self.mesh = id;
            }
        }
        // SAFETY: the buffer is alive and the offset lies inside it; the callers bind
        // allocations of buffers they hold for the frame, and Metal retains the buffer for
        // the command buffer's life besides.
        unsafe { encoder.setVertexBuffer_offset_atIndex(Some(buffer), offset, slot) };
    }

    /// Binds `buffer` at `offset` to the fragment stage's `slot`.
    pub(crate) fn set_fragment_buffer(
        &mut self,
        slot: usize,
        buffer: &ProtocolObject<dyn MTLBuffer>,
        offset: usize,
    ) {
        let Some(encoder) = &self.encoder else {
            return;
        };
        if slot == MATERIAL_SLOT {
            let id = identity(buffer);
            if self.material != id {
                self.stats.material_changes += 1;
                self.material = id;
            }
        }
        // SAFETY: as in `set_vertex_buffer`.
        unsafe { encoder.setFragmentBuffer_offset_atIndex(Some(buffer), offset, slot) };
    }

    /// Binds one constant block to both stages at `slot`.
    pub(crate) fn set_constants(
        &mut self,
        slot: usize,
        buffer: &ProtocolObject<dyn MTLBuffer>,
        offset: usize,
    ) {
        self.set_vertex_buffer(slot, buffer, offset);
        self.set_fragment_buffer(slot, buffer, offset);
        self.stats.root_cbv_binds += 1;
    }

    /// Binds `texture` to the fragment stage's `slot`, skipping a repeat.
    pub(crate) fn set_fragment_texture(
        &mut self,
        slot: usize,
        texture: &ProtocolObject<dyn MTLTexture>,
    ) {
        let Some(encoder) = &self.encoder else {
            return;
        };
        let id = identity(texture);
        if let Some(bound) = self.textures.get_mut(slot) {
            if *bound == id {
                return;
            }
            *bound = id;
        }
        // SAFETY: the texture is alive; Metal retains it for the command buffer's life.
        unsafe { encoder.setFragmentTexture_atIndex(Some(texture), slot) };
    }

    /// Draws `index_count` indices of `index_buffer` for `instances` instances, the first of
    /// which is `first_instance`. Nothing is recorded for an empty mesh.
    pub(crate) fn draw_indexed(
        &mut self,
        index_buffer: &ProtocolObject<dyn MTLBuffer>,
        index_count: u32,
        instances: u32,
        first_instance: u32,
    ) {
        if index_count == 0 || instances == 0 {
            return;
        }
        let Some(encoder) = &self.encoder else {
            return;
        };
        // SAFETY: the index buffer holds `index_count` `u32` indices from offset zero (the
        // mesh uploaded exactly that many), and the vertex stream the pipeline pulls from is
        // bound; the shaders read only inside the vertex buffer the indices were built for.
        unsafe {
            encoder.drawIndexedPrimitives_indexCount_indexType_indexBuffer_indexBufferOffset_instanceCount_baseVertex_baseInstance(
                MTLPrimitiveType::Triangle,
                index_count as usize,
                MTLIndexType::UInt32,
                index_buffer,
                0,
                instances as usize,
                0,
                first_instance as usize,
            );
        }
        self.stats.draw_calls += 1;
        self.stats.instances += u64::from(instances);
        self.stats.triangles += u64::from(index_count / 3) * u64::from(instances);
        if instances > 1 {
            self.stats.instanced_draws += 1;
        }
    }

    /// Draws `vertex_count` vertices as `primitive` for `instances` instances.
    pub(crate) fn draw(&mut self, primitive: MTLPrimitiveType, vertex_count: u32, instances: u32) {
        if vertex_count == 0 || instances == 0 {
            return;
        }
        let Some(encoder) = &self.encoder else {
            return;
        };
        // SAFETY: the vertex streams the bound pipeline pulls from were bound by the caller
        // and hold at least `vertex_count` (or `instances`) records.
        unsafe {
            encoder.drawPrimitives_vertexStart_vertexCount_instanceCount(
                primitive,
                0,
                vertex_count as usize,
                instances as usize,
            );
        }
        self.stats.draw_calls += 1;
        self.stats.instances += u64::from(instances);
        self.stats.triangles += u64::from(vertex_count / 3) * u64::from(instances);
        if instances > 1 {
            self.stats.instanced_draws += 1;
        }
    }
}

/// Adds `part`'s counters to `total`: the recording passes of one frame, each its own encoder,
/// reported as the one command list D3D12 would have recorded.
pub(crate) fn accumulate(total: &mut CommandStats, part: CommandStats) {
    total.draw_calls += part.draw_calls;
    total.instanced_draws += part.instanced_draws;
    total.instances += part.instances;
    total.triangles += part.triangles;
    total.pipeline_changes += part.pipeline_changes;
    total.redundant_pipeline_binds += part.redundant_pipeline_binds;
    total.root_cbv_binds += part.root_cbv_binds;
    total.redundant_root_cbv_binds += part.redundant_root_cbv_binds;
    total.material_changes += part.material_changes;
    total.mesh_changes += part.mesh_changes;
    total.vertex_buffer_binds += part.vertex_buffer_binds;
    total.redundant_vertex_buffer_binds += part.redundant_vertex_buffer_binds;
    total.index_buffer_binds += part.index_buffer_binds;
    total.redundant_index_buffer_binds += part.redundant_index_buffer_binds;
    total.barriers += part.barriers;
    total.barrier_flushes += part.barrier_flushes;
    total.redundant_transitions += part.redundant_transitions;
    total.copies += part.copies;
}
