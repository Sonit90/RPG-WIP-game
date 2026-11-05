//! The blended particle pass: two pipelines and the per-frame list.
//!
//! Particles have no vertex stream of geometry. The records are bound as the per-instance
//! stream at slot 16, and the quad's corner comes from `[[vertex_id]]` of a four-vertex
//! triangle strip: one draw per blend mode, whatever the particle count. Depth is tested
//! against the scene and never written; the quads are not culled.

use super::Device;
use super::library::Libraries;
use super::pipelines::{Blend, DepthRule, Pipeline, Raster, build_state};
use crate::error::RendererError;
use crate::particle_pipeline::ParticleConstants;
use engine_render_data::ParticleBlend;

/// Alignment for the particle record stream in the upload ring: sixteen, not the 36-byte
/// record, which is not a power of two (see the D3D12 renderer's constant of the same name).
pub(crate) const PARTICLE_ALIGNMENT: usize = 16;

/// The additive and the alpha pipelines.
pub(crate) struct ParticlePipelines {
    additive: Pipeline,
    alpha: Pipeline,
}

impl ParticlePipelines {
    /// Builds both.
    ///
    /// # Errors
    ///
    /// As [`build_state`], or a shader error.
    pub(crate) fn new(device: &Device, libraries: &mut Libraries) -> Result<Self, RendererError> {
        let vertex = libraries.function("particle", "particle_vs")?;
        let fragment = libraries.function("particle", "particle_fs")?;
        let make = |blend, what| {
            Ok(Pipeline {
                state: build_state(device, &vertex, Some(&fragment), blend, what)?,
                depth: DepthRule::Read,
                raster: Raster::DOUBLE_SIDED,
            })
        };
        Ok(Self {
            additive: make(Blend::Additive, "particles (additive)")?,
            alpha: make(Blend::Premultiplied, "particles (alpha)")?,
        })
    }

    /// The pipeline for `blend`.
    pub(crate) fn get(&self, blend: ParticleBlend) -> &Pipeline {
        match blend {
            ParticleBlend::Additive => &self.additive,
            ParticleBlend::Alpha => &self.alpha,
        }
    }
}

/// The blended particle pass, owned by the renderer.
pub(crate) struct ParticleState {
    pub(crate) pipelines: ParticlePipelines,
    pub(crate) list: engine_render_data::ParticleDrawList,
    /// This frame's camera basis, captured where the camera is in hand.
    pub(crate) constants: ParticleConstants,
    /// Particles submitted by the most recent frame.
    pub(crate) drawn: u32,
    /// Whether the pass draws at all.
    pub(crate) enabled: bool,
}
