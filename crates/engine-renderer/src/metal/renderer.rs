//! Renderer front end for Metal: owns the device, the layer, the resources and the frame
//! lifecycle. The passes themselves are recorded by `draw.rs`.
//!
//! This is the Metal twin of `crate::renderer::Renderer` and offers the same public API with
//! the same meaning. Where the two differ it is because the GPU API does; the notes on those
//! methods say how.
//!
//! # A frame
//!
//! ```text
//! render* / draw_render_list   waits for the slot, acquires a drawable, encodes the camera
//!                              pass into one MTLCommandBuffer, and leaves it uncommitted
//! present                      presentDrawable + commit; the completion handler paces the slot
//! ```
//!
//! Metal presents through the command buffer, so the buffer is committed by `present` and not
//! by the `render` that encoded it: the application's `render -> present` shape is kept, and a
//! frame that is never presented (`capture_world`, a resize) is simply never committed.

use std::collections::HashMap;
use std::ptr::NonNull;
use std::time::Instant;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{
    MTLClearColor, MTLCommandBuffer, MTLCommandBufferStatus, MTLCommandQueue, MTLLoadAction,
    MTLRenderPassDescriptor, MTLStoreAction,
};
use objc2_quartz_core::CAMetalDrawable;

use engine_core::math::Frustum;
use engine_core::time::{FrameStats, FrameStatsWindow};
use engine_render_data::{Camera, Lighting, LocalLight, MeshHandle, RenderWorld, SkinnedDrawList};

use super::device::MetalDevice;
use super::draw::{FramePalettes, may_be_dithered};
use super::frames::FrameRing;
use super::layer::MetalLayer;
use super::library::Libraries;
use super::particles::{ParticlePipelines, ParticleState};
use super::pipelines::{DepthStates, MeshPipelines, TrianglePipeline};
use super::readback::Readback;
use super::recorder::Recorder;
use super::resources::{DynamicMesh, GpuMesh, buffer_with_data, depth_texture};
use super::shadow::ShadowStage;
use super::ui::UiState;
use super::upload::{Allocation, UploadRing};
use super::{Buffer, CommandBuffer, Texture, api_error};
use crate::command::CommandStats;
use crate::culling::{CullStats, Culler};
use crate::depth::DEPTH_CLEAR;
use crate::device::{BACKEND, VideoMemory};
use crate::error::RendererError;
use crate::frame::SyncStats;
use crate::gpu_timer::MAX_TIMED_PASSES;
use crate::light_cluster::{LightClusterStats, LightClusters};
use crate::material::{MaterialDesc, MaterialTextures, SurfaceClass};
use crate::mesh_pipeline::InstanceTransform;
use crate::mipmap::Compression;
use crate::particle_pipeline::ParticleConstants;
use crate::pipeline::Vertex;
use crate::render_list::RenderList;
use crate::shader::ShaderFeatures;
use crate::shadow::{ShadowAtlasLayout, ShadowPlan, ShadowSettings, ShadowStats};
use crate::stress;
use crate::texture::TextureHandle;
use crate::texture_id::TextureKey;
use crate::{AdapterInfo, RendererConfig};

#[cfg(feature = "debug-overlay")]
use super::overlay::OverlayState;
#[cfg(feature = "debug-overlay")]
use crate::overlay::OverlayMetrics;

/// Frames of `Present` history kept for its distribution; the engine's frame-metrics window.
const PRESENT_WINDOW: usize = engine_core::metrics::DEFAULT_HISTORY;

/// A texture on the GPU and how it was stored.
pub(super) struct UploadedTexture {
    /// How this texture is compressed on the GPU, if it is.
    pub(super) compression: Compression,
    /// The texture, in shared storage. Its pixel format carries the colour space.
    pub(super) texture: Texture,
    /// Mip levels, the whole chain.
    pub(super) mip_levels: u32,
}

/// A material's constants on the GPU.
pub(super) struct MetalMaterial {
    /// The 48-byte `MaterialConstants`, bound at fragment slot 2 and never written again.
    pub(super) constants: Buffer,
}

/// A block Metal calls with a command buffer that has finished.
type CompletionHandler = RcBlock<dyn Fn(NonNull<ProtocolObject<dyn MTLCommandBuffer>>)>;

/// A frame being encoded: its command buffer, the drawable it renders into and its slot.
pub(super) struct FrameInProgress {
    pub(super) command_buffer: CommandBuffer,
    pub(super) drawable: Retained<ProtocolObject<dyn CAMetalDrawable>>,
    pub(super) slot: usize,
}

