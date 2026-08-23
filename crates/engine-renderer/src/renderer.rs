//! Renderer front end: owns the device, swap chain, per-frame command recording and
//! the fence that keeps the CPU from overtaking the GPU.
//!
//! This type knows nothing about gameplay. Milestone 2 can only clear the back buffer;
//! geometry arrives in Milestone 4 through a `RenderWorld`-shaped input.

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D::{
    D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP,
};
use windows::Win32::Graphics::Direct3D12::{
    D3D12_RESOURCE_STATE_DEPTH_WRITE, D3D12_RESOURCE_STATE_PRESENT,
    D3D12_RESOURCE_STATE_RENDER_TARGET, ID3D12CommandList,
};

use windows::core::Interface;

use crate::command::{CommandContext, CommandStats};
use crate::culling::{CullStats, Culler};
use crate::depth::{DEPTH_CLEAR, DepthBuffer};
use crate::descriptor::GpuDescriptorHeap;
use crate::device::{BACKEND, RenderDevice, feature_level_name};
use crate::error::RendererError;
use crate::frame::{FrameRing, SyncStats};
use crate::gpu_timer::{GpuTimer, SHADOW_PASS as SHADOW_PASS_QUERY};
use crate::material::{Material, MaterialDesc};
use crate::mesh::{GpuMesh, MeshData, MeshVertex, SkinnedMeshData, SkinnedVertex, VertexStream};
use crate::mesh_pipeline::{
    InstanceTransform, InstancedMeshPipeline, MeshPipeline, SkinnedMeshPipeline,
};
#[cfg(feature = "debug-overlay")]
use crate::overlay::{OverlayBuffers, OverlayGeometry, OverlayMetrics};
use crate::particle_pipeline::{ParticleConstants, ParticlePipeline, ROOT_PARTICLE_FRAME_CBV};
use crate::pipeline::{ScenePipeline, Vertex};
use crate::render_list::RenderList;
use crate::resource::DeferredReleaseQueue;
use crate::scene::{
    FrameConstants, ObjectConstants, ROOT_CLUSTER_SRV, ROOT_DEPTH_TABLE, ROOT_FRAME_CBV,
    ROOT_MATERIAL_CBV,
    ROOT_MATERIAL_TABLE, ROOT_OBJECT_CBV, ROOT_SHADOW_TABLE, ROOT_SKIN_CBV,
};
use crate::shadow::renderer::ShadowScene;
use crate::shadow::{ShadowRenderer, ShadowSettings, ShadowStats};
use crate::stress;
use crate::swapchain::SwapChain;
use crate::ui_atlas::UiAtlas;
use crate::ui_pipeline::{
    ROOT_UI_ATLAS_TABLE, ROOT_UI_FRAME_CBV, UiBatchStats, UiBatcher, UiFrameConstants, UiPipeline,
    UiVertex,
};
use crate::light_cluster::{LightClusterStats, LightClusters};
use crate::upload::{CONSTANT_BUFFER_ALIGNMENT, StagingUploader, UploadRing};
use crate::{AdapterInfo, RendererConfig};
use engine_core::math::Frustum;
use engine_core::time::{FrameStats, FrameStatsWindow};
use engine_render_data::Camera;
use engine_render_data::{
    Lighting, LocalLight, MAX_PALETTE_BONES, MaterialHandle, MeshHandle, ModelBinding,
    ModelLodBinding, ModelPart, PartGeometry, RenderWorld, SkinnedDrawList, SkinnedMeshHandle,
};

/// How many shader-visible SRVs the renderer's one descriptor heap holds.
///
/// One for the UI atlas, one per material texture. A thousand is roughly a hundred
/// characters' worth of maps at ten each; the heap is a few tens of kilobytes and running
/// out of it is a named error rather than corruption, so the number is generous on purpose.
const MAX_TEXTURE_DESCRIPTORS: u32 = 1024;

/// The height of a model's LOD0 geometry, in metres.
///
/// Measured rather than declared, so a [`ModelBinding`] can always be scaled to a gameplay
/// height. A model with no geometry reports one metre rather than zero — the caller divides
/// by this.
fn measured_height(model: &engine_assets::ModelAsset) -> f32 {
    let Some(lod) = model.lod0() else {
        return 1.0;
    };
    let mut lowest = f32::MAX;
    let mut highest = f32::MIN;
    for position in lod
        .submeshes
        .iter()
        .flat_map(|submesh| submesh.geometry.positions())
    {
        lowest = lowest.min(position.y);
        highest = highest.max(position.y);
    }
    let height = highest - lowest;
    if height.is_finite() && height > 1e-3 {
        height
    } else {
        1.0
    }
}

/// Frames of `Present` history kept for its distribution.
///
/// The same window the engine's frame metrics use, so the two describe the same span of
/// time and a present percentile can be read against a frame percentile.
const PRESENT_WINDOW: usize = engine_core::metrics::DEFAULT_HISTORY;

/// Alignment for a per-instance vertex stream in the upload ring.
///
/// A vertex buffer view has no 256-byte requirement the way a constant buffer view does;
/// what it needs is an address aligned to its largest element, so the input assembler's
/// fetches do not straddle a boundary. Every element in the stream is a `float4` or
/// smaller, so sixteen bytes is enough.
///
/// It is deliberately **not** `size_of::<InstanceTransform>()`, which is what it used to
/// be: the record grew to 96 bytes when it gained a tint, and 96 is not a power of two —
/// which the ring's aligner requires and which turned an alignment constant into a panic.
const INSTANCE_ALIGNMENT: usize = 16;

/// Alignment for the UI vertex stream in the upload ring.
///
/// The stride, for the same reason as [`INSTANCE_ALIGNMENT`]: a vertex buffer view has no
/// 256-byte requirement, it needs an address the input assembler can index by stride.
const UI_VERTEX_ALIGNMENT: usize = size_of::<UiVertex>();

/// Alignment for the particle instance stream in the upload ring.
///
/// Sixteen, deliberately **not** the 36-byte record size: the ring's aligner requires a
/// power of two, and a vertex buffer needs an address the input assembler can index from
/// rather than one that is a multiple of the stride. This is the same trap
/// [`INSTANCE_ALIGNMENT`] documents, met a second time by a record that is also not a power
/// of two.
const PARTICLE_ALIGNMENT: usize = 16;

/// Most quads one frame of UI may emit.
///
/// 65,536 quads is 12 MB of vertices, well past anything the HUD produces — twenty
/// nameplates plus the full combat HUD is a few thousand. It exists so a runaway UI drops
/// work visibly rather than allocating without bound inside the frame path.
const MAX_UI_QUADS: usize = 65_536;

/// A texture uploaded through [`Renderer::create_texture`].
///
/// A plain index. Textures are created while a scene is assembled and never released, so
/// there is no freed slot for a stale handle to address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextureHandle(u32);

impl TextureHandle {
    /// The slot this handle names.
    #[must_use]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// An image to upload: dimensions, colour space, and straight RGBA8 texels.
///
/// Borrowed rather than owned, because the caller already has the texels in an
/// `engine_assets::TextureAsset` and copying them to hand them over would double the peak
/// memory of a scene load for no reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureImage<'a> {
    /// Width in texels.
    pub width: u32,
    /// Height in texels.
    pub height: u32,
    /// Whether the hardware should linearize on read.
    ///
    /// True for base colour, false for normal maps and packed data. Getting this backwards
    /// is the classic washed-out or muddy import, and it has no other symptom.
    pub srgb: bool,
    /// Row-major RGBA8 texels.
    pub texels: &'a [u8],
}

/// A texture and the descriptor that addresses it.
///
/// One struct rather than two parallel arrays, because the two have exactly the same
/// lifetime: the descriptor points into the texture, so releasing either alone is the bug.
struct UploadedTexture {
    /// Held, not read: the descriptor below points into this texture, so dropping it would
    /// leave a shader-visible SRV addressing released memory. Ownership *is* its purpose.
    #[allow(dead_code, reason = "keeps the texture the descriptor points at alive")]
    texture: crate::resource::GpuTexture,
    descriptor: crate::descriptor::DescriptorHandle,
}

/// The batched game-UI pass.
///
/// One pipeline, one texture, one draw call. Everything the player sees that is not
/// geometry goes through here.
struct UiState {
    pipeline: UiPipeline,
    /// Turns a `UiDrawList` into vertices. Owns its scratch across frames.
    batcher: UiBatcher,
    /// The font atlas on the GPU. Uploaded once at creation and never touched again.
    ///
    /// Held, not read: the shader resource view below points into this texture, so dropping
    /// it would leave the descriptor addressing a released resource. Ownership *is* its
    /// purpose.
    #[allow(
        dead_code,
        reason = "keeps the texture the atlas descriptor points at alive"
    )]
    atlas: crate::resource::GpuTexture,
    /// Where in the renderer's shared descriptor heap the atlas's SRV lives.
    ///
    /// It used to have a heap of its own, on the grounds that it was the renderer's only
    /// shader-visible descriptor and the subsystem it would share with did not exist. Phase 5
    /// is that subsystem: materials now carry base colour maps, and only one CBV/SRV/UAV heap
    /// can be bound at a time. Two heaps would mean a `SetDescriptorHeaps` between the mesh
    /// pass and the UI pass on every frame, which drivers document as expensive; one heap
    /// means it is bound once, in `begin_recording`, and never changed.
    atlas_descriptor: crate::descriptor::DescriptorHandle,
    /// This frame's list, replaced by the application each frame.
    list: engine_render_data::UiDrawList,
    /// What the last batch produced.
    stats: UiBatchStats,
    /// Milliseconds the last batch build took, measured on the CPU.
    build_ms: f64,
    /// Whether the UI pass draws at all.
    enabled: bool,
}

/// The blended particle pass, owned by the renderer.
///
/// The list is held across frames and cleared rather than freed, exactly as the UI's is:
/// a frame's particles are transient, the memory that holds them is not.
struct ParticleState {
    pipeline: ParticlePipeline,
    list: engine_render_data::ParticleDrawList,
    /// This frame's camera basis, captured where the camera is in hand.
    ///
    /// The pass is recorded from `finish_recording`, which is handed a frame slot and a back
    /// buffer and deliberately not a camera — so the constants are built in
    /// `draw_render_list`, beside the scene's own, and read here.
    constants: ParticleConstants,
    /// Particles submitted by the most recent frame.
    drawn: u32,
    /// Whether the pass draws at all. A developer switch, and what the benchmark's
    /// particle-free baseline turns off.
    enabled: bool,
}

/// Debug overlay state owned by the renderer.
#[cfg(feature = "debug-overlay")]
struct OverlayState {
    geometry: OverlayGeometry,
    buffers: OverlayBuffers,
    /// Whether the overlay is drawn. Runtime switch on top of the compile-time feature.
    enabled: bool,
    /// Cached vertex buffer view per frame slot.
    ///
    /// The overlay text changes a few times a second but the frame runs thousands of
    /// times a second, so uploading unconditionally would memcpy hundreds of kilobytes
    /// per frame for identical data. Each slot keeps its view until the text changes.
    views: Vec<Option<windows::Win32::Graphics::Direct3D12::D3D12_VERTEX_BUFFER_VIEW>>,
    /// Slots whose upload buffer no longer matches the current geometry.
    stale: Vec<bool>,
    /// Geometry rebuilt from scratch every frame: nameplates and anything else anchored to
    /// something that moves.
    ///
    /// A second buffer rather than more of the first, because the two have opposite
    /// lifetimes. The developer HUD changes ten times a second and is cached; a nameplate
    /// follows a character and is stale the moment the camera turns, so caching it would
    /// mean marking every slot dirty every frame — which is the same as not caching, but
    /// also re-uploads the HUD's tens of thousands of quads alongside it.
    dynamic: OverlayGeometry,
    dynamic_buffers: OverlayBuffers,
    /// Vertices drawn by the most recent frame, both layers together.
    drawn_vertices: u32,
}

