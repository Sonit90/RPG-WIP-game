//! The batched game-UI pass: its pipeline, the atlas and font textures, and the map picture.
//!
//! One pipeline, three textures, one draw call. Everything the player sees that is not geometry
//! goes through here. The textures are bound at `ui.metal`'s slots: 0 the icon atlas, 1 the map
//! picture, 2 the font's distance field.

use super::library::Libraries;
use super::pipelines::{Blend, DepthRule, Pipeline, Raster, build_state};
use super::resources::{TextureFormat, texture_with_levels};
use super::{Device, Texture};
use crate::error::RendererError;
use crate::mipmap::Compression;
use crate::ui_atlas::UiAtlas;
use crate::ui_pipeline::{UiBatchStats, UiBatcher};

/// Most quads one frame of UI may emit; see the D3D12 renderer's constant of the same name.
pub(crate) const MAX_UI_QUADS: usize = 65_536;

/// Alignment for the UI vertex stream in the upload ring: the largest element, a `float4`.
pub(crate) const UI_VERTEX_ALIGNMENT: usize = 16;

/// Texture slot of the icon atlas.
pub(crate) const ATLAS_SLOT: usize = 0;
/// Texture slot of the map picture.
pub(crate) const MAP_SLOT: usize = 1;
/// Texture slot of the font's distance field.
pub(crate) const FONT_SLOT: usize = 2;

/// The UI pipeline: straight alpha blending, no depth test, no culling.
pub(crate) struct UiPipeline {
    pub(crate) pipeline: Pipeline,
}

impl UiPipeline {
    /// Builds it.
    ///
    /// # Errors
    ///
    /// As [`build_state`], or a shader error.
    pub(crate) fn new(device: &Device, libraries: &mut Libraries) -> Result<Self, RendererError> {
        let vertex = libraries.function("ui", "ui_vs")?;
        let fragment = libraries.function("ui", "ui_fs")?;
        Ok(Self {
            pipeline: Pipeline {
                state: build_state(device, &vertex, Some(&fragment), Blend::Straight, "ui")?,
                depth: DepthRule::Off,
                raster: Raster::DOUBLE_SIDED,
            },
        })
    }
}

/// The map picture on the GPU.
pub(crate) struct UiMapPicture {
    /// The texture `MapImage` commands sample.
    pub(crate) texture: Texture,
    /// Texels across and down.
    pub(crate) size: (u32, u32),
    /// Bytes of GPU memory, mips included.
    pub(crate) bytes: usize,
}

/// The batched game-UI pass.
pub(crate) struct UiState {
    pub(crate) pipeline: UiPipeline,
    /// Turns a `UiDrawList` into vertices. Owns its scratch across frames.
    pub(crate) batcher: UiBatcher,
    /// The icon atlas, uploaded once.
    pub(crate) atlas: Texture,
    /// The interface fonts' distance-field atlas, uploaded once.
    pub(crate) font: Texture,
    /// This frame's list, replaced by the application each frame.
    pub(crate) list: engine_render_data::UiDrawList,
    /// What the last batch produced.
    pub(crate) stats: UiBatchStats,
    /// Milliseconds the last batch build took, measured on the CPU.
    pub(crate) build_ms: f64,
    /// Whether the UI pass draws at all.
    pub(crate) enabled: bool,
    /// The level's map picture, if one is loaded.
    pub(crate) map: Option<UiMapPicture>,
}

impl UiState {
    /// Builds the pipeline and uploads the atlas and the font.
    ///
    /// # Errors
    ///
    /// Anything the pipeline or a texture upload can fail with.
    pub(crate) fn new(device: &Device, libraries: &mut Libraries) -> Result<Self, RendererError> {
        let pipeline = UiPipeline::new(device, libraries)?;
        let format = TextureFormat::new(Compression::None, false);
        let atlas_pixels = UiAtlas::new();
        let atlas = texture_with_levels(
            device,
            atlas_pixels.width(),
            atlas_pixels.height(),
            format,
            &[atlas_pixels.texels()],
        )?;
        let font = texture_with_levels(
            device,
            crate::text::ATLAS_WIDTH as u32,
            crate::text::ATLAS_HEIGHT as u32,
            format,
            &[&crate::text::atlas_rgba()],
        )?;
        Ok(Self {
            pipeline,
            batcher: UiBatcher::new(MAX_UI_QUADS),
            atlas,
            font,
            list: engine_render_data::UiDrawList::with_capacity(1024, 4096),
            stats: UiBatchStats::default(),
            build_ms: 0.0,
            enabled: true,
            map: None,
        })
    }
}