/// Metal renderer.
///
/// Owns every GPU object it creates. Fields drop in declaration order, but Metal objects are
/// reference counted and retained by the command buffers that use them, so unlike on D3D12 the
/// order is not load-bearing; `Drop` still waits for the GPU first.
pub struct Renderer {
    /// What the camera fades out of its way this frame. See [`Renderer::set_camera_fade`].
    pub(super) camera_fade: Option<engine_render_data::CameraFade>,
    /// Draws the translucent-character passes recorded last frame, both passes counted.
    pub(super) translucent_character_draws_last_frame: u32,
    /// What construction cost, for the application's startup report.
    #[cfg(feature = "startup-timing")]
    startup: RendererStartup,
    /// Frame slots and the serials that pace them.
    pub(super) frames: FrameRing,
    /// Per-frame upload memory.
    pub(super) uploads: UploadRing,
    /// The layer frames are presented through.
    pub(super) layer: MetalLayer,
    /// What the most recent frame's recording cost, every pass counted.
    pub(super) command_stats: CommandStats,
    /// What the frame being recorded has counted so far, from the passes already ended.
    pub(super) frame_stats: CommandStats,
    /// The open pass's encoder: the shadow stage's passes, then the camera pass.
    pub(super) recorder: Recorder,
    /// The frame being encoded, between `begin_frame` and `finish_recording`.
    pub(super) recording: Option<FrameInProgress>,
    /// The frame encoded and waiting for `present` (or `capture_world`) to commit it.
    pub(super) recorded: Option<FrameInProgress>,
    /// Most recent GPU frame time in milliseconds.
    pub(super) last_gpu_frame_ms: Option<f64>,
    /// Shader libraries, loaded as pipelines need them.
    pub(super) libraries: Libraries,
    /// The depth-stencil states every pass picks from.
    pub(super) depth_states: DepthStates,
    /// Every mesh pipeline state in use.
    pub(super) pipelines: MeshPipelines,
    /// The position + colour pipeline: the debug overlay and the stress grid.
    pub(super) triangle: TrianglePipeline,
    /// Depth target, the size of the drawable.
    pub(super) depth: Texture,
    /// The shadow atlas, the contact-shadow target and the pipelines that fill them.
    pub(super) shadow_stage: ShadowStage,
    /// Whether `render_world` draws through the wireframe pipeline.
    pub(super) wireframe: bool,
    /// Meshes created through [`Renderer::create_mesh`], indexed by handle.
    pub(super) meshes: Vec<GpuMesh>,
    /// Skinned meshes, indexed by handle.
    pub(super) skinned_meshes: Vec<GpuMesh>,
    /// Textures created through [`Renderer::create_texture`], indexed by handle.
    pub(super) textures: Vec<UploadedTexture>,
    /// Which uploaded texture holds a given image, keyed by content. See D3D12's field.
    pub(super) textures_by_content: HashMap<TextureKey, TextureHandle>,
    /// Bytes of GPU memory the resident textures occupy, mip chains and compression included.
    pub(super) texture_bytes: usize,
    /// What the same images would have cost as uncompressed RGBA8 with no deduplication.
    pub(super) texture_bytes_naive: usize,
    /// How many times a texture was asked for, including the ones the table answered.
    pub(super) texture_requests: usize,
    /// The four shared 1x1 fallback maps, one per [`MaterialTextureSlot`].
    pub(super) fallback_textures: MaterialTextures,
    /// Materials created through [`Renderer::create_material`], indexed by handle.
    pub(super) materials: Vec<MetalMaterial>,
    /// Which shader permutation each material asks for, indexed by handle.
    pub(super) material_features: Vec<ShaderFeatures>,
    /// Each material's parameters, so an identical request is answered with the same material.
    pub(super) material_descs: Vec<MaterialDesc>,
    /// Which phase each material is drawn in, indexed by handle.
    pub(super) material_surfaces: Vec<SurfaceClass>,
    /// Each material's texture set, indexed by handle.
    pub(super) material_textures: Vec<MaterialTextures>,
    /// Meshes replaced by [`Renderer::replace_mesh`], kept until the GPU has passed the last
    /// frame that could have drawn them.
    pub(super) retired_meshes: Vec<(u64, GpuMesh)>,
    /// The CPU side of each dynamic mesh, keyed by its index in `meshes`.
    pub(super) dynamic_meshes: HashMap<usize, DynamicMesh>,
    /// Objects the last frame's transparent phase drew.
    pub(super) transparent_draws_last_frame: u32,
    /// The shadow stage's CPU half: settings, cascades, caster lists, light selection.
    pub(super) shadow: ShadowPlan,
    /// Where each shadow view lives in the atlas.
    pub(super) shadow_layout: ShadowAtlasLayout,
    /// This frame's local lights.
    pub(super) lights: Vec<LocalLight>,
    /// This frame's clustered light grid.
    pub(super) clusters: LightClusters,
    /// Whether the clustered grid is built at all.
    light_clustering: bool,
    /// Whether this frame's local lights are shaded at all.
    local_lights_enabled: bool,
    /// This frame's bone palettes, each uploaded once and bound by every pass.
    pub(super) frame_palettes: FramePalettes,
    /// This frame's skinned draws, unculled, for the shadow passes.
    pub(super) shadow_skinned: SkinnedDrawList,
    /// Frustum culling: the visibility policy and what the last pass counted.
    culler: Culler,
    /// This frame's render list.
    pub(super) visible: RenderList,
    /// This frame's transparent draws, sorted farthest-first.
    pub(super) transparent: RenderList,
    /// This frame's opaque draws the camera may dither.
    pub(super) dithered: RenderList,
    /// Whether the camera pass may draw prepass-covered surfaces without writing depth.
    pub(super) depth_priming: bool,
    /// Whether this frame's depth prepass ran, so camera draws can be primed.
    pub(super) depth_primed: bool,
    /// This frame's instance stream for `visible`, uploaded before the passes that read it.
    pub(super) camera_instances: Option<Allocation>,
    /// Whether `render_world` culls.
    culling_enabled: bool,
    /// A frustum to cull against instead of the camera's own.
    cull_view_projection: Option<engine_core::math::Mat4>,
    /// Milliseconds the most recent cull pass took.
    last_cull_ms: f64,
    /// Per-chunk render lists for a parallel cull.
    cull_scratch: Vec<RenderList>,
    /// Milliseconds the most recent grouping pass took.
    last_group_ms: f64,
    /// Whether each batch is drawn as one instanced call.
    pub(super) instancing: bool,
    /// Per-batch instance data, staged here before the copy into the upload ring.
    pub(super) instance_scratch: Vec<InstanceTransform>,
    /// This frame's visible skinned draws.
    pub(super) skinned: engine_render_data::SkinnedDrawList,
    /// One character's bone palette, padded to the shader's array.
    pub(super) palette_scratch: Vec<[f32; 16]>,
    /// Skinned draws recorded by the most recent frame.
    pub(super) skinned_draws_last_frame: u32,
    /// Milliseconds the most recent `present` took, drawable acquisition included.
    last_present_ms: f64,
    /// Milliseconds the most recent frame spent waiting for a drawable.
    pub(super) last_acquire_ms: f64,
    /// Milliseconds the most recent commit bookkeeping took.
    last_fence_signal_ms: f64,
    /// A window of `present` durations.
    present_window: FrameStatsWindow,
    /// The stress scene: a grid of position + colour triangles.
    pub(super) scene: Buffer,
    /// Vertices in `scene`.
    pub(super) scene_vertices: u32,
    /// Scratch for regenerating the stress scene without allocating each time.
    scene_scratch: Vec<Vertex>,
    /// Debug overlay geometry and its per-slot buffers.
    #[cfg(feature = "debug-overlay")]
    pub(super) overlay: OverlayState,
    /// The batched game-UI pass.
    pub(super) ui: UiState,
    /// The blended particle pass.
    pub(super) particles: ParticleState,
    frames_presented: u64,
    /// Draw calls recorded by the most recent frame.
    pub(super) draw_calls_last_frame: u32,
    /// Triangles submitted by the most recent frame.
    pub(super) triangles_last_frame: u64,
    /// Whether an upload batch is open, and how many resources it has queued.
    pub(super) batch: Option<usize>,
    /// Where the frame being recorded is copied to, for [`Renderer::capture_world`].
    pub(super) capture: Option<Readback>,
    /// Where the next ordinary frame is copied to; see [`Renderer::request_screenshot`].
    pub(super) screenshot: Option<Readback>,
    /// Device and queue.
    pub(super) context: MetalDevice,
}

// SAFETY: a `Renderer` is used by one thread at a time (it is not `Sync`). Every Objective-C
// object it holds is a Metal object — device, queue, buffers, textures, pipeline states, the
// layer — which Metal documents as usable from any thread, and the completion handlers it
// installs touch only an `Arc<Mutex<..>>`. The raw pointers inside the upload ring address
// shared GPU memory that only this renderer writes.
unsafe impl Send for Renderer {}

/// What [`Renderer::new`] spent its time on, in milliseconds.
///
/// Zero unless the `startup-timing` feature is on, which is how a shipping build carries no
/// measurement: without the feature nothing writes these and nothing reads them.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RendererStartup {
    /// Creating the Metal device, its queue and the layer.
    pub device_ms: f64,
    /// Everything after that: pipelines, buffers, the UI atlas.
    pub objects_ms: f64,
}