/// D3D12 renderer.
///
/// **Field order is load-bearing.** Rust drops fields in declaration order, and D3D12
/// objects must be released before the device and queue that created them — releasing
/// the device while a swap chain or command list still lives crashes inside the driver
/// on some adapters. `context` therefore comes last.
pub struct Renderer {
    swap_chain: SwapChain,
    /// Frame slots, their allocators and the fence that paces them.
    frames: FrameRing,
    /// The command list and everything recorded through it.
    commands: CommandContext,
    /// What the most recent frame's recording cost.
    command_stats: CommandStats,
    /// Pre-built argument for `ExecuteCommandLists`, so submitting a frame performs no
    /// allocation and no `QueryInterface`.
    execute_lists: [Option<ID3D12CommandList>; 1],
    /// GPU-side frame timing. Read with a `FRAME_COUNT`-frame lag so the CPU never
    /// blocks to obtain it.
    gpu_timer: GpuTimer,
    /// Most recent GPU frame time in milliseconds, or `None` before the first slot has
    /// completed a full lap.
    last_gpu_frame_ms: Option<f64>,
    /// Root signature and pipeline state, independent of the geometry.
    pipeline: ScenePipeline,
    /// Depth target, the size of the back buffer.
    depth: DepthBuffer,
    /// Root signature and pipeline state for `MeshVertex` geometry.
    mesh_pipeline: MeshPipeline,
    /// The same pipeline with wireframe fill, selected by [`Renderer::set_wireframe`].
    mesh_pipeline_wireframe: MeshPipeline,
    /// Whether `render_world` draws through the wireframe pipeline.
    wireframe: bool,
    /// Root signature and pipeline state for skinned geometry.
    skinned_pipeline: SkinnedMeshPipeline,
    /// The same, with wireframe fill.
    skinned_pipeline_wireframe: SkinnedMeshPipeline,
    /// Meshes created through [`Renderer::create_mesh`], indexed by handle.
    meshes: Vec<GpuMesh>,
    /// Skinned meshes created through [`Renderer::create_skinned_mesh`], indexed by handle.
    ///
    /// A second table rather than a flag on `meshes`: the two are read by different pipeline
    /// states with different input layouts, and a handle that could name either would put
    /// the branch in the recording loop.
    skinned_meshes: Vec<GpuMesh>,
    /// Textures created through [`Renderer::create_texture`], indexed by handle.
    ///
    /// Held for ownership: each one's SRV lives in [`Renderer::descriptors`] and would
    /// address a released resource if the texture were dropped.
    textures: Vec<UploadedTexture>,
    /// The shader-visible CBV/SRV/UAV heap every SRV lives in — material maps and the UI
    /// atlas alike. Bound once per frame in `begin_recording`.
    descriptors: GpuDescriptorHeap,
    /// The 1×1 opaque white texture an untextured material samples.
    ///
    /// One texture fetch that returns 1, instead of a second pipeline state, a second pair
    /// of shaders and the branch that would choose between them.
    white_texture: TextureHandle,
    /// Materials created through [`Renderer::create_material`], indexed by handle.
    materials: Vec<Material>,
    /// The shadow stage: the atlas, what goes in it, and the passes that fill it.
    ///
    /// Declared before `context` like every other GPU-owning field, because it holds a
    /// texture and pipeline states that must be released before the device is.
    shadow: ShadowRenderer,
    /// Each material's base colour map as a shader-visible handle, parallel to
    /// [`Renderer::materials`].
    ///
    /// Resolved once at material creation rather than per batch. It exists because the
    /// shadow pass needs the same lookup and cannot call a `&self` method while it holds a
    /// `&mut` borrow of the renderer's shadow state — and having it as data rather than a
    /// method also removes two indirections from the camera pass's inner loop.
    material_tables: Vec<Option<windows::Win32::Graphics::Direct3D12::D3D12_GPU_DESCRIPTOR_HANDLE>>,
    /// This frame's local lights, copied out of the world by [`Renderer::prepare_shadows`].
    ///
    /// Copied rather than borrowed for the same reason the render list is a copy: recording
    /// happens in `draw_render_list`, which deliberately never looks at the `RenderWorld`.
    lights: Vec<LocalLight>,
    /// This frame's clustered light grid: which lights reach which part of the frustum.
    ///
    /// Rebuilt in [`Renderer::prepare_shadows`], where the camera is in scope, and written
    /// into the frame constants beside the lights themselves.
    clusters: LightClusters,
    /// Whether the clustered grid is built at all.
    ///
    /// A measurement control. False leaves the grid disabled, and the shader then walks the
    /// whole light list — which is what it did before [`crate::light_cluster`] existed.
    light_clustering: bool,
    /// Whether this frame's local lights are shaded at all.
    ///
    /// A measurement control, not a quality level: "what do the local lights cost?" is
    /// answered by two runs of one binary, and the alternative - deleting the braziers from
    /// the scene - would measure a different scene. On for every ordinary run.
    local_lights_enabled: bool,
    /// This frame's skinned draws, *unculled*, for the shadow passes.
    ///
    /// Separate from [`Renderer::skinned`], which is the camera's filtered copy: a character
    /// behind the camera casts into a cascade, so the shadow stage needs the whole list.
    shadow_skinned: SkinnedDrawList,
    /// Which texture each material samples, parallel to [`Renderer::materials`].
    ///
    /// Parallel rather than a field on `Material`: a material's constants are a device
    /// buffer it owns, and its texture is a descriptor the renderer owns. Keeping them apart
    /// is what let the texture path be added without changing `Material`'s upload at all.
    material_textures: Vec<TextureHandle>,
    /// Frustum culling: the visibility policy and what the last pass counted.
    culler: Culler,
    /// This frame's render list — what culling selected, in the compact form recording
    /// reads. Kept across frames so it stops allocating once the scene reaches its
    /// high-water mark.
    visible: RenderList,
    /// Whether `render_world` culls. Off makes the benchmark able to measure the same
    /// frame with one step removed rather than comparing two code paths.
    culling_enabled: bool,
    /// A frustum to cull against instead of the camera's own.
    ///
    /// Set to freeze the cull frustum where the camera was, then fly away and watch what
    /// culling actually rejected. Without it, "is culling working?" can only be answered by
    /// a counter, because the objects it removes are by definition the ones off screen.
    cull_view_projection: Option<engine_core::math::Mat4>,
    /// Milliseconds the most recent cull pass took, measured on the CPU.
    last_cull_ms: f64,
    /// Per-chunk render lists for a parallel cull, kept across frames so the chunks stop
    /// allocating once the scene reaches its high-water mark.
    cull_scratch: Vec<RenderList>,
    /// Milliseconds the most recent grouping pass took.
    last_group_ms: f64,
    /// Root signature and pipeline state for instanced mesh geometry.
    instanced_pipeline: InstancedMeshPipeline,
    /// The same, with wireframe fill.
    instanced_pipeline_wireframe: InstancedMeshPipeline,
    /// Whether each batch is drawn as one instanced call.
    ///
    /// On by default. Milestone 17 measured it at 2.2x the frame rate at 100,000 objects
    /// with an image proven pixel-identical to the per-object path, so the per-object path
    /// is now the fallback rather than the norm — it stays for A/B measurement.
    instancing: bool,
    /// Per-batch instance data, staged here before the memcpy into the upload ring.
    instance_scratch: Vec<InstanceTransform>,
    /// This frame's visible skinned draws, filtered out of the world by `collect_skinned`.
    ///
    /// Kept across frames so it stops allocating once the scene reaches its high-water mark,
    /// exactly like the render list.
    skinned: engine_render_data::SkinnedDrawList,
    /// One character's bone palette, padded to the shader's array, staged here before the
    /// memcpy into the upload ring.
    palette_scratch: Vec<[f32; 16]>,
    /// Skinned draws recorded by the most recent frame.
    skinned_draws_last_frame: u32,
    /// Milliseconds the most recent `Present` call took.
    last_present_ms: f64,
    /// Milliseconds the most recent post-present fence signal took.
    last_fence_signal_ms: f64,
    /// A window of `Present` durations.
    ///
    /// A single frame's present time is not a number worth reading: it varies by more than
    /// a factor of two frame to frame, because whether `Present` blocks depends on where
    /// the GPU happens to be. The distribution is the answer; one sample is an anecdote.
    present_window: FrameStatsWindow,
    /// The geometry currently being drawn.
    scene: VertexStream,
    /// Scratch buffer for regenerating scene geometry without allocating each time.
    scene_scratch: Vec<Vertex>,
    /// Debug overlay geometry and its per-slot upload buffers. Absent entirely from a
    /// build without the `debug-overlay` feature.
    #[cfg(feature = "debug-overlay")]
    overlay: OverlayState,
    /// The batched game-UI pass. **Not** behind `debug-overlay`: this is what the player's
    /// health bar is drawn with, and a shipping build needs it.
    ui: UiState,
    /// The blended particle pass, drawn between the opaque scene and the interface.
    particles: ParticleState,
    frames_presented: u64,
    /// Draw calls recorded by the most recent frame.
    draw_calls_last_frame: u32,
    /// Triangles submitted by the most recent frame.
    triangles_last_frame: u64,
    /// Per-frame upload memory, reset at the start of each frame.
    uploads: UploadRing,
    /// Resources waiting for the GPU to finish with them before being released.
    pending_release: DeferredReleaseQueue,
    /// Slot recorded by `render` and not yet presented.
    recorded_frame_slot: Option<usize>,
    /// Back-buffer index the recorded frame targeted. Tracked separately from the frame
    /// slot: one is a texture, the other is CPU-side recording state.
    recorded_back_buffer: usize,
    /// Device, queue and factory. Declared last so it is released last.
    context: RenderDevice,
}

impl Renderer {
    /// Initializes D3D12 and creates a swap chain for `window_handle`.
    ///
    /// `window_handle` is the raw `HWND` value, as returned by the platform layer. The
    /// renderer takes it as an opaque integer so it does not depend on the platform crate.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] for a zero-sized or handle-less request,
    /// [`RendererError::NoSuitableAdapter`] if no D3D12 adapter exists, or
    /// [`RendererError::Api`] if a DXGI/D3D12 call fails.
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

        let context =
            RenderDevice::new(config.enable_debug_layer, config.break_on_validation_error)?;
        let hwnd = HWND(window_handle as *mut std::ffi::c_void);
        let swap_chain = SwapChain::new(&context, hwnd, config.width, config.height, config.vsync)?;

        let frames = FrameRing::new(&context, config.frames_in_flight)?;
        let frames_in_flight = frames.frames_in_flight();

        // Created closed, so the per-frame path can always begin with Reset.
        let commands = CommandContext::new(&context, frames.allocator(0))?;

        let execute_lists = [Some(commands.list().cast::<ID3D12CommandList>().map_err(
            |error| RendererError::api("ID3D12GraphicsCommandList::cast<ID3D12CommandList>", error),
        )?)];

        let uploads = UploadRing::new(&context, config.upload_bytes_per_frame, frames_in_flight)?;
        let gpu_timer = GpuTimer::new(context.device(), context.queue(), frames_in_flight)?;
        let pipeline = ScenePipeline::new(context.device())?;
        let depth = DepthBuffer::new(&context, config.width, config.height)?;
        let mesh_pipeline = MeshPipeline::new(context.device())?;
        let mesh_pipeline_wireframe = MeshPipeline::wireframe(context.device())?;
        let instanced_pipeline = InstancedMeshPipeline::new(context.device())?;
        let instanced_pipeline_wireframe = InstancedMeshPipeline::wireframe(context.device())?;
        let mut scene_scratch = Vec::new();
        stress::build_grid(1, &mut scene_scratch);
        let scene = VertexStream::upload(&context, &scene_scratch)?;