impl Renderer {
    /// Attaches a `CAMetalLayer` to the window and creates the Metal renderer for it.
    ///
    /// `window_handle` is the raw handle the platform layer returns, which on macOS is the
    /// address of the window's `NSView`; the renderer takes it as an opaque integer so it does
    /// not depend on the platform crate. The view must outlive the renderer, and this should be
    /// called on the main thread, because it installs the layer on the view.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] for a zero-sized or handle-less request,
    /// [`RendererError::NoSuitableAdapter`] if there is no Apple silicon Metal device,
    /// [`RendererError::ShaderCompilation`] if a shader does not compile, or
    /// [`RendererError::Api`] if a Metal call fails.
    pub fn new(window_handle: isize, config: &RendererConfig) -> Result<Self, RendererError> {
        if window_handle == 0 {
            return Err(RendererError::InvalidConfig {
                reason: "window handle is null".to_owned(),
            });
        }
        if config.width == 0 || config.height == 0 {
            return Err(RendererError::InvalidConfig {
                reason: format!(
                    "resolution {}x{} has a zero dimension",
                    config.width, config.height
                ),
            });
        }

        #[cfg(feature = "startup-timing")]
        let device_started = Instant::now();
        let context = MetalDevice::new()?;
        let device = context.device().clone();
        let layer = MetalLayer::new(
            window_handle,
            &device,
            config.width,
            config.height,
            config.vsync,
        )?;
        #[cfg(feature = "startup-timing")]
        let device_ms = device_started.elapsed().as_secs_f64() * 1e3;

        let frames = FrameRing::new(config.frames_in_flight);
        let frames_in_flight = frames.frames_in_flight();
        let uploads = UploadRing::new(&device, config.upload_bytes_per_frame, frames_in_flight)?;
        let mut libraries = Libraries::new(&device);
        let depth_states = DepthStates::new(&device)?;
        let triangle = TrianglePipeline::new(&device, &mut libraries)?;
        let depth = depth_texture(&device, config.width, config.height)?;
        let mut scene_scratch = Vec::new();
        stress::build_grid(1, &mut scene_scratch);
        let scene = buffer_with_data(&device, &scene_scratch, "the stress scene")?;
        let scene_vertices = scene_scratch.len() as u32;
        let ui = UiState::new(&device, &mut libraries)?;
        let particle_pipelines = ParticlePipelines::new(&device, &mut libraries)?;

        let mut shadow = ShadowPlan::new(ShadowSettings::default().validated());
        // The camera pass is drawn primed by default, as on D3D12.
        shadow.set_priming_prepass(true);
        let shadow_layout = ShadowAtlasLayout::new(shadow.settings());
        let shadow_stage = ShadowStage::new(
            &device,
            &mut libraries,
            shadow.settings(),
            (config.width, config.height),
        )?;

        let mut renderer = Self {
            camera_fade: None,
            translucent_character_draws_last_frame: 0,
            #[cfg(feature = "startup-timing")]
            startup: RendererStartup {
                device_ms,
                objects_ms: device_started.elapsed().as_secs_f64() * 1e3 - device_ms,
            },
            frames,
            uploads,
            layer,
            command_stats: CommandStats::default(),
            frame_stats: CommandStats::default(),
            recorder: Recorder::default(),
            recording: None,
            recorded: None,
            last_gpu_frame_ms: None,
            libraries,
            depth_states,
            pipelines: MeshPipelines::new(),
            triangle,
            depth,
            shadow_stage,
            wireframe: false,
            meshes: Vec::new(),
            skinned_meshes: Vec::new(),
            textures: Vec::new(),
            textures_by_content: HashMap::new(),
            texture_bytes: 0,
            texture_bytes_naive: 0,
            texture_requests: 0,
            // Replaced immediately below, once there is a `self` to upload through. Every
            // slot names texture 0, which the constructor then creates as the white texel.
            fallback_textures: MaterialTextures::uniform(TextureHandle(0)),
            materials: Vec::new(),
            material_features: Vec::new(),
            material_descs: Vec::new(),
            material_surfaces: Vec::new(),
            material_textures: Vec::new(),
            retired_meshes: Vec::new(),
            dynamic_meshes: HashMap::new(),
            transparent_draws_last_frame: 0,
            shadow,
            shadow_layout,
            lights: Vec::new(),
            clusters: LightClusters::new(),
            light_clustering: true,
            local_lights_enabled: true,
            frame_palettes: FramePalettes::new(),
            shadow_skinned: SkinnedDrawList::new(),
            culler: Culler::new(),
            visible: RenderList::new(),
            transparent: RenderList::new(),
            dithered: RenderList::new(),
            depth_priming: true,
            depth_primed: false,
            camera_instances: None,
            culling_enabled: true,
            cull_view_projection: None,
            last_cull_ms: 0.0,
            cull_scratch: Vec::new(),
            last_group_ms: 0.0,
            instancing: true,
            instance_scratch: Vec::new(),
            skinned: engine_render_data::SkinnedDrawList::new(),
            palette_scratch: Vec::new(),
            skinned_draws_last_frame: 0,
            last_present_ms: 0.0,
            last_acquire_ms: 0.0,
            last_fence_signal_ms: 0.0,
            present_window: FrameStatsWindow::new(PRESENT_WINDOW),
            scene,
            scene_vertices,
            scene_scratch,
            #[cfg(feature = "debug-overlay")]
            overlay: OverlayState::new(frames_in_flight),
            ui,
            particles: ParticleState {
                pipelines: particle_pipelines,
                list: engine_render_data::ParticleDrawList::default(),
                constants: ParticleConstants::default(),
                drawn: 0,
                enabled: true,
            },
            frames_presented: 0,
            draw_calls_last_frame: 0,
            triangles_last_frame: 0,
            batch: None,
            capture: None,
            screenshot: None,
            context,
        };
        // The first texture, so an untextured material has something to sample.
        renderer.fallback_textures = renderer.create_fallback_textures()?;
        debug_assert_eq!(
            renderer.fallback_textures.base_color.index(),
            0,
            "the white texel is the placeholder the struct literal assumed"
        );
        renderer.log_startup_info();
        Ok(renderer)
    }

    /// How long the two halves of [`Renderer::new`] took, in milliseconds.
    #[must_use]
    pub fn startup(&self) -> RendererStartup {
        #[cfg(feature = "startup-timing")]
        {
            self.startup
        }
        #[cfg(not(feature = "startup-timing"))]
        {
            RendererStartup::default()
        }
    }

    /// Logs the adapter, feature set, backend and resolution.
    fn log_startup_info(&self) {
        let info = self.context.adapter_info();
        let (width, height) = self.layer.size();
        log::info!("GPU: {}", info.name);
        log::info!(
            "GPU memory budget (unified): {} MiB",
            info.dedicated_video_memory_mib()
        );
        log::info!("feature level: {}", self.context.feature_level_name());
        log::info!("backend: {BACKEND}");
        log::info!("resolution: {width}x{height}");
        log::info!("presentation: vsync={}", self.layer.vsync());
    }

    /// Description of the adapter in use.
    #[must_use]
    pub fn adapter_info(&self) -> &AdapterInfo {
        self.context.adapter_info()
    }

    /// Video memory this process is using: the device's allocated size against its recommended
    /// working set. Unified memory, so there is no non-local figure.
    #[must_use]
    pub fn video_memory(&self) -> Option<VideoMemory> {
        Some(self.context.video_memory())
    }

    /// Current drawable size in pixels.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        self.layer.size()
    }

    /// Number of frames presented since initialization.
    #[must_use]
    pub fn frames_presented(&self) -> u64 {
        self.frames_presented
    }

    /// Draw calls recorded by the most recent frame.
    #[must_use]
    pub fn draw_calls_last_frame(&self) -> u32 {
        self.draw_calls_last_frame
    }

    /// Triangles submitted by the most recent frame.
    #[must_use]
    pub fn triangles_last_frame(&self) -> u64 {
        self.triangles_last_frame
    }

    /// Most recent GPU frame time in milliseconds, from the command buffer's GPU timestamps.
    ///
    /// `None` until a frame has completed. Published by the completion handler, so it lags the
    /// CPU frame by the frames in flight, exactly as the D3D12 figure does.
    #[must_use]
    pub fn last_gpu_frame_ms(&self) -> Option<f64> {
        self.last_gpu_frame_ms
    }

    /// The same frame's GPU time, split by pass.
    ///
    /// Always `None` on Metal: per-pass times need counter sample buffers around each encoder,
    /// which this backend does not use. The frame total above is available.
    #[must_use]
    pub fn last_pass_ms(&self) -> [Option<f64>; MAX_TIMED_PASSES] {
        [None; MAX_TIMED_PASSES]
    }

    /// Whether the camera pass may draw what the depth prepass covered without writing depth.
    ///
    /// Takes effect where a prepass runs; see [`crate::mesh_pipeline::DepthMode::Primed`].
    pub fn set_depth_priming(&mut self, enabled: bool) {
        self.depth_priming = enabled;
    }

    /// Whether the camera depth prepass also runs where contact shadows do not need it, purely
    /// so the camera pass can be drawn primed.
    pub fn set_priming_prepass(&mut self, enabled: bool) {
        self.shadow.set_priming_prepass(enabled);
    }

    /// Turns the shadow views' static-caster cache on or off, with the cascade margin as a
    /// fraction of each cascade's radius. See `shadow::static_cache`.
    pub fn set_static_shadow_cache(&mut self, enabled: bool, margin_fraction: f32) {
        self.shadow.set_static_cache(enabled, margin_fraction);
    }

    /// Whether the last cascade draws movers and characters: a measured trade-off, on by
    /// default.
    pub fn set_far_shadow_movers(&mut self, enabled: bool) {
        self.shadow.set_far_movers(enabled);
    }

    /// Turns the mesh pass's pipeline-statistics query on or off.
    ///
    /// # Errors
    ///
    /// Metal has no pipeline-statistics query: asking for it returns
    /// [`RendererError::InvalidConfig`]. Turning it off always succeeds.
    pub fn set_pipeline_statistics(&mut self, enabled: bool) -> Result<(), RendererError> {
        if enabled {
            return Err(RendererError::InvalidConfig {
                reason: "pipeline statistics are not available on Metal".into(),
            });
        }
        Ok(())
    }

    /// The most recent mesh pass's pipeline statistics. Always `None` on Metal.
    #[must_use]
    pub fn last_scene_statistics(&self) -> Option<crate::pipeline_stats::SceneStatistics> {
        None
    }

    /// Timestamp ticks per second: GPU times are reported in nanoseconds.
    #[must_use]
    pub fn timestamp_frequency(&self) -> u64 {
        1_000_000_000
    }

    /// Triangles in the current stress scene.
    #[must_use]
    pub fn scene_triangle_count(&self) -> u32 {
        self.scene_vertices / 3
    }

    /// Replaces the stress scene with a grid of `triangles` triangles.
    ///
    /// Waits for the GPU to go idle first, because the buffer it is currently reading is about
    /// to be released. A scene change, not a per-frame operation.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] if `triangles` is zero, or
    /// [`RendererError::Api`] if the upload fails.
    pub fn set_scene_triangles(&mut self, triangles: u32) -> Result<(), RendererError> {
        if triangles == 0 {
            return Err(RendererError::InvalidConfig {
                reason: "a scene needs at least one triangle".to_owned(),
            });
        }
        if triangles == self.scene_triangle_count() {
            return Ok(());
        }
        self.wait_for_idle()?;
        self.abandon_frames();

        stress::build_grid(triangles, &mut self.scene_scratch);
        self.scene = buffer_with_data(
            self.context.device(),
            &self.scene_scratch,
            "the stress scene",
        )?;
        self.scene_vertices = self.scene_scratch.len() as u32;
        log::info!(
            "scene: {triangles} triangles ({} vertices, {} KiB)",
            self.scene_vertices,
            self.scene_vertices as usize * size_of::<Vertex>() / 1024
        );
        Ok(())
    }

    /// Replaces the overlay text, one entry per line.
    ///
    /// Call before [`Renderer::render`]; the geometry is rebuilt here rather than per frame,
    /// so callers should only pass new text when it changes. Text beyond the overlay's reserved
    /// capacity is dropped rather than allocated for.
    #[cfg(feature = "debug-overlay")]
    pub fn set_overlay_text<S: AsRef<str>>(&mut self, lines: &[S]) {
        let (width, height) = self.layer.size();
        self.overlay.geometry.begin();
        for (index, line) in lines.iter().enumerate() {
            self.overlay
                .geometry
                .push_line(index, line.as_ref(), width, height);
        }
        // Every slot's buffer now holds the previous text.
        self.overlay.stale.fill(true);
    }

    /// Appends an opaque backdrop behind the overlay text.
    ///
    /// Call *after* [`Renderer::set_overlay_text`], which rebuilds the geometry this appends
    /// to. `columns` is the longest line's length.
    #[cfg(feature = "debug-overlay")]
    pub fn push_overlay_backdrop(&mut self, lines: usize, columns: usize) {
        let (width, height) = self.layer.size();
        self.overlay
            .geometry
            .push_backdrop(lines, columns, width, height);
        self.overlay.stale.fill(true);
    }

    /// Appends a frame-time graph below the overlay text.
    ///
    /// Call *after* [`Renderer::set_overlay_text`]. `samples` are milliseconds, oldest first,
    /// and `budget_ms` is the height the panel is scaled to.
    #[cfg(feature = "debug-overlay")]
    pub fn push_overlay_frame_graph(&mut self, samples: &[f32], budget_ms: f32) {
        /// Distance from the left edge, matching the text margin.
        const LEFT_PIXELS: f32 = 12.0;
        /// Distance from the bottom edge.
        const BOTTOM_MARGIN_PIXELS: f32 = 16.0;
        /// Panel size in pixels.
        const PANEL: (f32, f32) = (360.0, 90.0);

        let (width, height) = self.layer.size();
        self.overlay.geometry.push_frame_graph(
            samples,
            budget_ms,
            (LEFT_PIXELS, height as f32 - BOTTOM_MARGIN_PIXELS),
            PANEL,
            width,
            height,
        );
        self.overlay.stale.fill(true);
    }

    /// Discards the previous frame's dynamic overlay geometry.
    ///
    /// Call once a frame before pushing nameplates. The developer HUD is not affected.
    #[cfg(feature = "debug-overlay")]
    pub fn begin_dynamic_overlay(&mut self) {
        self.overlay.dynamic.begin();
    }

    /// Appends text to the dynamic layer, positioned in pixels from the top-left.
    ///
    /// `depth` orders overlapping elements: the overlay shares the scene's depth test, so the
    /// smaller depth wins wherever two quads cover the same pixel.
    #[cfg(feature = "debug-overlay")]
    pub fn push_overlay_text_at(
        &mut self,
        origin: (f32, f32),
        text: &str,
        color: [f32; 4],
        depth: f32,
    ) {
        let target = self.layer.size();
        self.overlay
            .dynamic
            .push_text_at(origin, text, color, depth, target);
    }

    /// Appends a filled rectangle to the dynamic layer, in pixels from the top-left.
    #[cfg(feature = "debug-overlay")]
    pub fn push_overlay_rect(&mut self, rect: (f32, f32, f32, f32), color: [f32; 4], depth: f32) {
        let target = self.layer.size();
        self.overlay.dynamic.push_rect(rect, color, depth, target);
    }

    /// Appends a hollow rectangle to the dynamic layer, in pixels from the top-left.
    #[cfg(feature = "debug-overlay")]
    pub fn push_overlay_outline(
        &mut self,
        rect: (f32, f32, f32, f32),
        thickness: f32,
        color: [f32; 4],
        depth: f32,
    ) {
        let target = self.layer.size();
        self.overlay
            .dynamic
            .push_outline(rect, thickness, color, depth, target);
    }

    /// Pixel metrics of the overlay font, for a caller sizing a box around its own text.
    #[cfg(feature = "debug-overlay")]
    #[must_use]
    pub fn overlay_metrics(&self) -> OverlayMetrics {
        OverlayMetrics {
            cell_pixels: self.overlay.dynamic.cell_pixels(),
            glyph_height_pixels: self.overlay.dynamic.glyph_height_pixels(),
            line_advance_pixels: self.overlay.dynamic.line_advance_pixels(),
            glyph_advance_pixels: self.overlay.dynamic.text_width_pixels(1),
        }
    }

    /// Turns overlay drawing on or off at runtime.
    #[cfg(feature = "debug-overlay")]
    pub fn set_overlay_enabled(&mut self, enabled: bool) {
        self.overlay.enabled = enabled;
    }

    /// Whether the overlay is currently drawn.
    #[cfg(feature = "debug-overlay")]
    #[must_use]
    pub fn overlay_enabled(&self) -> bool {
        self.overlay.enabled
    }

    /// Vertices the overlay drew in the most recent frame, for tests and diagnostics.
    #[cfg(feature = "debug-overlay")]
    #[must_use]
    pub fn overlay_vertex_count(&self) -> u32 {
        self.overlay.drawn_vertices
    }

    /// Name of the Metal feature set in use: `Metal 3`, or `Metal` on an older family.
    #[must_use]
    pub fn feature_level_name(&self) -> &'static str {
        self.context.feature_level_name()
    }

    /// Records a frame that clears the drawable and draws the stress triangles.
    ///
    /// Does **not** present: [`Renderer::present`] is a separate step so the application loop
    /// keeps the `process_events -> update -> render -> present` shape.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if no drawable or command buffer can be had.
    pub fn render(&mut self, color: [f32; 4]) -> Result<(), RendererError> {
        objc2::rc::autoreleasepool(|_| {
            let slot = self.begin_recording(color)?;
            self.draw_stress_scene();
            self.finish_frame(slot)
        })
    }

    /// Records a frame that draws a [`RenderWorld`] through a [`Camera`].
    ///
    /// Two steps, and they are separate on purpose:
    ///
    /// ```text
    /// build_render_list()  ->  what is visible   (CPU only)
    /// draw_render_list()   ->  record it         (Metal only, no visibility logic)
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] if the world names a mesh or material that
    /// was never created, or [`RendererError::Api`] if recording fails.
    pub fn render_world(
        &mut self,
        world: &RenderWorld,
        camera: &Camera,
        lighting: Lighting,
        color: [f32; 4],
    ) -> Result<(), RendererError> {
        self.build_render_list(world, camera);
        self.prepare_shadows(world, camera, lighting);
        self.draw_render_list(camera, lighting, color)
    }

    /// [`Renderer::render_world`] with the render list built across `jobs`' workers.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError`] for the same reasons `render_world` does. A failed chunk
    /// is not one of them: it falls back to a serial cull and the frame still draws.
    pub fn render_world_parallel(
        &mut self,
        world: &RenderWorld,
        camera: &Camera,
        lighting: Lighting,
        color: [f32; 4],
        jobs: &engine_core::jobs::JobSystem,
    ) -> Result<(), RendererError> {
        self.build_render_list_with(world, camera, Some(jobs));
        self.prepare_shadows(world, camera, lighting);
        self.draw_render_list(camera, lighting, color)
    }

    /// Rebuilds [`Renderer::render_list`] from the entities of `world` that `camera` sees.
    ///
    /// Pure CPU: this touches no Metal object. Reuses the list's allocation, so after the
    /// first few frames this allocates nothing.
    pub fn build_render_list(&mut self, world: &RenderWorld, camera: &Camera) {
        self.build_render_list_with(world, camera, None);
    }

    /// Rebuilds the render list, optionally across `jobs`' workers.
    ///
    /// `None` culls on the calling thread. `Some` chunks the world and merges the chunks in
    /// world order, producing a list identical to the serial one.
    pub fn build_render_list_with(
        &mut self,
        world: &RenderWorld,
        camera: &Camera,
        jobs: Option<&engine_core::jobs::JobSystem>,
    ) {
        let frustum = Frustum::from_view_projection(
            &self
                .cull_view_projection
                .unwrap_or_else(|| camera.view_projection()),
        );
        // Timed around the build alone, not the whole frame.
        let started = Instant::now();
        match (self.culling_enabled, jobs) {
            (false, _) => self.culler.accept_all(world, &mut self.visible),
            (true, None) => self.culler.cull(world, &frustum, &mut self.visible),
            (true, Some(jobs)) => {
                let result = self.culler.cull_parallel(
                    world,
                    &frustum,
                    jobs,
                    &mut self.cull_scratch,
                    &mut self.visible,
                );
                if let Err(error) = result {
                    // A half-filled list would draw a half-finished scene.
                    log::error!("parallel cull failed, falling back to serial: {error}");
                    self.culler.cull(world, &frustum, &mut self.visible);
                }
            }
        }
        self.last_cull_ms = started.elapsed().as_secs_f64() * 1e3;

        // Grouping is timed with the cull rather than with recording: it is list-building work.
        let grouping = Instant::now();
        // Before the grouping, deliberately: the opaque phase's batches must contain exactly
        // what they contained before transparency existed.
        self.visible
            .split_transparent(&self.material_surfaces, &mut self.transparent);
        // Whatever the camera may dither this frame leaves the list the depth prepass draws.
        let fade = self.camera_fade;
        let eye = camera.position();
        let bounds = world.bounds();
        self.visible
            .split_where(&mut self.dithered, |entity, style| {
                may_be_dithered(style, fade, eye, bounds.get(entity.index()))
            });
        self.visible
            .group_by_material_and_mesh(self.materials.len(), self.meshes.len());
        self.dithered
            .group_by_material_and_mesh(self.materials.len(), self.meshes.len());
        // Sorted from the camera the frame is drawn with, not from the cull camera: freezing
        // what is culled must not reorder what is drawn.
        self.transparent.sort_back_to_front(camera.position());
        self.last_group_ms = grouping.elapsed().as_secs_f64() * 1e3;

        self.collect_skinned(world, &frustum);
    }

    /// Copies the world's skinned draws into the renderer, keeping the visible ones.
    fn collect_skinned(&mut self, world: &RenderWorld, frustum: &Frustum) {
        self.skinned.clear();
        let source = world.skinned();
        if source.is_empty() {
            return;
        }
        for instance in source.instances() {
            if self.culling_enabled && !frustum.intersects_sphere(&instance.bounds) {
                continue;
            }
            self.skinned.push(*instance);
        }
        // The palette is only meaningful with the ranges the instances name, so it is taken
        // as one block rather than compacted alongside the filter.
        self.skinned.set_palette(source.palette());
    }

    /// Milliseconds the most recent grouping pass took.
    #[must_use]
    pub fn last_group_ms(&self) -> f64 {
        self.last_group_ms
    }

    /// Decides what this frame's shadow maps contain.
    ///
    /// The shadow stage's half of the same split [`Renderer::build_render_list`] makes:
    /// visibility now, recording later. Everything the passes will need from the world is
    /// copied here: this frame's local lights and the unculled skinned draws.
    pub fn prepare_shadows(&mut self, world: &RenderWorld, camera: &Camera, lighting: Lighting) {
        self.lights.clear();
        if self.local_lights_enabled {
            self.lights.extend_from_slice(world.lights());
        }

        // The whole list, not the culled one: a monster behind the camera still casts into a
        // cascade the camera can see.
        self.shadow_skinned.clear();
        for instance in world.skinned().instances() {
            self.shadow_skinned.push(*instance);
        }
        self.shadow_skinned.set_palette(world.skinned().palette());

        // The lights are clustered against the same camera the frame is drawn with.
        if self.light_clustering {
            self.clusters.build(&self.lights, camera);
        } else {
            self.clusters.disable();
        }

        self.shadow.prepare(
            world,
            camera,
            lighting,
            self.materials.len(),
            self.meshes.len(),
            &self.shadow_layout,
        );
        // What the D3D12 shadow stage's `record` starts its statistics with; the passes in
        // `shadow.rs` add their own times to it.
        self.shadow.stats.cpu_ms = self.shadow.prepare_ms;
        self.shadow.stats.prepare_ms = self.shadow.prepare_ms;
    }

    /// The shadow settings in force.
    #[must_use]
    pub fn shadow_settings(&self) -> &ShadowSettings {
        self.shadow.settings()
    }

    /// Replaces the shadow settings.
    ///
    /// Waits for the GPU first, as the D3D12 renderer does, because a quality change can
    /// replace the atlas and the contact target. That makes this a stall, which belongs on a
    /// settings screen rather than on a slider that fires every frame. The depth bias needs no
    /// rebuild here: Metal takes it per pass, not per pipeline.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if the wait or a new texture fails.
    pub fn set_shadow_settings(&mut self, settings: ShadowSettings) -> Result<(), RendererError> {
        self.wait_for_idle()?;
        let settings = settings.validated();
        self.shadow_stage
            .resize(self.context.device(), &settings, self.layer.size())?;
        self.shadow_layout = ShadowAtlasLayout::new(&settings);
        self.shadow.apply_settings(settings);
        Ok(())
    }

    /// Switches the clustered light culling off, or back on.
    pub fn set_light_clustering(&mut self, enabled: bool) {
        self.light_clustering = enabled;
    }

    /// Whether the clustered light grid is being built.
    #[must_use]
    pub fn light_clustering(&self) -> bool {
        self.light_clustering
    }

    /// Switches the local lights off, or back on, for the whole frame.
    pub fn set_local_lights_enabled(&mut self, enabled: bool) {
        self.local_lights_enabled = enabled;
    }

    /// Whether local lights are being shaded.
    #[must_use]
    pub fn local_lights_enabled(&self) -> bool {
        self.local_lights_enabled
    }

    /// What the last frame's light clustering did.
    #[must_use]
    pub fn light_cluster_stats(&self) -> LightClusterStats {
        self.clusters.stats()
    }

    /// What the last frame's shadow stage did.
    #[must_use]
    pub fn shadow_stats(&self) -> ShadowStats {
        self.shadow.stats()
    }

    /// Selects the shadow debug view.
    ///
    /// `0` none, `1` tints each pixel by the cascade it sampled, `2` shows the shadow factor
    /// alone, `3` draws the atlas in the corner, and higher values show the material maps.
    pub fn set_shadow_debug_mode(&mut self, mode: u32) {
        self.shadow.set_debug_mode(mode);
    }

    /// The shadow debug view currently selected.
    #[must_use]
    pub fn shadow_debug_mode(&self) -> u32 {
        self.shadow.debug_mode()
    }

    /// This frame's cascades, for a debug view that wants to draw their bounds.
    #[must_use]
    pub fn shadow_cascades(&self) -> &[crate::shadow::CascadeView] {
        self.shadow.cascades()
    }

    /// Draws each batch as one instanced call rather than one call per object.
    pub fn set_instancing(&mut self, enabled: bool) {
        self.instancing = enabled;
    }

    /// Whether instanced batching is on.
    #[must_use]
    pub fn instancing(&self) -> bool {
        self.instancing
    }

    /// What the most recent [`Renderer::build_render_list`] selected.
    #[must_use]
    pub fn render_list(&self) -> &RenderList {
        &self.visible
    }

    /// Objects drawn by the most recent frame's transparent phase.
    #[must_use]
    pub fn transparent_draws_last_frame(&self) -> u32 {
        self.transparent_draws_last_frame
    }

    /// This frame's transparent draws, in the order they will be recorded.
    #[must_use]
    pub fn transparent_list(&self) -> &RenderList {
        &self.transparent
    }

    /// Draws the translucent-character passes recorded in the most recent frame: two per
    /// submesh of every character the camera was looking through.
    #[must_use]
    pub fn translucent_character_draws_last_frame(&self) -> u32 {
        self.translucent_character_draws_last_frame
    }

    /// Skinned submeshes drawn by the most recent frame.
    #[must_use]
    pub fn skinned_draws_last_frame(&self) -> u32 {
        self.skinned_draws_last_frame
    }

    /// Usage of the shader-visible descriptor heap's persistent region.
    ///
    /// Metal binds textures directly and has no descriptor heap, so this is always empty.
    #[must_use]
    pub fn descriptor_stats(&self) -> crate::descriptor::DescriptorStats {
        crate::descriptor::DescriptorStats::default()
    }

    /// What the most recent `render_world` cull pass did.
    #[must_use]
    pub fn cull_stats(&self) -> CullStats {
        self.culler.stats()
    }

    /// Turns frustum culling on or off.
    pub fn set_culling_enabled(&mut self, enabled: bool) {
        self.culling_enabled = enabled;
    }

    /// Whether frustum culling is on.
    #[must_use]
    pub fn culling_enabled(&self) -> bool {
        self.culling_enabled
    }

    /// Milliseconds the most recent `render_world` cull pass took.
    #[must_use]
    pub fn last_cull_ms(&self) -> f64 {
        self.last_cull_ms
    }

    /// Culls against `view_projection` instead of the camera's own, or clears the override.
    ///
    /// Freezing the frustum is the only way to *see* culling: the objects it removes are
    /// exactly the ones outside the view.
    pub fn set_cull_view_projection(&mut self, view_projection: Option<engine_core::math::Mat4>) {
        self.cull_view_projection = view_projection;
    }

    /// Whether the cull frustum is frozen away from the camera.
    #[must_use]
    pub fn cull_frustum_is_frozen(&self) -> bool {
        self.cull_view_projection.is_some()
    }

    /// Draws the world as wireframe instead of solid.
    pub fn set_wireframe(&mut self, enabled: bool) {
        self.wireframe = enabled;
    }

    /// Whether wireframe fill is selected.
    #[must_use]
    pub fn wireframe(&self) -> bool {
        self.wireframe
    }

    /// The local-space bounds of a created mesh, which a caller needs to spawn an entity.
    #[must_use]
    pub fn mesh_bounds(&self, mesh: MeshHandle) -> Option<engine_core::math::Sphere> {
        self.meshes.get(mesh.index()).map(GpuMesh::bounds)
    }

    /// Opens a frame and its first pass: [`Renderer::begin_frame`], then a camera pass with
    /// cleared targets.
    ///
    /// For the frames that have no shadow stage in front of the camera pass (the stress scene).
    pub(super) fn begin_recording(&mut self, color: [f32; 4]) -> Result<usize, RendererError> {
        let slot = self.begin_frame()?;
        self.begin_camera_pass(color, false)?;
        Ok(slot)
    }

    /// Opens a frame: waits for the slot, acquires a drawable, resets per-frame state and
    /// starts the command buffer. No pass is open yet.
    ///
    /// Shared by [`Renderer::render`] and [`Renderer::render_world`] so the two cannot drift
    /// apart — the pacing, the GPU-time read, the retirement of replaced meshes and the ring
    /// reset all have to happen in exactly this order.
    pub(super) fn begin_frame(&mut self) -> Result<usize, RendererError> {
        // A frame abandoned half-encoded still has an open encoder, which Metal insists is
        // ended before its command buffer goes away.
        self.abandon_frames();
        self.frame_stats = CommandStats::default();

        // Frame pacing: wait for this slot's previous command buffer.
        let slot = self.frames.begin_frame()?;
        // Last frame's palette addresses point into upload memory another slot owns.
        self.frame_palettes.clear();

        // Published by the completion handlers; lags the CPU by the frames in flight.
        if let Some(milliseconds) = self.frames.take_gpu_ms() {
            self.last_gpu_frame_ms = Some(milliseconds);
        }

        // Meshes whose last use the GPU has now passed.
        let completed = self.frames.completed();
        self.retired_meshes
            .retain(|(last_use, _)| *last_use > completed);

        // This slot's upload memory is free again, for the same reason its serial is.
        self.uploads.begin_frame(slot)?;

        // The drawable can block when every one of them is on screen or in flight; that wait
        // is the equivalent of D3D12's `Present` blocking, and is reported with present time.
        let acquire = Instant::now();
        let drawable = self.layer.next_drawable()?;
        self.last_acquire_ms = acquire.elapsed().as_secs_f64() * 1e3;

        let Some(command_buffer) = self.context.queue().commandBuffer() else {
            return Err(api_error(
                "MTLCommandQueue::commandBuffer",
                "the queue returned no command buffer",
            ));
        };
        self.recording = Some(FrameInProgress {
            command_buffer,
            drawable,
            slot,
        });
        Ok(slot)
    }

    /// Opens the camera pass on the frame's drawable and depth buffer.
    ///
    /// The colour is always cleared to `color`. The depth buffer is cleared too, unless
    /// `keep_depth`: the depth prepass wrote it earlier in this frame and the pass loads it.
    pub(super) fn begin_camera_pass(
        &mut self,
        color: [f32; 4],
        keep_depth: bool,
    ) -> Result<(), RendererError> {
        let Some(frame) = &self.recording else {
            return Err(RendererError::InvalidConfig {
                reason: "the camera pass was begun without a frame being recorded".to_owned(),
            });
        };
        let pass = MTLRenderPassDescriptor::renderPassDescriptor();
        // SAFETY: attachment 0 always exists; a render pass has up to eight colour attachments.
        let color_attachment = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
        let target = frame.drawable.texture();
        color_attachment.setTexture(Some(&target));
        color_attachment.setLoadAction(MTLLoadAction::Clear);
        color_attachment.setStoreAction(MTLStoreAction::Store);
        color_attachment.setClearColor(MTLClearColor {
            red: f64::from(color[0]),
            green: f64::from(color[1]),
            blue: f64::from(color[2]),
            alpha: f64::from(color[3]),
        });
        let depth_attachment = pass.depthAttachment();
        depth_attachment.setTexture(Some(&self.depth));
        if keep_depth {
            depth_attachment.setLoadAction(MTLLoadAction::Load);
        } else {
            depth_attachment.setLoadAction(MTLLoadAction::Clear);
            depth_attachment.setClearDepth(f64::from(DEPTH_CLEAR));
        }
        // Nothing reads the depth after the camera pass.
        depth_attachment.setStoreAction(MTLStoreAction::DontCare);

        let (width, height) = self.layer.size();
        self.begin_pass(&pass, width, height)
    }

    /// Ends the camera pass, adds the screenshot copy if one was asked for, and holds the
    /// encoded command buffer for [`Renderer::present`] to commit.
    pub(super) fn finish_recording(&mut self, slot: usize) -> Result<(), RendererError> {
        // The particles, the overlay and the UI are drawn by the caller of this function.
        self.end_pass();
        let stats = self.frame_stats;
        self.draw_calls_last_frame = stats.draw_calls;
        self.triangles_last_frame = stats.triangles;
        self.command_stats = stats;

        let Some(frame) = self.recording.take() else {
            return Err(RendererError::InvalidConfig {
                reason: "finish_recording called without a frame being recorded".to_owned(),
            });
        };
        if let Some(readback) = self.capture.as_ref().or(self.screenshot.as_ref()) {
            let Some(blit) = frame.command_buffer.blitCommandEncoder() else {
                return Err(api_error(
                    "MTLCommandBuffer::blitCommandEncoder",
                    "the command buffer returned no blit encoder",
                ));
            };
            readback.record_copy(&blit, &frame.drawable.texture());
        }
        debug_assert_eq!(frame.slot, slot, "the frame slot changed while recording");
        self.recorded = Some(frame);
        Ok(())
    }

    /// Drops whatever frame is half-encoded or encoded and never presented.
    ///
    /// A frame that was never committed did no GPU work, so dropping it is the whole recovery.
    fn abandon_frames(&mut self) {
        if self.recorder.is_recording() {
            self.recorder.finish();
        }
        self.recording = None;
        self.recorded = None;
    }

    /// Commits `command_buffer`, which pacing slot `slot` will wait on from now on.
    ///
    /// Installs the completion handler that publishes the frame's serial and its GPU time, and
    /// reports a command buffer that finished in error.
    pub(super) fn commit(&mut self, command_buffer: CommandBuffer, slot: usize) {
        let started = Instant::now();
        let serial = self.frames.next_serial();
        let shared = self.frames.shared();
        let handler: CompletionHandler = RcBlock::new(
            move |finished: NonNull<ProtocolObject<dyn MTLCommandBuffer>>| {
                // SAFETY: Metal passes the command buffer that just completed, which is alive
                // for the duration of the handler.
                let finished = unsafe { finished.as_ref() };
                let gpu_ms = (finished.GPUEndTime() - finished.GPUStartTime()) * 1e3;
                if finished.status() == MTLCommandBufferStatus::Error {
                    let description = finished.error().map_or_else(
                        || "unknown error".to_owned(),
                        |error| super::describe(&error),
                    );
                    log::error!("a frame's command buffer failed: {description}");
                }
                shared.complete(
                    serial,
                    (gpu_ms.is_finite() && gpu_ms >= 0.0).then_some(gpu_ms),
                );
            },
        );
        // SAFETY: the handler block is a valid `dyn Fn(NonNull<..>)` block; Metal copies it
        // before this call returns, so dropping `handler` afterwards is fine.
        unsafe { command_buffer.addCompletedHandler(RcBlock::as_ptr(&handler)) };
        command_buffer.commit();
        self.frames.end_frame(slot, serial);
        self.last_fence_signal_ms = started.elapsed().as_secs_f64() * 1e3;
    }

    /// Presents the frame recorded by [`Renderer::render`] and commits it.
    ///
    /// The drawable is presented by the command buffer, so this is where the frame is
    /// submitted to the GPU. The slot's serial is recorded here, and the next use of that slot
    /// waits on it before reusing the slot's upload memory.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] if there is no recorded frame.
    pub fn present(&mut self) -> Result<(), RendererError> {
        let Some(frame) = self.recorded.take() else {
            return Err(RendererError::InvalidConfig {
                reason: "present called without a recorded frame".to_owned(),
            });
        };
        let present_start = Instant::now();
        objc2::rc::autoreleasepool(|_| {
            frame
                .command_buffer
                .presentDrawable(ProtocolObject::from_ref(&*frame.drawable));
            self.commit(frame.command_buffer, frame.slot);
        });
        // The time the frame spent blocked on its drawable belongs to this stage: it is the
        // wait D3D12's `Present` does when every back buffer is spoken for.
        self.last_present_ms = self.last_acquire_ms + present_start.elapsed().as_secs_f64() * 1e3;
        self.present_window.record_ms(self.last_present_ms);

        self.frames_presented += 1;
        Ok(())
    }

    /// Milliseconds the most recent `present` took, including the wait for a drawable.
    #[must_use]
    pub fn last_present_ms(&self) -> f64 {
        self.last_present_ms
    }

    /// Distribution of `present` durations over the recent window.
    ///
    /// `None` before the first frame. Sorting costs O(n log n) in the window size, so read
    /// this on a reporting interval rather than per frame.
    pub fn present_stats(&mut self) -> Option<FrameStats> {
        self.present_window.stats()
    }

    /// Forgets the recorded `present` durations.
    pub fn reset_present_stats(&mut self) {
        self.present_window.clear();
    }

    /// Milliseconds the most recent commit bookkeeping took: installing the completion handler
    /// and committing. The Metal counterpart of D3D12's fence signal.
    #[must_use]
    pub fn last_fence_signal_ms(&self) -> f64 {
        self.last_fence_signal_ms
    }

    /// How often, and for how long, the CPU has waited for the GPU.
    #[must_use]
    pub fn sync_stats(&self) -> SyncStats {
        self.frames.stats()
    }

    /// Number of frames the CPU may run ahead of the GPU.
    #[must_use]
    pub fn frames_in_flight(&self) -> usize {
        self.frames.frames_in_flight()
    }

    /// Resizes the drawable, waiting for the GPU to finish with the old size first.
    ///
    /// A zero dimension (a minimized window) is ignored: there is nothing to present to.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if the idle wait or the new depth buffer fails.
    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), RendererError> {
        if width == 0 || height == 0 {
            log::debug!("ignoring resize to {width}x{height}");
            return Ok(());
        }
        if (width, height) == self.layer.size() {
            return Ok(());
        }

        self.wait_for_idle()?;
        // A readback sized for the old drawable would copy past the new one's edge.
        self.screenshot = None;
        // A frame recorded against the old size must not be presented after the layer changes;
        // it was never committed, so dropping it is the correct recovery.
        if self.recording.is_some() || self.recorded.is_some() {
            log::debug!("dropped a recorded frame because the layer was resized");
        }
        self.abandon_frames();

        self.layer.resize(width, height);
        // It must match the drawable exactly: a depth buffer smaller than the render target is
        // a validation error, and a larger one wastes memory.
        self.depth = depth_texture(self.context.device(), width, height)?;
        // The contact target follows the camera's size.
        self.shadow_stage.resize(
            self.context.device(),
            self.shadow.settings(),
            (width, height),
        )?;
        Ok(())
    }

    /// What the most recent frame's command recording cost.
    #[must_use]
    pub fn command_stats(&self) -> CommandStats {
        self.command_stats
    }

    /// Largest per-frame upload usage observed.
    ///
    /// The number that sizes `RendererConfig::upload_bytes_per_frame`.
    #[must_use]
    pub fn upload_peak_bytes(&self) -> usize {
        self.uploads.peak()
    }

    /// Resources waiting for the GPU before they can be released.
    #[must_use]
    pub fn pending_releases(&self) -> usize {
        self.retired_meshes.len()
    }

    /// Blocks until the GPU has finished all committed work.
    ///
    /// Fully serializing — for shutdown, resize and settings changes only, never per frame.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if the GPU does not finish.
    pub fn wait_for_idle(&mut self) -> Result<(), RendererError> {
        self.frames.wait_for_idle()
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        // An encoder still open when its command buffer is released is a Metal assertion.
        self.abandon_frames();
        // The GPU may still be reading buffers and textures this struct is about to release.
        if let Err(error) = self.wait_for_idle() {
            log::error!("GPU did not go idle during shutdown: {error}");
        }
        let sync = self.frames.stats();
        log::info!(
            "renderer shut down after {} frames ({} frames in flight)",
            self.frames_presented,
            self.frames.frames_in_flight()
        );
        log::info!(
            "CPU waited for the GPU on {}/{} frames ({:.1}%), mean {:.3} ms per frame, max {:.3} ms",
            sync.stalled_frames,
            sync.total_frames,
            sync.stall_ratio() * 100.0,
            sync.mean_stall_ms(),
            sync.max_stall.as_secs_f64() * 1e3
        );
    }
}