        let skinned_pipeline = SkinnedMeshPipeline::new(context.device())?;
        let skinned_pipeline_wireframe = SkinnedMeshPipeline::wireframe(context.device())?;

        // One heap for every shader-visible SRV in the renderer: the UI atlas and one per
        // material texture. Bound once per frame; never switched.
        let mut descriptors =
            GpuDescriptorHeap::new(&context, MAX_TEXTURE_DESCRIPTORS, 0, frames_in_flight)?;
        // Before the struct literal, because it allocates two persistent descriptors out of
        // the heap above and the heap is moved into the literal.
        let shadow = ShadowRenderer::new(
            &context,
            &mut descriptors,
            ShadowSettings::default(),
            (config.width, config.height),
        )?;
        let ui = Self::create_ui_state(&context, &mut descriptors)?;
        let particle_pipeline = ParticlePipeline::new(context.device())?;

        #[cfg(feature = "debug-overlay")]
        let overlay = OverlayState {
            dynamic: OverlayGeometry::new(),
            dynamic_buffers: OverlayBuffers::new(context.device(), frames_in_flight)?,
            geometry: OverlayGeometry::new(),
            buffers: OverlayBuffers::new(context.device(), frames_in_flight)?,
            enabled: true,
            views: vec![None; frames_in_flight],
            stale: vec![true; frames_in_flight],
            drawn_vertices: 0,
        };

        let renderer = Self {
            swap_chain,
            frames,
            commands,
            command_stats: CommandStats::default(),
            execute_lists,
            gpu_timer,
            last_gpu_frame_ms: None,
            pipeline,
            depth,
            mesh_pipeline,
            mesh_pipeline_wireframe,
            wireframe: false,
            skinned_pipeline,
            skinned_pipeline_wireframe,
            meshes: Vec::new(),
            skinned_meshes: Vec::new(),
            textures: Vec::new(),
            descriptors,
            // Replaced immediately below, once there is a `self` to upload through. The
            // placeholder names slot 0, which is exactly what the white texture becomes.
            white_texture: TextureHandle(0),
            materials: Vec::new(),
            material_textures: Vec::new(),
            material_tables: Vec::new(),
            shadow,
            lights: Vec::new(),
            clusters: LightClusters::new(),
            light_clustering: true,
            local_lights_enabled: true,
            shadow_skinned: SkinnedDrawList::new(),
            culler: Culler::new(),
            visible: RenderList::new(),
            culling_enabled: true,
            cull_view_projection: None,
            last_cull_ms: 0.0,
            cull_scratch: Vec::new(),
            last_group_ms: 0.0,
            instanced_pipeline,
            instanced_pipeline_wireframe,
            instancing: true,
            instance_scratch: Vec::new(),
            skinned: engine_render_data::SkinnedDrawList::new(),
            palette_scratch: Vec::new(),
            skinned_draws_last_frame: 0,
            last_present_ms: 0.0,
            last_fence_signal_ms: 0.0,
            present_window: FrameStatsWindow::new(PRESENT_WINDOW),
            scene,
            scene_scratch,
            #[cfg(feature = "debug-overlay")]
            overlay,
            ui,
            particles: ParticleState {
                pipeline: particle_pipeline,
                list: engine_render_data::ParticleDrawList::default(),
                constants: ParticleConstants::default(),
                drawn: 0,
                enabled: true,
            },
            frames_presented: 0,
            draw_calls_last_frame: 0,
            triangles_last_frame: 0,
            uploads,
            pending_release: DeferredReleaseQueue::new(),
            recorded_frame_slot: None,
            recorded_back_buffer: 0,
            context,
        };
        let mut renderer = renderer;
        // The first texture, so an untextured material has something to sample. Created here
        // rather than in the struct literal because uploading needs a `self`.
        renderer.white_texture = renderer.create_texture(&TextureImage {
            width: 1,
            height: 1,
            srgb: true,
            texels: &[255, 255, 255, 255],
        })?;
        debug_assert_eq!(
            renderer.white_texture.index(),
            0,
            "the white texture is the placeholder the struct literal assumed"
        );
        renderer.log_startup_info();
        Ok(renderer)
    }

    /// Logs the adapter, feature level, backend and resolution.
    fn log_startup_info(&self) {
        let info = self.context.adapter_info();
        let (width, height) = self.swap_chain.size();
        log::info!("GPU: {}", info.name);
        if info.dedicated_video_memory > 0 {
            log::info!(
                "dedicated GPU memory: {} MiB",
                info.dedicated_video_memory_mib()
            );
        } else {
            log::info!(
                "dedicated GPU memory: none reported (shared: {} MiB)",
                info.shared_system_memory / (1024 * 1024)
            );
        }
        log::info!(
            "feature level: {}",
            feature_level_name(self.context.feature_level())
        );
        log::info!("backend: {BACKEND}");
        log::info!("resolution: {width}x{height}");
        log::info!(
            "presentation: vsync={}, tearing={}",
            self.swap_chain.vsync(),
            self.context.allow_tearing()
        );
    }

    /// Description of the adapter in use.
    #[must_use]
    pub fn adapter_info(&self) -> &AdapterInfo {
        self.context.adapter_info()
    }

    /// Current back-buffer size in pixels.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        self.swap_chain.size()
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

    /// Most recent GPU frame time in milliseconds, measured with GPU timestamps.
    ///
    /// `None` until a frame slot has been reused, which takes `FRAME_COUNT` frames. The
    /// value lags the CPU frame time by up to `FRAME_COUNT` frames, because reading it
    /// sooner would require blocking on the GPU and would change what is being measured.
    #[must_use]
    pub fn last_gpu_frame_ms(&self) -> Option<f64> {
        self.last_gpu_frame_ms
    }

    /// Timestamp ticks per second on the direct queue.
    #[must_use]
    pub fn timestamp_frequency(&self) -> u64 {
        self.gpu_timer.frequency()
    }

    /// Triangles in the current scene.
    #[must_use]
    pub fn scene_triangle_count(&self) -> u32 {
        self.scene.triangle_count()
    }

    /// Replaces the scene with a grid of `triangles` triangles.
    ///
    /// Waits for the GPU to go idle first, because the mesh it is currently reading is
    /// about to be released. This is a scene change, not a per-frame operation, so the
    /// stall is acceptable — and it is deliberately *not* on the measured path: callers
    /// should switch levels, then measure.
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
        if triangles == self.scene.triangle_count() {
            return Ok(());
        }

        self.wait_for_idle()?;
        if self.recorded_frame_slot.take().is_some() {
            log::debug!("dropped a recorded frame because the scene changed");
        }
        self.reset_command_list_to_empty()?;

        stress::build_grid(triangles, &mut self.scene_scratch);
        self.scene = VertexStream::upload(&self.context, &self.scene_scratch)?;

        log::info!(
            "scene: {triangles} triangles ({} vertices, {} KiB)",
            self.scene.vertex_count(),
            self.scene.vertex_count() as usize * size_of::<Vertex>() / 1024
        );
        Ok(())
    }

    /// Replaces the overlay text, one entry per line.
    ///
    /// Call before [`Renderer::render`]; the geometry is rebuilt here rather than per
    /// frame, so callers should only pass new text when it changes. Text beyond the
    /// overlay's reserved capacity is dropped rather than allocated for.
    #[cfg(feature = "debug-overlay")]
    pub fn set_overlay_text<S: AsRef<str>>(&mut self, lines: &[S]) {
        let (width, height) = self.swap_chain.size();
        self.overlay.geometry.begin();
        for (index, line) in lines.iter().enumerate() {
            self.overlay
                .geometry
                .push_line(index, line.as_ref(), width, height);
        }
        // Every slot's upload buffer now holds the previous text.
        self.overlay.stale.fill(true);
        if self.overlay.geometry.is_empty() {
            self.overlay.views.fill(None);
        }
    }

    /// Appends an opaque backdrop behind the overlay text.
    ///
    /// Call *after* [`Renderer::set_overlay_text`], which rebuilds the geometry this
    /// appends to. `columns` is the longest line's length; anything wider spills onto the
    /// scene, which is a readability problem rather than a correctness one.
    #[cfg(feature = "debug-overlay")]
    pub fn push_overlay_backdrop(&mut self, lines: usize, columns: usize) {
        let (width, height) = self.swap_chain.size();
        self.overlay
            .geometry
            .push_backdrop(lines, columns, width, height);
        self.overlay.stale.fill(true);
    }

    /// Appends a frame-time graph below the overlay text.
    ///
    /// Call *after* [`Renderer::set_overlay_text`]: that call rebuilds the geometry from
    /// scratch, and this one appends to it. `samples` are milliseconds, oldest first, and
    /// `budget_ms` is the height the panel is scaled to.
    #[cfg(feature = "debug-overlay")]
    pub fn push_overlay_frame_graph(&mut self, samples: &[f32], budget_ms: f32) {
        /// Distance from the left edge, matching the text margin.
        const LEFT_PIXELS: f32 = 12.0;
        /// Distance from the bottom edge.
        const BOTTOM_MARGIN_PIXELS: f32 = 16.0;
        /// Panel size in pixels.
        const PANEL: (f32, f32) = (360.0, 90.0);

        let (width, height) = self.swap_chain.size();
        self.overlay.geometry.push_frame_graph(
            samples,
            budget_ms,
            (LEFT_PIXELS, height as f32 - BOTTOM_MARGIN_PIXELS),
            PANEL,
            width,
            height,
        );
        // Every slot's upload buffer now holds geometry without the graph. The views
        // themselves are replaced by the re-upload this flag forces.
        self.overlay.stale.fill(true);
    }

    /// Discards the previous frame's dynamic overlay geometry.
    ///
    /// Call once a frame before pushing nameplates. The developer HUD is *not* affected:
    /// it lives in the cached layer and is rebuilt only when its text changes.
    #[cfg(feature = "debug-overlay")]
    pub fn begin_dynamic_overlay(&mut self) {
        self.overlay.dynamic.begin();
    }

    /// Appends text to the dynamic layer, positioned in pixels from the top-left.
    ///
    /// `depth` orders overlapping elements: the overlay shares the scene's `LESS` depth
    /// test, so the smaller depth wins wherever two quads cover the same pixel. A caller
    /// drawing nameplates derives it from distance, which sorts nearer plates in front of
    /// further ones without sorting anything.
    #[cfg(feature = "debug-overlay")]
    pub fn push_overlay_text_at(
        &mut self,
        origin: (f32, f32),
        text: &str,
        color: [f32; 4],
        depth: f32,
    ) {
        let target = self.swap_chain.size();
        self.overlay
            .dynamic
            .push_text_at(origin, text, color, depth, target);
    }

    /// Appends a filled rectangle to the dynamic layer, in pixels from the top-left.
    #[cfg(feature = "debug-overlay")]
    pub fn push_overlay_rect(&mut self, rect: (f32, f32, f32, f32), color: [f32; 4], depth: f32) {
        let target = self.swap_chain.size();
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
        let target = self.swap_chain.size();
        self.overlay
            .dynamic
            .push_outline(rect, thickness, color, depth, target);
    }

    /// Pixel metrics of the overlay font, for a caller sizing a box around its own text.
    ///
    /// Returned rather than exported as constants because the cell size is a runtime
    /// setting: a caller that hardcoded 5x7 cells would misplace every plate the moment the
    /// overlay was scaled for a different resolution.
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
    ///
    /// Independent of the `debug-overlay` feature, which decides whether any of this
    /// exists in the binary at all.
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

    /// Name of the highest supported feature level, e.g. `12_1`.
    #[must_use]
    pub fn feature_level_name(&self) -> &'static str {
        feature_level_name(self.context.feature_level())
    }

    /// Records and submits a frame that clears the back buffer and draws the triangle.
    ///
    /// Does **not** present: [`Renderer::present`] is a separate step so the application
    /// loop keeps the `process_events -> update -> render -> present` shape and can time
    /// the stages independently.
    ///
    /// Performs no heap allocation: the command list, its allocators and the submission
    /// argument are all created once in [`Renderer::new`].
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if recording or submission fails.
    pub fn render(&mut self, color: [f32; 4]) -> Result<(), RendererError> {
        let (slot, back_buffer) = self.begin_recording(color)?;
        self.commands.set_pipeline(
            self.pipeline.root_signature(),
            self.pipeline.pipeline_state(),
        );
        self.commands
            .set_topology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        self.commands.set_vertex_buffer(0, *self.scene.view());
        self.commands.draw(self.scene.vertex_count(), 1);
        self.finish_recording(slot, back_buffer)
    }

    /// Records and submits a frame that draws a [`RenderWorld`] through a [`Camera`].
    ///
    /// Two steps, and they are separate on purpose:
    ///
    /// ```text
    /// build_render_list()  ->  what is visible   (CPU only, no D3D12)
    /// draw_render_list()   ->  record it         (D3D12 only, no visibility logic)
    /// ```
    ///
    /// Every entry in the list still becomes one draw. Sorting, batching and instancing
    /// arrive in later milestones — this is the unoptimized baseline they have to beat.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] if the world names a mesh or material that
    /// was never created, or [`RendererError::Api`] if recording or submission fails.
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
    /// Pure CPU: this touches no D3D12 object, which is what keeps it testable without a
    /// GPU and what will let it move onto worker threads later. It is also why it is a
    /// separate call — "what is visible" and "how it is recorded" are different questions
    /// and a caller may want to ask the first without the second.
    ///
    /// Reuses the list's allocation, so after the first few frames this allocates nothing.
    pub fn build_render_list(&mut self, world: &RenderWorld, camera: &Camera) {
        self.build_render_list_with(world, camera, None);
    }

    /// Rebuilds the render list, optionally across `jobs`' workers.
    ///
    /// `None` culls on the calling thread. `Some` chunks the world and merges the chunks in
    /// world order, producing a list identical to the serial one — the parallelism is an
    /// implementation detail of how the same answer is reached, not a different answer.
    ///
    /// A chunk that panics is logged and the frame falls back to a serial cull, because a
    /// half-filled render list would draw a half-finished scene.
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
        // Timed around the build alone, not the whole frame: the number the overlay reports
        // as "culling" has to be this pass, or it flatters or blames the wrong thing.
        let started = std::time::Instant::now();
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
                    // A half-filled list would draw a half-finished scene, so the frame is
                    // culled again serially rather than submitted as it stands.
                    log::error!("parallel cull failed, falling back to serial: {error}");
                    self.culler.cull(world, &frustum, &mut self.visible);
                }
            }
        }
        self.last_cull_ms = started.elapsed().as_secs_f64() * 1e3;

        // Grouping is timed with the cull rather than with recording: it is list-building
        // work, it runs on the same CPU-only side of the seam, and attributing it to
        // recording would credit the sort with the saving it produces.
        let grouping = std::time::Instant::now();
        self.visible
            .group_by_material_and_mesh(self.materials.len(), self.meshes.len());
        self.last_group_ms = grouping.elapsed().as_secs_f64() * 1e3;

        self.collect_skinned(world, &frustum);
    }

    /// Copies the world's skinned draws into the renderer, keeping the visible ones.
    ///
    /// Done here rather than during recording for the same reason the render list is: this
    /// is CPU-only visibility work, and recording should only have to record. The palette is
    /// copied wholesale — it is a few kilobytes and shared between an instance's submeshes,
    /// so filtering it would cost more index arithmetic than it saves memcpy.
    ///
    /// Culled with the same frustum the rigid entities use, so a character and the ground it
    /// stands on cannot disagree about whether they are on screen.
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
    /// visibility now, recording later. It is a separate call for the same reason — a caller
    /// may want to ask what is visible without recording it — and because it is the *last*
    /// point at which the `RenderWorld` is in scope. Everything the shadow passes will need
    /// from the world is copied here:
    ///
    /// ```text
    /// RenderWorld ->  cascades, caster lists, chosen lights   (the shadow renderer)
    ///             ->  this frame's local lights               (Renderer::lights)
    ///             ->  this frame's skinned draws, unculled    (Renderer::shadow_skinned)
    /// ```
    ///
    /// The two copies are a few kilobytes. They are what lets `draw_render_list` keep the
    /// boundary Milestone 16 established: by the time recording starts, what to draw is
    /// settled and the world is not consulted again.
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

        // The lights are clustered against the same camera the frame is drawn with, and
        // before the shadow stage chooses which of them get maps: the two decisions are
        // independent - a light with no shadow map still lights - and doing the grid here
        // keeps every use of the camera in one function.
        if self.light_clustering {
            self.clusters.build(&self.lights, camera);
        } else {
            self.clusters.disable();
        }

        self.shadow.prepare(world, camera, lighting);
    }

    /// The shadow system, for a caller that wants to change its settings or read its stats.
    #[must_use]
    pub fn shadow_settings(&self) -> &ShadowSettings {
        self.shadow.settings()
    }

    /// Replaces the shadow settings, recreating whatever they invalidate.
    ///
    /// Waits for the GPU first, because a quality change can release the atlas and a texture
    /// released while a frame in flight still reads it is a use-after-free inside the driver.
    /// That makes this a *stall*, and it is why it belongs on a settings screen rather than
    /// on a slider that fires every frame.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if a D3D12 call fails.
    pub fn set_shadow_settings(&mut self, settings: ShadowSettings) -> Result<(), RendererError> {
        self.wait_for_idle()?;
        let size = self.swap_chain.size();
        self.shadow.set_settings(&self.context, settings, size)
    }

    /// Switches the clustered light culling off, or back on.
    ///
    /// See [`Renderer::light_clustering`] for why this exists.
    pub fn set_light_clustering(&mut self, enabled: bool) {
        self.light_clustering = enabled;
    }

    /// Whether the clustered light grid is being built.
    #[must_use]
    pub fn light_clustering(&self) -> bool {
        self.light_clustering
    }

    /// Switches the local lights off, or back on, for the whole frame.
    ///
    /// See [`Renderer::local_lights_enabled`] for why this exists rather than a scene edit.
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
    /// alone with albedo and lighting out of the way, `3` draws the atlas itself in the
    /// bottom-left corner with the cascade tiles outlined.
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

    /// Records the shadow stage, and restores the camera's own targets afterwards.
    ///
    /// ```text
    /// shadow atlas   one depth pass per cascade and per local view
    /// depth prepass  the camera's depth, for the contact-shadow march   (only if enabled)
    /// depth copy     into the texture that march samples                (only if enabled)
    /// -> back buffer and depth rebound, viewport restored
    /// ```
    fn record_shadow_stage(
        &mut self,
        slot: usize,
        back_buffer: usize,
        camera: &Camera,
    ) -> Result<(), RendererError> {
        let scene = ShadowScene {
            meshes: &self.meshes,
            skinned_meshes: &self.skinned_meshes,
            materials: &self.materials,
            material_tables: &self.material_tables,
            skinned: &self.shadow_skinned,
        };
        // Every one of these is a *different field* of `self`, which is what lets the shadow
        // renderer be borrowed mutably while the meshes and materials are borrowed shared.
        self.shadow
            .record(&mut self.commands, &mut self.uploads, slot, &scene)?;

        if self.shadow.needs_prepass() {
            let (width, height) = self.swap_chain.size();
            let dsv = self.depth.view().cpu;
            let scene = ShadowScene {
                meshes: &self.meshes,
                skinned_meshes: &self.skinned_meshes,
                materials: &self.materials,
                material_tables: &self.material_tables,
                skinned: &self.shadow_skinned,
            };
            self.shadow.record_prepass(
                &mut self.commands,
                &mut self.uploads,
                slot,
                &scene,
                &self.visible,
                camera,
                dsv,
                (width, height),
            )?;
            self.shadow
                .copy_scene_depth(&mut self.commands, self.depth.texture());
        }

        // Back to the camera's targets. The shadow passes bound a depth-only target and a
        // viewport the size of an atlas tile; leaving either would render the scene into the
        // corner of the shadow map, which is a spectacular and very confusing failure.
        let rtv = self.swap_chain.rtv_handle(back_buffer);
        let dsv = self.depth.view().cpu;
        self.commands.set_render_target_and_depth(rtv, dsv);
        let (width, height) = self.swap_chain.size();
        self.commands.set_viewport(width, height);
        Ok(())
    }

    /// Records and submits a frame drawing whatever [`Renderer::build_render_list`] left in
    /// the render list.
    ///
    /// Reads the list and the GPU resources it names, and nothing else — in particular it
    /// never looks at the `RenderWorld`. That is the boundary Milestone 16 established: by
    /// this point "what to draw" is settled and recording only has to do it.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] if the list names a mesh or material that
    /// was never created, or [`RendererError::Api`] if recording or submission fails.
    pub fn draw_render_list(
        &mut self,
        camera: &Camera,
        lighting: Lighting,
        color: [f32; 4],
    ) -> Result<(), RendererError> {
        let (slot, back_buffer) = self.begin_recording(color)?;

        // Before the camera pass, and before its pipeline is bound: the shadow stage binds
        // its own depth target, its own viewports and its own pipeline states, and rebinds
        // the camera's when it is done.
        self.gpu_timer
            .begin_pass(self.commands.list(), slot, SHADOW_PASS_QUERY);
        self.record_shadow_stage(slot, back_buffer, camera)?;
        self.gpu_timer
            .end_pass(self.commands.list(), slot, SHADOW_PASS_QUERY);

        let pipeline = if self.wireframe {
            &self.mesh_pipeline_wireframe
        } else {
            &self.mesh_pipeline
        };
        self.commands
            .set_pipeline(pipeline.root_signature(), pipeline.pipeline_state());
        self.commands
            .set_topology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);

        // The frame constants are written once into this slot's upload memory and bound
        // once. This is the upload ring's first real use: transient data, alive for exactly
        // one frame, reclaimed by the reset in `begin_recording`.
        let mut frame_constants =
            FrameConstants::new(camera.view_projection(), camera.position(), lighting);
        frame_constants.set_camera(camera);
        let (viewport_width, viewport_height) = self.swap_chain.size();
        frame_constants.set_viewport(viewport_width, viewport_height);
        self.shadow
            .write_frame_constants(&mut frame_constants, &self.lights);
        frame_constants.set_light_clusters(&self.clusters);
        // The particle pass is recorded later, from a function with no camera. Its basis is
        // captured here, from the same camera the scene is drawn with, so a particle cannot
        // be billboarded against a different frame's view than the one it is drawn into.
        self.particles.constants =
            ParticleConstants::new(&camera.view(), &camera.view_projection());
        let allocation = self.uploads.write_constants(slot, &frame_constants)?;
        self.commands
            .set_root_cbv(ROOT_FRAME_CBV, allocation.gpu_address());
        // The cluster grid rides in the same upload ring as the constants, in its own
        // allocation: 13.5 KB of masks, bound as a root SRV and read by the lighting.
        let clusters = self
            .uploads
            .write(slot, self.clusters.masks(), crate::upload::DEFAULT_ALIGNMENT)?;
        self.commands
            .set_root_srv(ROOT_CLUSTER_SRV, clusters.gpu_address());
        self.bind_shadow_tables();

        if self.instancing {
            return self.draw_batches_instanced(slot, back_buffer);
        }

        // Batch by batch. Grouping put entries sharing a mesh and a material next to each
        // other, so the mesh and material binds happen once per run rather than once per
        // object — and the command context skips them anyway if the run is a continuation
        // of the previous one.
        for batch in self.visible.batches() {
            let Some(mesh) = self.meshes.get(batch.mesh.index()) else {
                return Err(RendererError::InvalidConfig {
                    reason: format!(
                        "render list names mesh {} which does not exist",
                        batch.mesh.index()
                    ),
                });
            };
            let Some(material) = self.materials.get(batch.material.index()) else {
                return Err(RendererError::InvalidConfig {
                    reason: format!(
                        "render list names material {} which does not exist",
                        batch.material.index()
                    ),
                });
            };

            let Some(table) = self.material_table(batch.material) else {
                return Err(RendererError::InvalidConfig {
                    reason: format!(
                        "material {} has no shader-visible base colour map",
                        batch.material.index()
                    ),
                });
            };

            self.commands
                .set_root_cbv(ROOT_MATERIAL_CBV, material.gpu_address());
            self.commands
                .set_root_descriptor_table(ROOT_MATERIAL_TABLE, table);
            self.commands.set_vertex_buffer(0, *mesh.vertex_view());
            self.commands.set_index_buffer(*mesh.index_view());

            let first = batch.first as usize;
            let last = first + batch.count as usize;
            for &entry in &self.visible.order()[first..last] {
                let object = ObjectConstants::shadowed(
                    self.visible.transforms()[entry as usize].matrix(),
                    self.visible.styles()[entry as usize],
                    self.visible.receives_shadow()[entry as usize],
                );
                let allocation = self.uploads.write_constants(slot, &object)?;
                self.commands
                    .set_root_cbv(ROOT_OBJECT_CBV, allocation.gpu_address());
                self.commands.draw_indexed(mesh.index_count(), 1, 0);
            }
        }

        self.draw_skinned(slot)?;
        self.finish_recording(slot, back_buffer)
    }

    /// Records every batch as one instanced draw.
    ///
    /// The pipeline state and the per-object binds are what change. Each batch binds its
    /// mesh and material once, uploads its instances contiguous in the frame's upload ring,
    /// and issues one `DrawIndexedInstanced` — so a run of 3,000 objects costs one draw and
    /// one buffer bind instead of 3,000 draws and 3,000 constant-buffer binds.
    ///
    /// The transforms are composed straight into the upload ring. That is the revisit
    /// Milestone 16 predicted: composing the matrix in the recording loop used to hide under
    /// the per-draw calls, and once those calls are gone there is nothing left to hide it
    /// behind — so it moves to where the data is written, which is a memcpy that was
    /// happening anyway.
    fn draw_batches_instanced(
        &mut self,
        slot: usize,
        back_buffer: usize,
    ) -> Result<(), RendererError> {
        let pipeline = if self.wireframe {
            &self.instanced_pipeline_wireframe
        } else {
            &self.instanced_pipeline
        };
        self.commands
            .set_pipeline(pipeline.root_signature(), pipeline.pipeline_state());

        for batch in self.visible.batches() {
            let Some(mesh) = self.meshes.get(batch.mesh.index()) else {
                return Err(RendererError::InvalidConfig {
                    reason: format!(
                        "render list names mesh {} which does not exist",
                        batch.mesh.index()
                    ),
                });
            };
            let Some(material) = self.materials.get(batch.material.index()) else {
                return Err(RendererError::InvalidConfig {
                    reason: format!(
                        "render list names material {} which does not exist",
                        batch.material.index()
                    ),
                });
            };

            // One allocation for the whole run, so the instances are contiguous — which is
            // what a per-instance vertex stream requires.
            let count = batch.count as usize;
            let bytes = count * size_of::<InstanceTransform>();
            let allocation = self.uploads.allocate(slot, bytes, INSTANCE_ALIGNMENT)?;

            let first = batch.first as usize;
            self.instance_scratch.clear();
            self.instance_scratch.reserve(count);
            for &entry in &self.visible.order()[first..first + count] {
                self.instance_scratch.push(InstanceTransform::shadowed(
                    self.visible.transforms()[entry as usize].matrix(),
                    self.visible.styles()[entry as usize],
                    self.visible.receives_shadow()[entry as usize],
                ));
            }
            allocation.write(&self.instance_scratch)?;

            let Some(table) = self.material_table(batch.material) else {
                return Err(RendererError::InvalidConfig {
                    reason: format!(
                        "material {} has no shader-visible base colour map",
                        batch.material.index()
                    ),
                });
            };

            self.commands
                .set_root_cbv(ROOT_MATERIAL_CBV, material.gpu_address());
            self.commands
                .set_root_descriptor_table(ROOT_MATERIAL_TABLE, table);
            self.commands.set_vertex_buffer(0, *mesh.vertex_view());
            self.commands.set_vertex_buffer(
                1,
                windows::Win32::Graphics::Direct3D12::D3D12_VERTEX_BUFFER_VIEW {
                    BufferLocation: allocation.gpu_address(),
                    SizeInBytes: bytes as u32,
                    StrideInBytes: size_of::<InstanceTransform>() as u32,
                },
            );
            self.commands.set_index_buffer(*mesh.index_view());
            self.commands
                .draw_indexed(mesh.index_count(), batch.count, 0);
        }

        self.draw_skinned(slot)?;
        self.finish_recording(slot, back_buffer)
    }

    /// Records this frame's skinned characters, one draw per submesh.
    ///
    /// Runs after the rigid batches and before the frame is closed, so it inherits the frame
    /// constants already bound and only has to change what differs: the pipeline state, the
    /// palette, and the per-draw object constants.
    ///
    /// **One draw per submesh, no instancing.** Every instance would need its own palette,
    /// which means a structured buffer and a second indirection in the vertex shader — real
    /// work, worth doing when there are enough characters to pay for it. There is one, and
    /// the hundred dummies that *do* need instancing are rigid.
    ///
    /// The palette is written into this frame's upload ring padded to the shader's array
    /// size. Padding rather than writing only the bones in use, because the shader's
    /// `cbuffer` is a fixed-size array and a root CBV shorter than the declared buffer lets
    /// the GPU read past the allocation into whatever the ring hands out next.
    fn draw_skinned(&mut self, slot: usize) -> Result<u32, RendererError> {
        if self.skinned.is_empty() {
            return Ok(0);
        }

        let pipeline = if self.wireframe {
            &self.skinned_pipeline_wireframe
        } else {
            &self.skinned_pipeline
        };
        self.commands
            .set_pipeline(pipeline.root_signature(), pipeline.pipeline_state());
        self.commands
            .set_topology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);

        let mut drawn = 0;
        // Consecutive submeshes of one character name the same palette range, so the upload
        // is skipped for all but the first of them.
        let mut bound_palette: Option<(u32, u32)> = None;

        for index in 0..self.skinned.len() {
            let Some(instance) = self.skinned.instances().get(index).copied() else {
                break;
            };
            let Some(mesh) = self.skinned_meshes.get(instance.mesh.index()) else {
                return Err(RendererError::InvalidConfig {
                    reason: format!(
                        "a skinned draw names skinned mesh {} which does not exist",
                        instance.mesh.index()
                    ),
                });
            };
            let index_count = mesh.index_count();
            let vertex_view = *mesh.vertex_view();
            let index_view = *mesh.index_view();

            let Some(material) = self.materials.get(instance.material.index()) else {
                return Err(RendererError::InvalidConfig {
                    reason: format!(
                        "a skinned draw names material {} which does not exist",
                        instance.material.index()
                    ),
                });
            };
            let material_address = material.gpu_address();
            let Some(table) = self.material_table(instance.material) else {
                return Err(RendererError::InvalidConfig {
                    reason: format!(
                        "material {} has no shader-visible base colour map",
                        instance.material.index()
                    ),
                });
            };

            let range = (instance.palette_start, instance.palette_len);
            if bound_palette != Some(range) {
                let Some(palette) = self.skinned.palette_of(index) else {
                    return Err(RendererError::InvalidConfig {
                        reason: format!(
                            "a skinned draw names bones {}..{} of a {}-matrix palette",
                            instance.palette_start,
                            instance.palette_start + instance.palette_len,
                            self.skinned.palette().len()
                        ),
                    });
                };
                self.palette_scratch.clear();
                self.palette_scratch
                    .extend(palette.iter().map(engine_core::math::Mat4::to_cols_array));
                // The identity for the bones the character does not have. Zeroes would
                // collapse any vertex that reached them to the origin; the identity leaves
                // it in bind pose, which is wrong in a way that is visible and bounded.
                self.palette_scratch.resize(
                    MAX_PALETTE_BONES,
                    engine_core::math::Mat4::IDENTITY.to_cols_array(),
                );

                let allocation = self.uploads.allocate(
                    slot,
                    MAX_PALETTE_BONES * size_of::<[f32; 16]>(),
                    CONSTANT_BUFFER_ALIGNMENT,
                )?;
                allocation.write(&self.palette_scratch)?;
                self.commands
                    .set_root_cbv(ROOT_SKIN_CBV, allocation.gpu_address());
                bound_palette = Some(range);
            }

            let object = ObjectConstants::shadowed(
                instance.transform.matrix(),
                instance.style,
                instance.receives_shadow,
            );
            let allocation = self.uploads.write_constants(slot, &object)?;

            self.commands
                .set_root_cbv(ROOT_MATERIAL_CBV, material_address);
            self.commands
                .set_root_descriptor_table(ROOT_MATERIAL_TABLE, table);
            self.commands
                .set_root_cbv(ROOT_OBJECT_CBV, allocation.gpu_address());
            self.commands.set_vertex_buffer(0, vertex_view);
            self.commands.set_index_buffer(index_view);
            self.commands.draw_indexed(index_count, 1, 0);

            drawn += 1;
        }

        // Draw and triangle counts are not accumulated here: `CommandContext` already counts
        // every `draw_indexed` recorded through it, and a second tally would either
        // double-count or drift.
        self.skinned_draws_last_frame = drawn;
        Ok(drawn)
    }

    /// Skinned submeshes drawn by the most recent frame.
    ///
    /// Reported separately from `draw_calls_last_frame` so the cost of the character pass is
    /// attributable rather than folded into the arena's.
    #[must_use]
    pub fn skinned_draws_last_frame(&self) -> u32 {
        self.skinned_draws_last_frame
    }

    /// Binds the shadow atlas and the scene-depth copy for the camera pass.
    ///
    /// Once per frame, right after the frame constants: both are single-descriptor tables
    /// that change no more often than the frame does, and the lighting shader samples them
    /// for every pixel of every batch.
    fn bind_shadow_tables(&mut self) {
        if let Some(table) = self.shadow.atlas_table() {
            self.commands
                .set_root_descriptor_table(ROOT_SHADOW_TABLE, table);
        }
        if let Some(table) = self.shadow.depth_table() {
            self.commands
                .set_root_descriptor_table(ROOT_DEPTH_TABLE, table);
        }
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
    ///
    /// `render_list().len()` and `cull_stats().visible` are the same number by
    /// construction — the culler derives the counter from the list it filled.
    #[must_use]
    pub fn render_list(&self) -> &RenderList {
        &self.visible
    }

    /// Uploads a mesh and returns a handle to it.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if a D3D12 call fails.
    pub fn create_mesh(&mut self, data: &MeshData) -> Result<MeshHandle, RendererError> {
        let mesh = GpuMesh::upload(&self.context, data)?;
        let index = self.meshes.len() as u32;
        self.meshes.push(mesh);
        Ok(MeshHandle::new(index, 1))
    }

    /// Uploads a skinned mesh and returns a handle to it.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if a D3D12 call fails.
    pub fn create_skinned_mesh(
        &mut self,
        data: &SkinnedMeshData,
    ) -> Result<SkinnedMeshHandle, RendererError> {
        let mesh = GpuMesh::upload_skinned(&self.context, data)?;
        let index = self.skinned_meshes.len() as u32;
        self.skinned_meshes.push(mesh);
        Ok(SkinnedMeshHandle::new(index, 1))
    }

    /// Uploads a texture, creates its shader resource view, and returns a handle.
    ///
    /// The SRV goes into the renderer's one shared descriptor heap, in the persistent region
    /// — the heap is bound once per frame and never changed, so a material bind is a table
    /// offset rather than a heap switch.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] if the texel buffer does not match the
    /// dimensions, or [`RendererError::Api`] if a D3D12 call fails.
    pub fn create_texture(
        &mut self,
        image: &TextureImage<'_>,
    ) -> Result<TextureHandle, RendererError> {
        use windows::Win32::Graphics::Direct3D12::{
            D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
            D3D12_SHADER_RESOURCE_VIEW_DESC, D3D12_SHADER_RESOURCE_VIEW_DESC_0,
            D3D12_SRV_DIMENSION_TEXTURE2D, D3D12_TEX2D_SRV,
        };
        use windows::Win32::Graphics::Dxgi::Common::{
            DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
        };

        let expected = image.width as usize * image.height as usize * 4;
        if image.width == 0 || image.height == 0 || image.texels.len() != expected {
            return Err(RendererError::InvalidConfig {
                reason: format!(
                    "a {}x{} RGBA texture needs {expected} bytes, got {}",
                    image.width,
                    image.height,
                    image.texels.len()
                ),
            });
        }

        // The *view* format carries the colour space, not the resource format: the texels
        // are the same bytes either way, and it is the read that linearizes.
        let format = if image.srgb {
            DXGI_FORMAT_R8G8B8A8_UNORM_SRGB
        } else {
            DXGI_FORMAT_R8G8B8A8_UNORM
        };

        let texture = crate::resource::GpuTexture::new(
            &self.context,
            image.width,
            image.height,
            format,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )?;
        let mut uploader = StagingUploader::new(&self.context)?;
        uploader.queue_texture(
            &self.context,
            &texture,
            image.texels,
            D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
        )?;
        uploader.flush(&self.context)?;

        let descriptor = self.descriptors.allocate_persistent()?;
        let desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
            Format: format,
            ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
            // The default component mapping, spelled out: leaving it zero is an invalid
            // encoding the debug layer rejects.
            Shader4ComponentMapping: 0x1688,
            Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                Texture2D: D3D12_TEX2D_SRV {
                    MostDetailedMip: 0,
                    MipLevels: 1,
                    PlaneSlice: 0,
                    ResourceMinLODClamp: 0.0,
                },
            },
        };
        // SAFETY: FFI call. The texture outlives the renderer, and the CPU handle addresses
        // a slot this heap just allocated.
        unsafe {
            self.context.device().CreateShaderResourceView(
                Some(crate::resource::TrackedResource::resource(&texture)),
                Some(&desc),
                descriptor.cpu,
            );
        }

        let index = self.textures.len() as u32;
        self.textures.push(UploadedTexture {
            texture,
            descriptor,
        });
        Ok(TextureHandle(index))
    }

    /// Uploads an [`engine_assets::ModelAsset`] and reports what it became.
    ///
    /// Every level of detail's every submesh becomes one mesh and one material, and the
    /// returned [`ModelBinding`] is the description extraction reads — which is why it is a
    /// `engine-render-data` type rather than one of the renderer's: presentation must be able
    /// to place a model without linking D3D12.
    ///
    /// The model is validated first. A model that fails is **not** uploaded, and the error
    /// names the model and the place inside it.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] carrying the validation failure, or
    /// [`RendererError::Api`] if a D3D12 call fails.
    pub fn create_model(
        &mut self,
        model: &engine_assets::ModelAsset,
    ) -> Result<ModelBinding, RendererError> {
        use engine_assets::mesh::Geometry;

        engine_assets::validate_model(model, &engine_assets::ValidationLimits::default()).map_err(
            |error| RendererError::InvalidConfig {
                reason: error.to_string(),
            },
        )?;

        // Textures first, then materials that name them, then geometry that names materials.
        let mut textures = Vec::with_capacity(model.textures.len());
        for texture in &model.textures {
            textures.push(self.create_texture(&TextureImage {
                width: texture.width,
                height: texture.height,
                srgb: texture.format.is_srgb(),
                texels: &texture.texels,
            })?);
        }

        let mut materials = Vec::with_capacity(model.materials.len());
        for material in &model.materials {
            let desc = MaterialDesc {
                base_color: material.base_color,
                roughness: material.roughness,
                metallic: material.metallic,
                emissive: material.emissive,
                // Carried through to the shadow pass as well as the camera pass: a cutout
                // material that shadowed as an opaque rectangle would be worse than one
                // that did not shadow at all.
                alpha_cutoff: material.alpha_cutoff,
            };
            let texture = material
                .base_color_texture
                .and_then(|index| textures.get(index).copied());
            materials.push(self.create_material_textured(desc, texture)?);
        }

        let mut lods = Vec::with_capacity(model.lods.len());
        for lod in &model.lods {
            let mut parts = Vec::with_capacity(lod.submeshes.len());
            for submesh in &lod.submeshes {
                let geometry = match &submesh.geometry {
                    Geometry::Static(vertices) => {
                        let vertices = vertices
                            .iter()
                            .map(|vertex| {
                                MeshVertex::new(vertex.position, vertex.normal, vertex.uv)
                            })
                            .collect();
                        let data = MeshData::new(vertices, submesh.indices.clone())?;
                        PartGeometry::Rigid(self.create_mesh(&data)?)
                    }
                    Geometry::Skinned(vertices) => {
                        let vertices = vertices
                            .iter()
                            .map(|vertex| {
                                SkinnedVertex::new(
                                    vertex.position,
                                    vertex.normal,
                                    vertex.uv,
                                    vertex.joints,
                                    vertex.weights,
                                )
                            })
                            .collect();
                        let data = SkinnedMeshData::new(vertices, submesh.indices.clone())?;
                        PartGeometry::Skinned(self.create_skinned_mesh(&data)?)
                    }
                };
                let Some(material) = materials.get(submesh.material).copied() else {
                    return Err(RendererError::InvalidConfig {
                        reason: format!(
                            "'{}' submesh '{}' names material {}, and validation should have \
                             caught that",
                            model.name, submesh.name, submesh.material
                        ),
                    });
                };
                parts.push(ModelPart {
                    geometry,
                    material,
                    bounds: submesh.bounds,
                });
            }
            lods.push(ModelLodBinding {
                parts,
                min_distance: lod.min_distance,
                bounds: lod.bounds(),
            });
        }

        Ok(ModelBinding {
            lods,
            skinned: model.is_skinned(),
            // Measured from LOD0's own vertices, so a binding is scalable even for a model
            // whose author never said how tall it is. A *character* uses its
            // `CharacterDef::model_height` instead, which is the authored figure — the
            // FireMage's staff reaches 2.22 m and the mage is 1.80, and it is the mage that
            // has to match the capsule.
            model_height: measured_height(model),
        })
    }

    /// Creates the UI pipeline and uploads the UI atlas.
    ///
    /// The atlas holds the font and every ability icon, and is uploaded exactly once, at
    /// renderer creation, through the same staging path meshes use. Nothing about it changes
    /// per frame.
    fn create_ui_state(
        context: &RenderDevice,
        descriptors: &mut GpuDescriptorHeap,
    ) -> Result<UiState, RendererError> {
        use windows::Win32::Graphics::Direct3D12::{
            D3D12_RESOURCE_STATE_COPY_DEST, D3D12_SHADER_RESOURCE_VIEW_DESC,
            D3D12_SHADER_RESOURCE_VIEW_DESC_0, D3D12_SRV_DIMENSION_TEXTURE2D, D3D12_TEX2D_SRV,
        };
        use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;

        let pipeline = UiPipeline::new(context.device())?;
        let atlas_pixels = UiAtlas::new();
        let atlas = crate::resource::GpuTexture::new(
            context,
            atlas_pixels.width(),
            atlas_pixels.height(),
            DXGI_FORMAT_R8G8B8A8_UNORM,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )?;

        let mut uploader = StagingUploader::new(context)?;
        uploader.queue_texture(
            context,
            &atlas,
            atlas_pixels.texels(),
            windows::Win32::Graphics::Direct3D12::D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
        )?;
        uploader.flush(context)?;

        // One slot out of the renderer's shared heap. The UI binds a single SRV that never
        // changes; what changed in Phase 5 is that it is no longer the only one.
        let atlas_descriptor = descriptors.allocate_persistent()?;

        let desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
            // The default component mapping, spelled out: the four channels arrive in the
            // order they were written, and leaving this zero is an invalid encoding the
            // debug layer rejects.
            Shader4ComponentMapping: 0x1688,
            Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                Texture2D: D3D12_TEX2D_SRV {
                    MostDetailedMip: 0,
                    MipLevels: 1,
                    PlaneSlice: 0,
                    ResourceMinLODClamp: 0.0,
                },
            },
        };
        // SAFETY: FFI call. The texture and the descriptor heap outlive the renderer, and
        // the CPU handle addresses a slot this heap just allocated.
        unsafe {
            context.device().CreateShaderResourceView(
                Some(crate::resource::TrackedResource::resource(&atlas)),
                Some(&desc),
                atlas_descriptor.cpu,
            );
        }

        Ok(UiState {
            pipeline,
            batcher: UiBatcher::new(MAX_UI_QUADS),
            atlas,
            atlas_descriptor,
            list: engine_render_data::UiDrawList::with_capacity(1024, 4096),
            stats: UiBatchStats::default(),
            build_ms: 0.0,
            enabled: true,
        })
    }

    /// Replaces the UI to draw this frame and batches it into vertices.
    ///
    /// Batching happens here rather than during recording so its cost is attributable and
    /// so `render` stays a function of data that is already prepared.
    pub fn set_ui(&mut self, list: &engine_render_data::UiDrawList) {
        let started = std::time::Instant::now();
        self.ui.list.clear();
        self.ui.list.append(list);
        self.ui.batcher.build(&self.ui.list);
        self.ui.stats = self.ui.batcher.stats();
        self.ui.build_ms = started.elapsed().as_secs_f64() * 1e3;
    }

    /// The list the renderer will draw, for a caller that wants to build into it directly.
    ///
    /// Building in place avoids the copy `set_ui` performs. The caller must call
    /// [`Renderer::batch_ui`] afterwards.
    pub fn ui_list_mut(&mut self) -> &mut engine_render_data::UiDrawList {
        &mut self.ui.list
    }

    /// Batches whatever is currently in the UI list.
    pub fn batch_ui(&mut self) {
        let started = std::time::Instant::now();
        self.ui.batcher.build(&self.ui.list);
        self.ui.stats = self.ui.batcher.stats();
        self.ui.build_ms = started.elapsed().as_secs_f64() * 1e3;
    }

    /// What the last UI batch produced.
    #[must_use]
    pub fn ui_stats(&self) -> UiBatchStats {
        self.ui.stats
    }

    /// Milliseconds the last UI batch build took.
    #[must_use]
    pub fn ui_build_ms(&self) -> f64 {
        self.ui.build_ms
    }

    /// Shows or hides the whole UI pass.
    pub fn set_ui_enabled(&mut self, enabled: bool) {
        self.ui.enabled = enabled;
    }

    /// Whether the UI pass draws.
    #[must_use]
    pub fn ui_enabled(&self) -> bool {
        self.ui.enabled
    }

    /// Records the UI as one draw call, if there is anything to draw.
    ///
    /// Returns the vertices submitted. The whole interface is one vertex buffer against one
    /// texture through one pipeline state, so this is a single `DrawInstanced` regardless of
    /// how many elements the HUD contains.
    fn draw_ui(&mut self, slot: usize) -> Result<u32, RendererError> {
        if !self.ui.enabled {
            return Ok(0);
        }
        let vertices = self.ui.batcher.vertices();
        if vertices.is_empty() {
            return Ok(0);
        }

        let bytes = std::mem::size_of_val(vertices);
        let allocation = self.uploads.allocate(slot, bytes, UI_VERTEX_ALIGNMENT)?;
        allocation.write(vertices)?;

        let (width, height) = self.swap_chain.size();
        let constants = UiFrameConstants::for_viewport(width, height);
        let constant_allocation = self.uploads.write_constants(slot, &constants)?;

        self.commands
            .set_pipeline(self.ui.pipeline.root_signature(), self.ui.pipeline.state());
        self.commands
            .set_root_cbv(ROOT_UI_FRAME_CBV, constant_allocation.gpu_address());
        let Some(table) = self.ui.atlas_descriptor.gpu else {
            return Err(RendererError::InvalidConfig {
                reason: "the font atlas descriptor is not shader visible".to_owned(),
            });
        };
        self.commands
            .set_root_descriptor_table(ROOT_UI_ATLAS_TABLE, table);
        self.commands
            .set_topology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        self.commands.set_vertex_buffer(
            0,
            windows::Win32::Graphics::Direct3D12::D3D12_VERTEX_BUFFER_VIEW {
                BufferLocation: allocation.gpu_address(),
                SizeInBytes: bytes as u32,
                StrideInBytes: size_of::<UiVertex>() as u32,
            },
        );
        let count = vertices.len() as u32;
        self.commands.draw(count, 1);
        Ok(count)
    }

    /// Replaces this frame's particles.
    ///
    /// A copy rather than a borrow, for the same reason [`Renderer::set_ui`] copies: the
    /// caller owns its list across frames and the renderer needs the data to survive until
    /// recording. Use [`Renderer::particle_list_mut`] to build into the renderer's own list
    /// and skip the copy.
    pub fn set_particles(&mut self, list: &engine_render_data::ParticleDrawList) {
        self.particles.list.clear();
        for blend in engine_render_data::ParticleBlend::ALL {
            for particle in list.of(blend) {
                self.particles.list.push(blend, *particle);
            }
        }
    }

    /// The list the renderer will draw, for a caller that fills it directly.
    pub fn particle_list_mut(&mut self) -> &mut engine_render_data::ParticleDrawList {
        &mut self.particles.list
    }

    /// Particles submitted by the most recent frame.
    #[must_use]
    pub fn particles_last_frame(&self) -> u32 {
        self.particles.drawn
    }

    /// Turns the whole particle pass on or off.
    ///
    /// What a benchmark's particle-free baseline uses, and what answers "how much of this
    /// frame is fire?" without rebuilding the scene around it.
    pub fn set_particles_enabled(&mut self, enabled: bool) {
        self.particles.enabled = enabled;
    }

    /// Whether the particle pass draws.
    #[must_use]
    pub fn particles_enabled(&self) -> bool {
        self.particles.enabled
    }

    /// Records the particle pass: one draw per blend mode, whatever the particle count.
    ///
    /// Each mode's records are uploaded contiguously into this frame's ring and drawn as
    /// `DrawInstanced(4, count)` over a triangle strip — the quad's corners come from
    /// `SV_VertexID`, so there is no vertex buffer of geometry at all and no index buffer.
    ///
    /// Returns the particles submitted.
    fn draw_particles(&mut self, slot: usize) -> Result<u32, RendererError> {
        self.particles.drawn = 0;
        if !self.particles.enabled || self.particles.list.is_empty() {
            return Ok(0);
        }

        let constants = self.particles.constants;
        let constant_allocation = self.uploads.write_constants(slot, &constants)?;

        self.commands
            .set_topology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);

        let mut drawn = 0;
        for blend in engine_render_data::ParticleBlend::ALL {
            let instances = self.particles.list.of(blend);
            let count = instances.len();
            if count == 0 {
                continue;
            }
            let bytes = std::mem::size_of_val(instances);
            let allocation = self.uploads.allocate(slot, bytes, PARTICLE_ALIGNMENT)?;
            allocation.write(self.particles.list.of(blend))?;

            self.commands.set_pipeline(
                self.particles.pipeline.root_signature(),
                self.particles.pipeline.state(blend),
            );
            self.commands
                .set_root_cbv(ROOT_PARTICLE_FRAME_CBV, constant_allocation.gpu_address());
            self.commands.set_vertex_buffer(
                0,
                windows::Win32::Graphics::Direct3D12::D3D12_VERTEX_BUFFER_VIEW {
                    BufferLocation: allocation.gpu_address(),
                    SizeInBytes: bytes as u32,
                    StrideInBytes: size_of::<engine_render_data::ParticleInstance>() as u32,
                },
            );
            // Four vertices, `count` instances: the whole mode is one draw call.
            self.commands.draw(4, count as u32);
            drawn += count as u32;
        }

        self.particles.drawn = drawn;
        Ok(drawn)
    }

    /// Uploads a material and returns a handle to it.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] for out-of-range parameters, or
    /// [`RendererError::Api`] if a D3D12 call fails.
    pub fn create_material(&mut self, desc: MaterialDesc) -> Result<MaterialHandle, RendererError> {
        self.create_material_textured(desc, None)
    }

    /// Uploads a material that samples `texture` for its base colour.
    ///
    /// `None` binds the shared 1×1 white texture, so every material takes one code path and
    /// the shader needs no branch. The map *multiplies* the material's base colour rather
    /// than replacing it, which is what lets one texture serve several tinted materials.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] for out-of-range parameters or a texture
    /// handle that names nothing, or [`RendererError::Api`] if a D3D12 call fails.
    pub fn create_material_textured(
        &mut self,
        desc: MaterialDesc,
        texture: Option<TextureHandle>,
    ) -> Result<MaterialHandle, RendererError> {
        let texture = texture.unwrap_or(self.white_texture);
        if texture.index() >= self.textures.len() {
            return Err(RendererError::InvalidConfig {
                reason: format!(
                    "material names texture {}, and {} exist",
                    texture.index(),
                    self.textures.len()
                ),
            });
        }
        let material = Material::new(&self.context, desc)?;
        let index = self.materials.len() as u32;
        self.materials.push(material);
        self.material_textures.push(texture);
        // Resolved now, once: a texture's descriptor is fixed at creation and never moves,
        // so there is nothing to invalidate it later.
        self.material_tables.push(
            self.textures
                .get(texture.index())
                .and_then(|uploaded| uploaded.descriptor.gpu),
        );
        Ok(MaterialHandle::new(index, 1))
    }

    /// The descriptor table to bind for `material`'s base colour map.
    ///
    /// Returns `None` for a material that was never created, or whose texture's descriptor
    /// is somehow not shader visible — both of which are recording bugs rather than content
    /// ones, and both of which the caller turns into a named error rather than a wrong
    /// picture.
    fn material_table(
        &self,
        material: MaterialHandle,
    ) -> Option<windows::Win32::Graphics::Direct3D12::D3D12_GPU_DESCRIPTOR_HANDLE> {
        self.material_tables
            .get(material.index())
            .copied()
            .flatten()
    }

    /// What the most recent `render_world` cull pass did.
    #[must_use]
    pub fn cull_stats(&self) -> CullStats {
        self.culler.stats()
    }

    /// Turns frustum culling on or off.
    ///
    /// Exists so a benchmark can measure the same frame with one step removed, rather than
    /// comparing two different code paths and attributing the difference to culling.
    pub fn set_culling_enabled(&mut self, enabled: bool) {
        self.culling_enabled = enabled;
    }

    /// Whether frustum culling is on.
    #[must_use]
    pub fn culling_enabled(&self) -> bool {
        self.culling_enabled
    }

    /// Milliseconds the most recent `render_world` cull pass took.
    ///
    /// Zero before the first frame. Measured around the cull loop only, so it is comparable
    /// against the render stage rather than a share of it.
    #[must_use]
    pub fn last_cull_ms(&self) -> f64 {
        self.last_cull_ms
    }

    /// Culls against `view_projection` instead of the camera's own, or clears the override.
    ///
    /// Freezing the frustum is the only way to *see* culling: the objects it removes are
    /// exactly the ones outside the view, so with a live frustum there is nothing on screen
    /// to look at. With the frustum frozen and the camera free, the culled volume becomes a
    /// visible hole in the scene.
    pub fn set_cull_view_projection(&mut self, view_projection: Option<engine_core::math::Mat4>) {
        self.cull_view_projection = view_projection;
    }

    /// Whether the cull frustum is frozen away from the camera.
    #[must_use]
    pub fn cull_frustum_is_frozen(&self) -> bool {
        self.cull_view_projection.is_some()
    }

    /// Draws the world as wireframe instead of solid.
    ///
    /// Switches between two pipeline states created at startup; the fill mode is baked into
    /// a D3D12 PSO and cannot be changed on a bound one.
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

    /// Opens a frame: waits for the slot, resets per-frame state, and clears the targets.
    ///
    /// Shared by [`Renderer::render`] and [`Renderer::render_world`] so the two cannot
    /// drift apart — the pacing, the timestamp read, the deferred releases and the ring
    /// reset all have to happen in exactly this order, and having that written twice is how
    /// one of them ends up missing.
    fn begin_recording(&mut self, color: [f32; 4]) -> Result<(usize, usize), RendererError> {
        // Frame pacing lives in the ring: it waits for this slot's previous submission,
        // resets the allocator, and records whether the wait actually blocked.
        let slot = self.frames.begin_frame()?;

        // The wait inside `begin_frame` is exactly the point where this slot's previous
        // timestamps are known to be resolved, so reading them costs no extra
        // synchronization. The value describes the frame one lap ago, not the one about to
        // be recorded — see `gpu_timer` for why that lag is the right trade.
        if let Some(milliseconds) = self.gpu_timer.read_frame_ms(slot)? {
            self.last_gpu_frame_ms = Some(milliseconds);
        }
        // The shadow stage's own timestamps, resolved by the same call and read at the same
        // safe point. It lags by the same few frames the frame time does, which the overlay
        // says rather than pretending otherwise.
        let shadow_ms = self.gpu_timer.read_pass_ms(slot, SHADOW_PASS_QUERY)?;
        self.shadow.set_gpu_ms(shadow_ms);

        // Resources whose last use the GPU has now passed. Costs a comparison per frame
        // when the queue is empty, which it usually is.
        self.pending_release.collect(self.frames.completed_value());

        // This slot's upload memory is free again, for the same reason its allocator is:
        // the ring's wait proved the GPU finished the frame that used it.
        self.uploads.begin_frame(slot);

        // The back buffer to draw into is the swap chain's business, and is deliberately
        // not assumed to equal the frame slot.
        let back_buffer = self.swap_chain.current_back_buffer_index();

        // SAFETY: FFI call. The list is closed at this point (either from `new` or from
        // the previous frame), which is what Reset requires, and the allocator was just
        // reset by the ring after proving the GPU was done with it.
        self.commands.begin(self.frames.allocator(slot))?;

        // The one shader-visible descriptor heap, bound once for the whole frame. Every pass
        // that samples anything — the mesh passes' base colour maps, the UI atlas — reads
        // out of it, so it is never switched mid-list.
        self.commands
            .set_descriptor_heaps(&[Some(self.descriptors.heap().clone())]);

        self.gpu_timer.begin_frame(self.commands.list(), slot);

        let buffer = self.swap_chain.buffer(back_buffer);
        let rtv = self.swap_chain.rtv_handle(back_buffer);
        let (width, height) = self.swap_chain.size();

        // Transition by destination only: the back buffer knows it is in PRESENT, so the
        // "before" state cannot be stated wrongly. Queued, not submitted — the context
        // flushes it before the clear below, which is the first command that depends on it.
        self.commands
            .transition_to(buffer, D3D12_RESOURCE_STATE_RENDER_TARGET);
        // The depth buffer stays in DEPTH_WRITE for its whole life — nothing samples it —
        // so this transition is a comparison that finds the state already correct and
        // records a redundant transition rather than a barrier.
        self.commands
            .transition_to(self.depth.texture(), D3D12_RESOURCE_STATE_DEPTH_WRITE);
        let dsv = self.depth.view().cpu;
        self.commands.set_render_target_and_depth(rtv, dsv);
        self.commands.clear_render_target(rtv, color);
        self.commands.clear_depth(dsv, DEPTH_CLEAR);
        self.commands.set_viewport(width, height);
        Ok((slot, back_buffer))
    }

    /// Closes a frame: draws the overlay, returns the back buffer to `PRESENT`, and submits.
    fn finish_recording(&mut self, slot: usize, back_buffer: usize) -> Result<(), RendererError> {
        // Before the interface and after the scene: a blended particle has to see the opaque
        // world beneath it, and a health bar under a fireball is a health bar nobody can
        // read.
        self.draw_particles(slot)?;

        #[cfg(feature = "debug-overlay")]
        let index = slot;

        // The overlay needs its own pipeline: it draws clip-space position+colour vertices,
        // which the mesh pipeline's input layout does not describe. Binding it here rather
        // than assuming whatever the scene left bound is what makes both draw paths work.
        #[cfg(feature = "debug-overlay")]
        {
            self.overlay.drawn_vertices = 0;
            if self.overlay.enabled {
                // Upload only when this slot's buffer no longer matches the text. Safe to
                // overwrite then: the pacing wait above proved the GPU finished the
                // previous frame that used this slot.
                if self.overlay.stale[index] {
                    self.overlay.views[index] = self
                        .overlay
                        .buffers
                        .upload(index, self.overlay.geometry.vertices());
                    self.overlay.stale[index] = false;
                }

                if let Some(view) = self.overlay.views[index] {
                    let count = view.SizeInBytes / view.StrideInBytes;
                    self.commands.set_pipeline(
                        self.pipeline.root_signature(),
                        self.pipeline.pipeline_state(),
                    );
                    self.commands
                        .set_topology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
                    self.commands.set_vertex_buffer(0, view);
                    self.commands.draw(count, 1);
                    self.overlay.drawn_vertices = count;
                }
            }

            // The dynamic layer is drawn whether or not the developer HUD is enabled:
            // nameplates are game UI, and F5 hides the statistics rather than the game's
            // own readouts. Uploaded unconditionally, because it is rebuilt every frame by
            // construction and a staleness check would always say "stale".
            if !self.overlay.dynamic.is_empty()
                && let Some(view) = self
                    .overlay
                    .dynamic_buffers
                    .upload(index, self.overlay.dynamic.vertices())
            {
                let count = view.SizeInBytes / view.StrideInBytes;
                self.commands.set_pipeline(
                    self.pipeline.root_signature(),
                    self.pipeline.pipeline_state(),
                );
                self.commands
                    .set_topology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
                self.commands.set_vertex_buffer(0, view);
                self.commands.draw(count, 1);
                self.overlay.drawn_vertices += count;
            }
        }

        // Last, and on top of everything: the UI blends, so it must see the finished scene
        // beneath it. One draw call for the whole interface.
        self.draw_ui(slot)?;

        // Re-borrowed after `draw_ui`, which needs `&mut self`. The buffer is the same
        // resource either way; holding the earlier borrow across the call would only be a
        // borrow-checker argument, not a correctness one.
        let buffer = self.swap_chain.buffer(back_buffer);
        self.commands
            .transition_to(buffer, D3D12_RESOURCE_STATE_PRESENT);

        // The closing timestamp brackets the frame's own work; the resolve that follows it
        // is a GPU-side copy into the readback buffer. Barriers are flushed first so the
        // transition is recorded before the timestamp that closes the frame.
        self.commands.flush_barriers();
        self.gpu_timer.end_frame(self.commands.list(), slot);

        let stats = self.commands.stats();
        self.draw_calls_last_frame = stats.draw_calls;
        self.triangles_last_frame = stats.triangles;
        self.command_stats = stats;

        self.commands.close()?;

        // SAFETY: FFI call submitting the closed list. `execute_lists` holds exactly the
        // list closed above and outlives the call.
        unsafe {
            self.context
                .queue()
                .ExecuteCommandLists(&self.execute_lists);
        }

        self.recorded_frame_slot = Some(slot);
        self.recorded_back_buffer = back_buffer;
        Ok(())
    }

    /// Presents the frame recorded by [`Renderer::render`] and signals its fence.
    ///
    /// The fence is signalled after `Present` so the slot's value covers the whole
    /// submission; the next use of that slot waits on it before resetting its allocator.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if `Present` or the fence signal fails.
    pub fn present(&mut self) -> Result<(), RendererError> {
        let Some(slot) = self.recorded_frame_slot.take() else {
            return Err(RendererError::InvalidConfig {
                reason: "present called without a recorded frame".to_owned(),
            });
        };

        // The frame was recorded against a specific back buffer. If the swap chain moved
        // on between recording and presenting, something reset it behind the renderer's
        // back and the presented image would not be the one recorded.
        debug_assert_eq!(
            self.recorded_back_buffer,
            self.swap_chain.current_back_buffer_index(),
            "the swap chain advanced between render and present"
        );

        // Split, because the engine's "present" stage has never been attributed and at
        // 100,000 objects it is 1.11 ms of a 4.17 ms frame. Two Instant reads per frame is
        // roughly 40 ns against a millisecond being explained, which is a trade worth
        // making permanently rather than behind a flag.
        let present_start = std::time::Instant::now();
        self.swap_chain.present()?;
        self.last_present_ms = present_start.elapsed().as_secs_f64() * 1e3;
        self.present_window.record_ms(self.last_present_ms);

        // Signalled after Present, so the slot's fence value covers the whole submission;
        // this also advances the ring to the next slot.
        let signal_start = std::time::Instant::now();
        self.frames.end_frame(self.context.queue(), slot)?;
        self.last_fence_signal_ms = signal_start.elapsed().as_secs_f64() * 1e3;

        self.frames_presented += 1;
        Ok(())
    }

    /// Milliseconds the most recent `IDXGISwapChain::Present` call took.
    ///
    /// With vsync off and tearing allowed, `Present(0, ALLOW_TEARING)` is documented to
    /// return without waiting for a vertical blank — but it still blocks once every back
    /// buffer is spoken for, and that is a queue the engine has never measured. The frame
    /// ring's stall counter does not see it: that measures the *fence* wait at the top of
    /// the next frame, which is a different queue entirely.
    #[must_use]
    pub fn last_present_ms(&self) -> f64 {
        self.last_present_ms
    }

    /// Distribution of `Present` durations over the recent window.
    ///
    /// `None` before the first frame. Sorting costs O(n log n) in the window size, so read
    /// this on a reporting interval rather than per frame.
    pub fn present_stats(&mut self) -> Option<FrameStats> {
        self.present_window.stats()
    }

    /// Forgets the recorded `Present` durations.
    ///
    /// Called when the workload changes: the previous scene's present times describe a
    /// different amount of GPU work to wait for.
    pub fn reset_present_stats(&mut self) {
        self.present_window.clear();
    }

    /// Milliseconds the most recent fence signal after `Present` took.
    ///
    /// Separated from `Present` itself so a slow present stage cannot be blamed on the
    /// signal, or the other way round. This should be a fraction of a microsecond; if it is
    /// not, the queue is doing something unexpected.
    #[must_use]
    pub fn last_fence_signal_ms(&self) -> f64 {
        self.last_fence_signal_ms
    }

    /// How often, and for how long, the CPU has waited for the GPU.
    ///
    /// A high stall ratio means the renderer is GPU-bound, which is the healthy state for
    /// an uncapped loop; a low one means the CPU is the limit. The number exists so that
    /// claim is measured rather than assumed.
    #[must_use]
    pub fn sync_stats(&self) -> SyncStats {
        self.frames.stats()
    }

    /// Number of frames the CPU may run ahead of the GPU.
    #[must_use]
    pub fn frames_in_flight(&self) -> usize {
        self.frames.frames_in_flight()
    }

    /// Resizes the swap chain, waiting for the GPU to finish with the old buffers first.
    ///
    /// A zero dimension (a minimized window) is ignored: there is nothing to present to.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if the idle wait or `ResizeBuffers` fails.
    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), RendererError> {
        if width == 0 || height == 0 {
            log::debug!("ignoring resize to {width}x{height}");
            return Ok(());
        }
        if (width, height) == self.swap_chain.size() {
            return Ok(());
        }

        self.wait_for_idle()?;

        // A frame recorded against the old back buffers must not be presented after the
        // buffers are replaced; its work is already submitted and complete after the
        // idle wait above, so dropping the pending present is the correct recovery.
        if self.recorded_frame_slot.take().is_some() {
            log::debug!("dropped a recorded frame because the swap chain was resized");
        }

        // A recorded command list keeps references to every resource it touched, and
        // those references live until the list is reset. ResizeBuffers requires that no
        // reference to a back buffer survives anywhere, so the list is reset to an empty
        // state here. Skipping this leaves the old buffers referenced and corrupts the
        // driver's state — it manifests as an access violation under concurrent GPU work
        // rather than as a clean HRESULT failure.
        self.reset_command_list_to_empty()?;

        self.swap_chain.resize(&self.context, width, height)?;
        // After the idle wait above, nothing references the old depth texture, so replacing
        // it is safe. It must match the back buffer exactly: a depth buffer smaller than
        // the render target is a draw-time error, and a larger one wastes memory.
        self.depth.resize(&self.context, width, height)?;
        // The contact-shadow march samples a copy of the depth buffer, so it is the same
        // size and is replaced at the same moment. The shadow *atlas* is unaffected: its
        // size comes from the quality settings and not from the window.
        self.shadow.resize(&self.context, (width, height))?;

        // After ResizeBuffers every slot is free again; forget the old fence values so
        // the next frames do not wait on submissions that referenced destroyed buffers.
        self.frames.release_all_slots();
        Ok(())
    }

    /// Resets the command list without recording anything, releasing every resource
    /// reference it still holds.
    ///
    /// The caller must have waited for the GPU to go idle: resetting an allocator whose
    /// submissions are still executing is undefined behaviour in D3D12.
    fn reset_command_list_to_empty(&mut self) -> Result<(), RendererError> {
        // Cloned because `reset_to_empty` needs `&mut self.commands` while the allocator
        // is borrowed from `self.frames`; the clone is a refcount bump on a COM pointer.
        let allocator = self.frames.allocator(0).clone();
        self.commands.reset_to_empty(&allocator)
    }

    /// What the most recent frame's command recording cost.
    #[must_use]
    pub fn command_stats(&self) -> CommandStats {
        self.command_stats
    }

    /// Per-frame upload memory, for callers that need to stage dynamic data.
    #[must_use]
    pub fn uploads(&mut self) -> &mut UploadRing {
        &mut self.uploads
    }

    /// Largest per-frame upload usage observed.
    ///
    /// The number that sizes `RendererConfig::upload_bytes_per_frame`.
    #[must_use]
    pub fn upload_peak_bytes(&self) -> usize {
        self.uploads.peak()
    }

    /// Resources waiting for the GPU before they can be released.
    ///
    /// A number that keeps growing means resources are being released faster than frames
    /// complete, which is a leak in slow motion.
    #[must_use]
    pub fn pending_releases(&self) -> usize {
        self.pending_release.pending()
    }

    /// Blocks until the GPU has finished all submitted work.
    ///
    /// Fully serializing — for shutdown and resize only, never per frame.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if the fence signal or wait fails.
    pub fn wait_for_idle(&mut self) -> Result<(), RendererError> {
        self.frames.wait_for_idle(self.context.queue())
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        // The GPU may still be reading the swap-chain buffers and the command allocators
        // this struct is about to release. Releasing them early is exactly what produces
        // "device removed" reports and debug-layer live-object errors at shutdown.
        if let Err(error) = self.wait_for_idle() {
            log::error!("GPU did not go idle during shutdown: {error}");
        }
        let sync = self.frames.stats();
        log::info!(
            "renderer shut down after {} frames ({} frames in flight)",
            self.frames_presented,
            self.frames.frames_in_flight()
        );
        // Which processor was the limit, as a measurement rather than an assumption: a
        // high ratio means the CPU spent the frame waiting for the GPU.
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
