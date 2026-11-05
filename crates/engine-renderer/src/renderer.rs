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
use std::collections::HashMap;

use crate::gpu_timer::{
    CONTACT_PASS, DEPTH_PREPASS, GpuTimer, MAX_TIMED_PASSES, PARTICLE_PASS, SCENE_PASS,
    SHADOW_ATLAS_PASS, SHADOW_PASS as SHADOW_PASS_QUERY, UI_PASS,
};
use crate::light_cluster::{LightClusterStats, LightClusters};
use crate::material::{
    MATERIAL_TEXTURE_COUNT, Material, MaterialDesc, MaterialTextureSet, MaterialTextureSlot,
    MaterialTextures,
};
use crate::mesh::{GpuMesh, MeshData, MeshVertex, SkinnedMeshData, SkinnedVertex, VertexStream};
use crate::mesh_pipeline::{
    CharacterPass, DepthMode, InstanceTransform, MeshPipelines, PipelineVariant,
};
#[cfg(feature = "debug-overlay")]
use crate::overlay::{OverlayBuffers, OverlayGeometry, OverlayMetrics};
use crate::particle_pipeline::{ParticleConstants, ParticlePipeline, ROOT_PARTICLE_FRAME_CBV};
use crate::pipeline::{ScenePipeline, Vertex};
use crate::render_list::RenderList;
use crate::resource::{BufferArena, BufferSource, DeferredReleaseQueue};
use crate::scene::{
    FrameConstants, ObjectConstants, ROOT_CLUSTER_SRV, ROOT_CONTACT_TABLE, ROOT_DEPTH_TABLE,
    ROOT_FRAME_CBV, ROOT_MATERIAL_CBV, ROOT_MATERIAL_TABLE, ROOT_OBJECT_CBV, ROOT_SHADOW_TABLE,
    ROOT_SKIN_CBV,
};
use crate::shader::ShaderFeatures;
use crate::shadow::renderer::ShadowScene;
use crate::shadow::{ShadowRenderer, ShadowSettings, ShadowStats};
use crate::stress;
use crate::swapchain::SwapChain;
pub use crate::texture::{
    PreparedTexture, PreparedTextures, TextureHandle, TextureImage, prepare_model_textures,
    prepare_texture,
};
use crate::texture::{TextureIdentity, texture_kinds};
use crate::texture_id::TextureKey;
use crate::ui_atlas::UiAtlas;
use crate::ui_pipeline::{
    ROOT_UI_ATLAS_TABLE, ROOT_UI_FRAME_CBV, UiBatchStats, UiBatcher, UiFrameConstants, UiPipeline,
    UiVertex,
};
use crate::upload::{StagingUploader, UploadRing};
use crate::{AdapterInfo, RendererConfig};
use engine_core::math::Frustum;
use engine_core::time::{FrameStats, FrameStatsWindow};
use engine_render_data::Camera;
use engine_render_data::{
    Lighting, LocalLight, MaterialHandle, MeshHandle, ModelBinding, ModelLodBinding, ModelPart,
    PartGeometry, RenderWorld, SkinnedDrawList, SkinnedMeshHandle,
};

/// How many shader-visible SRVs the renderer's one descriptor heap holds.
///
/// One for the UI atlas, one per uploaded texture, and four per material — a material's
/// texture set is one contiguous four-SRV table, so it costs four descriptors whether or not
/// it uses four distinct maps.
///
/// Four thousand rather than the thousand this was before the material set grew from one
/// texture to four: at the old figure a scene of 200 materials over 200 textures spent 1000
/// descriptors exactly, and the next material was a named error. The heap is 4096 x 32 bytes
/// — 128 KB — so the headroom costs nothing worth measuring, and running out is still a
/// named error rather than corruption.
const MAX_TEXTURE_DESCRIPTORS: u32 = 4096;

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

/// Alignment for the UI vertex stream in the upload ring.
///
/// Sixteen, as for [`crate::render_list::INSTANCE_STREAM_ALIGNMENT`]: a vertex buffer view
/// has no 256-byte requirement, only a four-byte one, and the ring's allocator wants a power
/// of two. It used to be the stride, which stopped being a power of two when the vertex grew
/// its clip to 48 bytes.
const UI_VERTEX_ALIGNMENT: usize = 16;

// The ring rounds every allocation up to its alignment, so the alignment must divide cleanly
// into what the input assembler reads: whole floats.
const _: () = assert!(UI_VERTEX_ALIGNMENT.is_power_of_two() && size_of::<UiVertex>() % 4 == 0);

/// Alignment for the particle instance stream in the upload ring.
///
/// Sixteen, deliberately **not** the 36-byte record size: the ring's aligner requires a
/// power of two, and a vertex buffer needs an address the input assembler can index from
/// rather than one that is a multiple of the stride. This is the same trap
/// [`crate::render_list::INSTANCE_STREAM_ALIGNMENT`] documents, met a second time by a
/// record that is also not a power of two.
const PARTICLE_ALIGNMENT: usize = 16;

/// Most quads one frame of UI may emit.
///
/// 65,536 quads is 12 MB of vertices, well past anything the HUD produces — twenty
/// nameplates plus the full combat HUD is a few thousand. It exists so a runaway UI drops
/// work visibly rather than allocating without bound inside the frame path.
const MAX_UI_QUADS: usize = 65_536;

/// What an RGBA8 image of this size costs with a full mip chain.
///
/// The baseline the texture-memory accounting compares against: four bytes a texel for every
/// level. Level 0 is most of it — a chain adds a third — but the third is real and stating it
/// keeps the comparison honest, since the chains did not exist before this milestone either.
fn rgba_chain_bytes(width: u32, height: u32) -> usize {
    (0..crate::mipmap::level_count(width, height))
        .map(|level| {
            let (level_width, level_height) = crate::mipmap::level_size(width, height, level);
            level_width as usize * level_height as usize * 4
        })
        .sum()
}

/// A texture and the descriptor that addresses it.
///
/// One struct rather than two parallel arrays, because the two have exactly the same
/// lifetime: the descriptor points into the texture, so releasing either alone is the bug.
struct UploadedTexture {
    /// How this texture was compressed, if it was.
    ///
    /// Stored rather than derived from the format so the answer to "was this compressed, and
    /// as what" has one definition — the format is a `DXGI_FORMAT` and mapping it back would
    /// be a second table to keep in step with the first.
    compression: crate::mipmap::Compression,
    /// The resource every view of this texture points into. Read when a material writes it
    /// into its own table, and held for ownership besides: dropping it would leave a
    /// shader-visible SRV addressing released memory.
    texture: crate::resource::GpuTexture,
    /// The view format, which carries the colour space. Kept because a material's table
    /// holds its *own* SRV for each of its four maps rather than a copy of this one — the
    /// D3D12 copy path needs a non-shader-visible source heap, and creating the view again
    /// is cheaper than the staging heap that would avoid it.
    format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT,
    /// The standalone view, in the persistent region. Kept so a texture is addressable
    /// without a material, which is what the UI atlas and the diagnostics need.
    descriptor: crate::descriptor::DescriptorHandle,
}

/// One material's texture set and the contiguous descriptor range that binds it.
///
/// The range is [`MATERIAL_TEXTURE_COUNT`] slots taken from the persistent region in a
/// single allocation, because a root descriptor table has to be contiguous in the heap. The
/// four SRVs are written into it in [`MaterialTextureSlot::ALL`] order — that ordering is
/// the ABI, and it is applied in exactly one place so it cannot drift.
struct MaterialTextureBinding {
    /// Which texture each slot samples, by name. The renderer never indexes this
    /// positionally; only the write of the table does, through
    /// [`MaterialTextures::in_descriptor_order`].
    textures: MaterialTextures,
    /// The first descriptor of the range. Held so the range can be released with the
    /// material, and so a test can read back where the table starts.
    range: crate::descriptor::DescriptorHandle,
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
    /// The level's map picture, which `UiCommand::MapImage` samples. `None` until a level
    /// with a baked map is loaded.
    map: Option<UiMapPicture>,
    /// The descriptor slot the map's view is written into, allocated on first use and reused
    /// by every map after it, so re-baking a level in the editor costs no descriptor.
    map_descriptor: Option<crate::descriptor::DescriptorHandle>,
    /// The interface fonts' distance-field atlas. Held for its descriptor, like the atlas.
    #[allow(
        dead_code,
        reason = "keeps the texture the font descriptor points at alive"
    )]
    font: crate::resource::GpuTexture,
    /// Where the font atlas's view lives.
    font_descriptor: crate::descriptor::DescriptorHandle,
}

/// The map picture on the GPU.
struct UiMapPicture {
    /// Held, not read: the map descriptor views it.
    #[allow(
        dead_code,
        reason = "keeps the texture the map descriptor points at alive"
    )]
    texture: crate::resource::GpuTexture,
    /// Texels across and down.
    size: (u32, u32),
    /// Bytes of GPU memory, mips included.
    bytes: usize,
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
    /// The open upload batch, if one is. See [`Renderer::begin_upload_batch`].
    bulk: Option<StagingUploader>,
    /// What the camera fades out of its way this frame. See [`Renderer::set_camera_fade`].
    camera_fade: Option<engine_render_data::CameraFade>,
    /// Draws the translucent-character passes recorded last frame, both passes counted.
    translucent_character_draws_last_frame: u32,
    /// Device-local memory every mesh and material is suballocated out of, so a model costs
    /// a few offsets rather than a driver allocation per buffer.
    geometry: BufferArena,
    /// What construction cost, for the application's startup report. Absent without the
    /// `startup-timing` feature.
    #[cfg(feature = "startup-timing")]
    startup: RendererStartup,
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
    /// The same frame's time, split by pass. Indexed by the `*_PASS` constants; `None` for a
    /// pass whose timestamps have not been resolved yet, which is every pass for the first
    /// few frames of a run.
    last_pass_ms: [Option<f64>; MAX_TIMED_PASSES],
    /// The mesh pass's pipeline statistics, when asked for. `None` costs nothing.
    pipeline_statistics: Option<crate::pipeline_stats::PipelineStatistics>,
    /// The most recent resolved statistics, read with the same lag as the timestamps.
    last_scene_statistics: Option<crate::pipeline_stats::SceneStatistics>,
    /// Root signature and pipeline state, independent of the geometry.
    pipeline: ScenePipeline,
    /// Depth target, the size of the back buffer.
    depth: DepthBuffer,
    /// Every mesh pipeline state in use, keyed by shader permutation and fill mode.
    ///
    /// One cache rather than six fields, because the pixel shader is now chosen per draw
    /// from what the material declares and per frame from whether shadows are on: the set of
    /// states a frame needs is a property of the scene, not something startup can enumerate.
    /// See [`MeshPipelines`].
    pipelines: MeshPipelines,
    /// Whether `render_world` draws through the wireframe pipeline.
    wireframe: bool,
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
    /// Which uploaded texture holds a given image, keyed by content.
    ///
    /// The deduplication table. An environment library shares one bark image across every
    /// tree and a glTF file hands the same packed ORM image to every material that uses it;
    /// before this existed each of those became its own resource, its own upload and its own
    /// descriptor. Keyed by the texels rather than by a path or an index, because the same
    /// image arrives from two `.glb` files under two different indices and from a procedural
    /// author with no path at all — see [`crate::texture_id`].
    ///
    /// Sharing needs no reference counting: `textures` is append-only and lives as long as
    /// the renderer, so a handle two materials hold cannot be released out from under one of
    /// them. When textures become releasable this table gains a count, and that is the
    /// moment to add one — not before.
    textures_by_content: HashMap<TextureKey, TextureHandle>,
    /// Bytes of GPU memory the resident textures occupy, mip chains and compression included.
    texture_bytes: usize,
    /// What those same images would have cost as uncompressed RGBA8 with no deduplication:
    /// one full chain per *request*.
    ///
    /// Against `texture_bytes`, what Milestone 4 saved. Counted rather than estimated because
    /// both halves of the saving — sharing an image and compressing it — are decisions made
    /// per texture, and a formula would drift from them.
    texture_bytes_naive: usize,
    /// How many times a texture was asked for, including the ones the table answered.
    ///
    /// Against `textures.len()`, this is the deduplication ratio, and it is the number the
    /// benchmark reports.
    texture_requests: usize,
    /// The shader-visible CBV/SRV/UAV heap every SRV lives in — material maps and the UI
    /// atlas alike. Bound once per frame in `begin_recording`.
    descriptors: GpuDescriptorHeap,
    /// The four shared 1×1 fallback maps, one per [`MaterialTextureSlot`].
    ///
    /// **Shared, not per material.** A material that provides no normal map points at the
    /// same flat-normal texel every other such material points at; there is one texture per
    /// slot in the whole renderer. Each is neutral for its slot, so binding a fallback is
    /// arithmetically the same as having no map at all:
    ///
    /// ```text
    /// base colour  (1, 1, 1, 1)      sRGB   multiplicative identity
    /// normal       (0.5, 0.5, 1, 1)  linear the flat tangent-space normal, +Z
    /// ORM          (1, 1, 1, 1)      linear glTF's identity: AO 1, and the roughness and
    ///                                       metallic factors pass through unscaled
    /// emissive     (0, 0, 0, 1)      sRGB   emits nothing
    /// ```
    fallback_textures: MaterialTextures,
    /// Materials created through [`Renderer::create_material`], indexed by handle.
    materials: Vec<Material>,
    /// The shadow stage: the atlas, what goes in it, and the passes that fill it.
    ///
    /// Declared before `context` like every other GPU-owning field, because it holds a
    /// texture and pipeline states that must be released before the device is.
    shadow: ShadowRenderer,
    /// The base of each material's four-descriptor table, indexed by material handle.
    ///
    /// The *bind-time* view of [`Renderer::material_bindings`], flattened to one GPU handle
    /// per material because that is the only thing recording needs and because
    /// [`crate::shadow::renderer::ShadowScene`] takes it as a slice — the shadow pass cannot
    /// call a `&self` method while it holds a `&mut` borrow of the renderer's shadow state.
    /// It is deliberately not four parallel arrays: what a descriptor *means* lives in
    /// Which shader permutation each material asks for, indexed by handle.
    ///
    /// Parallel to `materials`, and settled at creation: `pbr` is what the description
    /// declares and `normal_map` is whether the caller supplied a normal map rather than
    /// letting the slot resolve to the flat fallback. The shadow bit is not stored here -
    /// it is global and per frame.
    material_features: Vec<ShaderFeatures>,
    /// Each material's parameters, as it was created, so an identical request is answered
    /// with the material that already exists. See [`Renderer::create_material_with_textures`].
    material_descs: Vec<MaterialDesc>,
    /// Meshes replaced by [`Renderer::replace_mesh`], kept alive until the GPU has passed the
    /// last frame that could have drawn them.
    retired_meshes: Vec<(u64, GpuMesh)>,
    /// The CPU side of each dynamic mesh, keyed by its index in `meshes`. See
    /// [`Renderer::create_dynamic_mesh`].
    dynamic_meshes: HashMap<usize, crate::dynamic_mesh::DynamicMesh>,
    /// Which phase each material is drawn in, indexed by material handle.
    ///
    /// A parallel array beside `material_features` rather than a field read back out of the
    /// `Material` - the split runs over the visible set every frame and wants a dense array of
    /// one-byte classes, not a pointer chase into a type that owns a GPU buffer.
    material_surfaces: Vec<crate::material::SurfaceClass>,
    /// Objects the last frame's transparent phase drew.
    transparent_draws_last_frame: u32,
    /// [`MaterialTextures`], and this array carries only where the table starts.
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
    /// This frame's bone palettes, each uploaded once and bound by every pass that draws the
    /// character: see `crate::palette`.
    frame_palettes: crate::palette::FramePalettes,
    /// This frame's skinned draws, *unculled*, for the shadow passes.
    ///
    /// Separate from [`Renderer::skinned`], which is the camera's filtered copy: a character
    /// behind the camera casts into a cascade, so the shadow stage needs the whole list.
    shadow_skinned: SkinnedDrawList,
    /// Each material's texture set and the descriptor range that binds it, indexed by
    /// material handle.
    ///
    /// Separate from [`Renderer::materials`] rather than a field on `Material`: a material's
    /// constants are a device buffer it owns, and its descriptors are heap slots the renderer
    /// owns — and the offscreen paths build a `Material` with no heap at all. What matters is
    /// that the *four maps* are one struct with named slots rather than four arrays whose
    /// ordering has to stay synchronized.
    material_bindings: Vec<MaterialTextureBinding>,
    /// Frustum culling: the visibility policy and what the last pass counted.
    culler: Culler,
    /// This frame's render list — what culling selected, in the compact form recording
    /// reads. Kept across frames so it stops allocating once the scene reaches its
    /// high-water mark.
    visible: RenderList,
    /// This frame's transparent draws, sorted farthest-first.
    ///
    /// Split out of `visible` after the cull and before the grouping, so the opaque phase
    /// never sees a blended surface and its batching is exactly what it always was. Owned
    /// across frames for the same reason `visible` is: after the scene reaches its
    /// high-water mark, building it allocates nothing.
    transparent: RenderList,
    /// This frame's opaque draws the camera may dither: faded entities, and see-through ones
    /// the frame's camera fade reaches.
    ///
    /// Split out of `visible` so the depth prepass never draws them. The prepass writes whole
    /// surfaces, and a surface the camera pass then dithers would leave its depth behind in
    /// the holes — hiding exactly what the dither is there to show. Drawn after the opaque
    /// batches with ordinary depth writes; usually empty, or a handful of props in front of
    /// the camera.
    dithered: RenderList,
    /// Whether the camera pass may draw prepass-covered surfaces without writing depth. On by
    /// default; off is the A/B control (`--shadow-tweak prime=off`). See `DepthMode::Primed`.
    depth_priming: bool,
    /// Whether this frame's depth prepass ran and covers `visible` and the whole characters,
    /// so their camera draws can be [`DepthMode::Primed`]. Set by the shadow stage.
    depth_primed: bool,
    /// This frame's instance stream for `visible`, uploaded once before the shadow stage and
    /// bound by both the depth prepass and the camera pass. `None` before it is uploaded, and
    /// for an empty list.
    camera_instances: Option<windows::Win32::Graphics::Direct3D12::D3D12_VERTEX_BUFFER_VIEW>,
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
    /// Where the frame being recorded is copied to, when it is a [`Renderer::capture_world`]
    /// frame rather than one for the screen.
    capture: Option<crate::offscreen::Readback>,
    /// Where the next ordinary frame — interface and all — is copied to, when a screenshot
    /// was asked for. See [`Renderer::request_screenshot`].
    screenshot: Option<crate::offscreen::Readback>,
    /// Back-buffer index the recorded frame targeted. Tracked separately from the frame
    /// slot: one is a texture, the other is CPU-side recording state.
    recorded_back_buffer: usize,
    /// Device, queue and factory. Declared last so it is released last.
    context: RenderDevice,
}

/// What [`Renderer::new`] spent its time on, in milliseconds.
///
/// Zero unless the `startup-timing` feature is on, which is how a shipping build carries no
/// measurement: without the feature nothing writes these and nothing reads them.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RendererStartup {
    /// Creating the D3D12 device, its queue and the swap chain.
    pub device_ms: f64,
    /// Everything after that: pipelines, heaps, the shadow atlas, the UI atlas, buffers.
    pub objects_ms: f64,
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

        // Startup timing: the device and the swap chain are the D3D12 half of "the renderer
        // started", and everything after them is pipelines, heaps and buffers. Behind a
        // feature, so a shipping build does not even take the clock — see `Renderer::startup`.
        #[cfg(feature = "startup-timing")]
        let device_started = std::time::Instant::now();
        let context =
            RenderDevice::new(config.enable_debug_layer, config.break_on_validation_error)?;
        let hwnd = HWND(window_handle as *mut std::ffi::c_void);
        let swap_chain = SwapChain::new(&context, hwnd, config.width, config.height, config.vsync)?;
        #[cfg(feature = "startup-timing")]
        let device_ms = device_started.elapsed().as_secs_f64() * 1e3;

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
        let mut scene_scratch = Vec::new();
        stress::build_grid(1, &mut scene_scratch);
        let scene = VertexStream::upload(&context, &scene_scratch)?;

        // One heap for every shader-visible SRV in the renderer: the UI atlas and one per
        // material texture. Bound once per frame; never switched.
        let mut descriptors =
            GpuDescriptorHeap::new(&context, MAX_TEXTURE_DESCRIPTORS, 0, frames_in_flight)?;
        // Before the struct literal, because it allocates two persistent descriptors out of
        // the heap above and the heap is moved into the literal.
        let mut shadow = ShadowRenderer::new(
            &context,
            &mut descriptors,
            ShadowSettings::default(),
            (config.width, config.height),
        )?;
        // The camera pass is drawn primed by default, so the prepass runs at every preset, not
        // only where contact shadows need it: the floors and the sky alone are more pixels than
        // the screen, and without a primed depth they are shaded wherever they are hidden.
        shadow.set_priming_prepass(true);
        // The contact-shadow march reads the depth buffer itself, not a copy of it.
        shadow.sample_camera_depth(&context, depth.texture())?;
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
            bulk: None,
            camera_fade: None,
            translucent_character_draws_last_frame: 0,
            geometry: BufferArena::new(
                windows::Win32::Graphics::Direct3D12::D3D12_RESOURCE_STATE_COMMON,
            ),
            #[cfg(feature = "startup-timing")]
            startup: RendererStartup {
                device_ms,
                objects_ms: device_started.elapsed().as_secs_f64() * 1e3 - device_ms,
            },
            swap_chain,
            frames,
            commands,
            command_stats: CommandStats::default(),
            execute_lists,
            gpu_timer,
            last_gpu_frame_ms: None,
            last_pass_ms: [None; MAX_TIMED_PASSES],
            pipeline_statistics: None,
            last_scene_statistics: None,
            pipeline,
            depth,
            pipelines: MeshPipelines::new(),
            wireframe: false,
            meshes: Vec::new(),
            skinned_meshes: Vec::new(),
            textures: Vec::new(),
            textures_by_content: HashMap::new(),
            texture_bytes: 0,
            texture_bytes_naive: 0,
            texture_requests: 0,
            descriptors,
            // Replaced immediately below, once there is a `self` to upload through. The
            // placeholder names slot 0, which is exactly what the white texture becomes.
            // Replaced immediately below, once there is a `self` to upload through. Every
            // slot names texture 0, which the constructor then creates as the white texel.
            fallback_textures: MaterialTextures::uniform(TextureHandle(0)),
            materials: Vec::new(),
            material_bindings: Vec::new(),
            material_features: Vec::new(),
            material_descs: Vec::new(),
            retired_meshes: Vec::new(),
            dynamic_meshes: HashMap::new(),
            material_surfaces: Vec::new(),
            transparent_draws_last_frame: 0,
            material_tables: Vec::new(),
            shadow,
            lights: Vec::new(),
            clusters: LightClusters::new(),
            light_clustering: true,
            local_lights_enabled: true,
            frame_palettes: crate::palette::FramePalettes::new(),
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
            capture: None,
            screenshot: None,
            recorded_back_buffer: 0,
            context,
        };
        let mut renderer = renderer;
        // The first texture, so an untextured material has something to sample. Created here
        // rather than in the struct literal because uploading needs a `self`.
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
    ///
    /// Read once, by the application's startup report. Two `f64`s rather than a profiler:
    /// "why does a launch take a second" is answered by a handful of numbers, and this crate
    /// has no business growing a measurement system to produce them.
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

    /// Video memory this process is using. A kernel call: for a report, not every frame.
    #[must_use]
    pub fn video_memory(&self) -> Option<crate::device::VideoMemory> {
        self.context.video_memory()
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

    /// The same frame's GPU time, split by pass.
    ///
    /// Indexed by [`crate::gpu_timer::SHADOW_PASS`] and its neighbours; the names are in
    /// [`crate::gpu_timer::TIMED_PASSES`]. The parts do not have to sum to
    /// [`Renderer::last_gpu_frame_ms`]: the frame's own pair also covers the clears, the
    /// barriers and whatever the driver does between passes, and that difference is itself
    /// worth reading — it is the frame's fixed overhead.
    #[must_use]
    pub fn last_pass_ms(&self) -> [Option<f64>; MAX_TIMED_PASSES] {
        self.last_pass_ms
    }

    /// Whether the camera pass may draw what the depth prepass covered without writing depth.
    ///
    /// On by default; it only has an effect where the shadow stage runs a prepass (contact
    /// shadows on). Off draws exactly as before priming existed, which is the A/B control.
    pub fn set_depth_priming(&mut self, enabled: bool) {
        self.depth_priming = enabled;
    }

    /// Whether the camera depth prepass also runs where contact shadows do not need it, purely
    /// so the camera pass can be drawn primed: at Low and Medium, and with shadows off.
    pub fn set_priming_prepass(&mut self, enabled: bool) {
        self.shadow.set_priming_prepass(enabled);
    }

    /// Turns the shadow views' static-caster cache on or off, with the cascade margin as a
    /// fraction of each cascade's radius. See `shadow::static_cache`.
    pub fn set_static_shadow_cache(&mut self, enabled: bool, margin_fraction: f32) {
        self.shadow.set_static_cache(enabled, margin_fraction);
    }

    /// Whether the last cascade draws movers and characters: a measured trade-off, on by
    /// default. See `ShadowRenderer::set_far_movers`.
    pub fn set_far_shadow_movers(&mut self, enabled: bool) {
        self.shadow.set_far_movers(enabled);
    }

    /// Turns the mesh pass's pipeline-statistics query on or off.
    ///
    /// A measurement control (`--pipeline-stats`): off by default, because a statistics
    /// query is not free on every driver and a timing comparison must not carry one.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if the query heap cannot be created.
    pub fn set_pipeline_statistics(&mut self, enabled: bool) -> Result<(), RendererError> {
        if !enabled {
            self.pipeline_statistics = None;
            self.last_scene_statistics = None;
            return Ok(());
        }
        if self.pipeline_statistics.is_none() {
            self.pipeline_statistics = Some(crate::pipeline_stats::PipelineStatistics::new(
                self.context.device(),
                self.gpu_timer.frame_count(),
            )?);
        }
        Ok(())
    }

    /// The most recent mesh pass's pipeline statistics, or `None` when the query is off or
    /// has not completed a lap. Lags the frame exactly as [`Renderer::last_pass_ms`] does.
    #[must_use]
    pub fn last_scene_statistics(&self) -> Option<crate::pipeline_stats::SceneStatistics> {
        self.last_scene_statistics
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
        // Before the grouping, deliberately. The counting sort's key is `(material, mesh)` and
        // it must not see a transparent entry: the opaque phase's batches have to contain
        // exactly what they contained before this feature, and a transparent object left in
        // them would be drawn with depth writes on, in material order.
        //
        // A scene with no transparent material pays one predicate pass and nothing else.
        self.visible
            .split_transparent(&self.material_surfaces, &mut self.transparent);
        // Whatever the camera may dither this frame leaves the list the depth prepass draws;
        // see `dithered`. Decided per entity from the same fade the shader applies, so a
        // see-through prop nowhere near the eye or the line to the character stays in.
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
        // Sorted from the camera the frame is drawn with, not from the cull camera: the cull
        // view may be frozen for debugging (`cull_view_projection`), and freezing what is
        // culled must not reorder what is drawn.
        self.transparent.sort_back_to_front(camera.position());
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

        self.shadow.prepare(
            world,
            camera,
            lighting,
            self.materials.len(),
            self.meshes.len(),
        );
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
        frame_constants: u64,
    ) -> Result<(), RendererError> {
        let scene = ShadowScene {
            meshes: &self.meshes,
            skinned_meshes: &self.skinned_meshes,
            materials: &self.materials,
            material_tables: &self.material_tables,
            skinned: &self.shadow_skinned,
            palettes: &self.frame_palettes,
        };
        // Every one of these is a *different field* of `self`, which is what lets the shadow
        // renderer be borrowed mutably while the meshes and materials are borrowed shared.
        self.gpu_timer
            .begin_pass(self.commands.list(), slot, SHADOW_ATLAS_PASS);
        self.shadow
            .record(&mut self.commands, &mut self.uploads, slot, &scene)?;
        self.gpu_timer
            .end_pass(self.commands.list(), slot, SHADOW_ATLAS_PASS);

        // Both sub-passes are stamped whether or not they run, so a frame without contact
        // shadows reads zero for them rather than a stale number from the last frame that had.
        self.gpu_timer
            .begin_pass(self.commands.list(), slot, DEPTH_PREPASS);
        let prepass = self.shadow.needs_prepass();
        if prepass {
            let (width, height) = self.swap_chain.size();
            let dsv = self.depth.view().cpu;
            // The camera's own lists, rigid and skinned: the prepass draws exactly what the
            // camera pass will draw primed, and nothing that pass will not.
            let scene = ShadowScene {
                meshes: &self.meshes,
                skinned_meshes: &self.skinned_meshes,
                materials: &self.materials,
                material_tables: &self.material_tables,
                skinned: &self.skinned,
                palettes: &self.frame_palettes,
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
                self.camera_instances,
            )?;
            self.shadow
                .copy_scene_depth(&mut self.commands, self.depth.texture());
            self.depth_primed = true;
        }
        self.gpu_timer
            .end_pass(self.commands.list(), slot, DEPTH_PREPASS);
        self.gpu_timer
            .begin_pass(self.commands.list(), slot, CONTACT_PASS);
        if prepass && self.shadow.needs_depth_copy() {
            // The contact-shadow march, into its own half-resolution target. It reads the
            // depth copy made on the line above and the frame constants, so it goes here and
            // not with the atlas: the depth it marches through does not exist until now.
            self.commands.set_root_cbv(ROOT_FRAME_CBV, frame_constants);
            if let Some(table) = self.shadow.depth_table() {
                self.commands
                    .set_root_descriptor_table(ROOT_DEPTH_TABLE, table);
            }
            self.shadow.record_contact(&mut self.commands);
            self.shadow
                .finish_scene_depth(&mut self.commands, self.depth.texture());
        }
        self.gpu_timer
            .end_pass(self.commands.list(), slot, CONTACT_PASS);

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
        // Until the shadow stage has actually recorded this frame's prepass, nothing may be
        // drawn primed.
        self.depth_primed = false;
        // The camera's instances, once, for the prepass and the camera pass both: the two draw
        // the same list, and composing it twice was the prepass's larger half.
        self.camera_instances =
            self.visible
                .upload_instances(&mut self.uploads, slot, &mut self.instance_scratch)?;

        // Every character's palette, once, before any pass binds one: the shadow views, the
        // prepass and the camera pass then share these addresses rather than each uploading
        // its own copy. The shadow list is the unculled superset and only matters when there
        // are shadows; the camera's list is what it draws.
        self.frame_palettes.clear();
        if self.shadow.settings().enabled {
            self.frame_palettes
                .upload_all(&self.shadow_skinned, &mut self.uploads, slot)?;
        }
        self.frame_palettes
            .upload_all(&self.skinned, &mut self.uploads, slot)?;

        // The frame constants are written once into this slot's upload memory and bound
        // twice: once for the contact-shadow pass inside the shadow stage, once for the mesh
        // pass. Built *before* the stage for that reason — the pass reads the same camera,
        // the same light and the same contact settings the lighting does, and a second copy
        // of them is a second thing to keep in step.
        let mut frame_constants =
            FrameConstants::new(camera.view_projection(), camera.position(), lighting);
        frame_constants.set_camera(camera);
        frame_constants.set_camera_fade(self.camera_fade);
        let (viewport_width, viewport_height) = self.swap_chain.size();
        frame_constants.set_viewport(viewport_width, viewport_height);
        self.shadow
            .write_frame_constants(&mut frame_constants, &self.lights);
        frame_constants.set_light_clusters(&self.clusters);
        frame_constants.set_contact_target(self.shadow.contact_target_size());
        // The particle pass is recorded later, from a function with no camera. Its basis is
        // captured here, from the same camera the scene is drawn with, so a particle cannot
        // be billboarded against a different frame's view than the one it is drawn into.
        self.particles.constants =
            ParticleConstants::new(&camera.view(), &camera.view_projection());
        let allocation = self.uploads.write_constants(slot, &frame_constants)?;

        // Before the camera pass, and before its pipeline is bound: the shadow stage binds
        // its own depth target, its own viewports and its own pipeline states, and rebinds
        // the camera's when it is done.
        self.gpu_timer
            .begin_pass(self.commands.list(), slot, SHADOW_PASS_QUERY);
        self.record_shadow_stage(slot, back_buffer, camera, allocation.gpu_address())?;
        self.gpu_timer
            .end_pass(self.commands.list(), slot, SHADOW_PASS_QUERY);

        // Everything from here to `finish_recording` is the camera's mesh pass: the binds
        // above, every batch below, and the skinned draws after them. Closed in
        // `finish_recording`, which both the instanced and the non-instanced path reach.
        self.gpu_timer
            .begin_pass(self.commands.list(), slot, SCENE_PASS);
        self.statistics_section(slot, crate::pipeline_stats::OPAQUE_SECTION);

        // The root signature first, before a single root argument is written.
        //
        // Every mesh permutation shares one signature object, so this is the only time it is
        // bound in the camera pass and the per-batch pipeline switches below cannot discard
        // what the next few lines write. Binding it *after* the frame constants - which is
        // what happens if a batch is the first thing to bind a signature - throws them away
        // and the first draw reads unbound root parameters, which the GPU reports as a
        // removed device rather than as a validation error.
        let mesh_root_signature = self
            .pipelines
            .root_signature(self.context.device())?
            .clone();
        self.commands.set_root_signature(&mesh_root_signature);

        // The pipeline state is bound per batch below, from the material's permutation.
        // The topology, the frame constants and the cluster grid are shared by all of them.
        self.commands
            .set_topology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        self.commands
            .set_root_cbv(ROOT_FRAME_CBV, allocation.gpu_address());
        // The cluster grid rides in the same upload ring as the constants, in its own
        // allocation: 13.5 KB of masks, bound as a root SRV and read by the lighting.
        let clusters = self.uploads.write(
            slot,
            self.clusters.masks(),
            crate::upload::DEFAULT_ALIGNMENT,
        )?;
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
        //
        // Always with depth writes: the per-object path has its own vertex shader, which is
        // not the prepass's, so its depth is not promised to equal what the prepass wrote.
        for which in [CameraList::Visible, CameraList::Dithered] {
            let list = match which {
                CameraList::Visible => &self.visible,
                CameraList::Dithered => &self.dithered,
            };
            for batch in list.batches() {
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

                let variant = self.material_variant(batch.material);
                let pipeline_state = self
                    .pipelines
                    .mesh(self.context.device(), variant)?
                    .pipeline_state()
                    .clone();
                // State only: the signature was bound once above. The command context drops a
                // redundant bind, so a run of batches sharing a permutation - which is nearly
                // every scene - costs one PSO change, not one per batch.
                self.commands.set_pipeline_state(&pipeline_state);

                self.commands
                    .set_root_cbv(ROOT_MATERIAL_CBV, material.gpu_address());
                self.commands
                    .set_root_descriptor_table(ROOT_MATERIAL_TABLE, table);
                self.commands.set_vertex_buffer(0, *mesh.vertex_view());
                self.commands.set_index_buffer(*mesh.index_view());

                let first = batch.first as usize;
                let last = first + batch.count as usize;
                for &entry in &list.order()[first..last] {
                    let object = ObjectConstants::shadowed(
                        list.transforms()[entry as usize].matrix(),
                        list.styles()[entry as usize],
                        list.receives_shadow()[entry as usize],
                    );
                    let allocation = self.uploads.write_constants(slot, &object)?;
                    self.commands
                        .set_root_cbv(ROOT_OBJECT_CBV, allocation.gpu_address());
                    self.commands.draw_indexed(mesh.index_count(), 1, 0);
                }
            }
        }

        self.statistics_section(slot, crate::pipeline_stats::CHARACTER_SECTION);
        self.draw_skinned(slot)?;
        self.statistics_section(slot, crate::pipeline_stats::BLENDED_SECTION);
        self.draw_transparent(slot)?;
        self.draw_translucent_characters(slot)?;
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
        // Everything the prepass drew tests against its depth and writes none; what it did
        // not draw - the entries the camera may dither - makes its own depth afterwards.
        let primed = self.primed_depth_mode();
        self.draw_opaque_instanced(slot, CameraList::Visible, primed)?;
        self.draw_opaque_instanced(slot, CameraList::Dithered, DepthMode::Write)?;

        self.statistics_section(slot, crate::pipeline_stats::CHARACTER_SECTION);
        self.draw_skinned(slot)?;
        self.statistics_section(slot, crate::pipeline_stats::BLENDED_SECTION);
        self.draw_transparent(slot)?;
        self.draw_translucent_characters(slot)?;
        self.finish_recording(slot, back_buffer)
    }

    /// Opens the mesh pass's next statistics section, when `--pipeline-stats` asked for them.
    fn statistics_section(&mut self, slot: usize, section: usize) {
        if let Some(statistics) = self.pipeline_statistics.as_mut() {
            statistics.begin(self.commands.list(), slot, section);
        }
    }

    /// The depth mode for a surface this frame's prepass covered: [`DepthMode::Primed`] when
    /// the prepass ran and priming is on, [`DepthMode::Write`] otherwise.
    fn primed_depth_mode(&self) -> DepthMode {
        if self.depth_primed && self.depth_priming && !self.wireframe {
            DepthMode::Primed
        } else {
            DepthMode::Write
        }
    }

    /// Records one of the camera's opaque lists as instanced batches, drawn with `depth`.
    fn draw_opaque_instanced(
        &mut self,
        slot: usize,
        which: CameraList,
        depth: DepthMode,
    ) -> Result<(), RendererError> {
        // One stream for the whole list, in batch order, bound once: the visible list's was
        // uploaded before the shadow stage, which the prepass has already drawn from.
        let stream = match which {
            CameraList::Visible => self.camera_instances,
            CameraList::Dithered => self.dithered.upload_instances(
                &mut self.uploads,
                slot,
                &mut self.instance_scratch,
            )?,
        };
        let Some(stream) = stream else {
            return Ok(());
        };
        self.commands.set_vertex_buffer(1, stream);
        let list = match which {
            CameraList::Visible => &self.visible,
            CameraList::Dithered => &self.dithered,
        };
        for batch in list.batches() {
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

            let variant = self.material_variant(batch.material).with_depth(depth);
            let pipeline_state = self
                .pipelines
                .instanced(self.context.device(), variant)?
                .pipeline_state()
                .clone();
            self.commands.set_pipeline_state(&pipeline_state);

            self.commands
                .set_root_cbv(ROOT_MATERIAL_CBV, material.gpu_address());
            self.commands
                .set_root_descriptor_table(ROOT_MATERIAL_TABLE, table);
            self.commands.set_vertex_buffer(0, *mesh.vertex_view());
            self.commands.set_index_buffer(*mesh.index_view());
            self.commands
                .draw_indexed(mesh.index_count(), batch.count, batch.first);
        }
        Ok(())
    }

    /// Records this frame's transparent draws, farthest first.
    ///
    /// ```text
    /// shadow stage -> opaque batches -> skinned -> TRANSPARENT -> particles -> overlay -> UI
    ///                                              ^^^^^^^^^^^ this function
    /// ```
    ///
    /// Last of the mesh phases and before the particle pass, which is the only position that
    /// works: it needs the opaque depth buffer finished to test against, and it must not be
    /// separated from the particles it shares a screen with by anything that writes colour.
    ///
    /// # One draw per object, and no batching
    ///
    /// The opaque phase groups by `(material, mesh)` so it can bind once per run; this one
    /// cannot, because its order is fixed by distance and grouping would reorder it. Two
    /// entries that happen to be adjacent *and* share a mesh and material still cost only one
    /// set of binds - [`crate::command::CommandContext`] drops a redundant bind - so the run
    /// that a sort happens to produce is exploited without the order ever being changed to
    /// produce one.
    ///
    /// Instancing is not used here for the same reason, even when the frame is otherwise
    /// instanced: an instanced draw is one draw for many objects and gives the GPU no ordering
    /// guarantee between them. The phase is small by construction - effects, not scenery.
    ///
    /// Returns how many objects were drawn.
    fn draw_transparent(&mut self, slot: usize) -> Result<u32, RendererError> {
        // Zeroed before the early return, not after it: a frame with nothing transparent must
        // report zero rather than whatever the last frame that had some reported.
        self.transparent_draws_last_frame = 0;
        if self.transparent.is_empty() {
            return Ok(0);
        }

        // The skinned pass may have left a different topology bound; the opaque batches set it
        // once at the top of the frame and this phase cannot assume it survived.
        self.commands
            .set_topology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);

        let mut drawn = 0;
        for position in 0..self.transparent.order().len() {
            let entry = self.transparent.order()[position] as usize;
            let mesh_handle = self.transparent.meshes()[entry];
            let material_handle = self.transparent.materials()[entry];

            let Some(mesh) = self.meshes.get(mesh_handle.index()) else {
                return Err(RendererError::InvalidConfig {
                    reason: format!(
                        "the transparent list names mesh {} which does not exist",
                        mesh_handle.index()
                    ),
                });
            };
            let index_count = mesh.index_count();
            let vertex_view = *mesh.vertex_view();
            let index_view = *mesh.index_view();

            let Some(material) = self.materials.get(material_handle.index()) else {
                return Err(RendererError::InvalidConfig {
                    reason: format!(
                        "the transparent list names material {} which does not exist",
                        material_handle.index()
                    ),
                });
            };
            let material_address = material.gpu_address();

            let Some(table) = self.material_table(material_handle) else {
                return Err(RendererError::InvalidConfig {
                    reason: format!(
                        "material {} has no shader-visible base colour map",
                        material_handle.index()
                    ),
                });
            };

            // The same permutation the material would get in the opaque phase, with its class
            // folded in - so a transparent surface is shaded by the identical pixel shader and
            // differs only in the blend and depth state the PSO carries.
            let variant = self.material_variant(material_handle);
            let pipeline_state = self
                .pipelines
                .mesh(self.context.device(), variant)?
                .pipeline_state()
                .clone();
            self.commands.set_pipeline_state(&pipeline_state);

            self.commands
                .set_root_cbv(ROOT_MATERIAL_CBV, material_address);
            self.commands
                .set_root_descriptor_table(ROOT_MATERIAL_TABLE, table);
            self.commands.set_vertex_buffer(0, vertex_view);
            self.commands.set_index_buffer(index_view);

            let object = ObjectConstants::shadowed(
                self.transparent.transforms()[entry].matrix(),
                self.transparent.styles()[entry],
                self.transparent.receives_shadow()[entry],
            );
            let allocation = self.uploads.write_constants(slot, &object)?;
            self.commands
                .set_root_cbv(ROOT_OBJECT_CBV, allocation.gpu_address());
            self.commands.draw_indexed(index_count, 1, 0);
            drawn += 1;
        }

        self.transparent_draws_last_frame = drawn;
        Ok(drawn)
    }

    /// Objects drawn by the most recent frame's transparent phase.
    ///
    /// Reported separately from the opaque draw count so the cost of blended geometry is
    /// attributable rather than folded into the scene's.
    #[must_use]
    pub fn transparent_draws_last_frame(&self) -> u32 {
        self.transparent_draws_last_frame
    }

    /// This frame's transparent draws, in the order they will be recorded.
    ///
    /// For tests and diagnostics: it is how "the list really is sorted farthest-first" can be
    /// asserted without a GPU, on the same list the recording walks rather than on a copy.
    #[must_use]
    pub fn transparent_list(&self) -> &RenderList {
        &self.transparent
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

        self.commands
            .set_topology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);

        let mut drawn = 0;
        // A whole character is in the prepass, so it tests against its own depth there.
        let depth = self.primed_depth_mode();
        // Consecutive submeshes of one character name the same palette range, so the upload
        // is skipped for all but the first of them.
        let mut bound_palette: Option<(u32, u32)> = None;

        for index in 0..self.skinned.len() {
            let Some(instance) = self.skinned.instances().get(index).copied() else {
                break;
            };
            // A character the camera is looking through is drawn later, over everything
            // else: see `draw_translucent_characters`.
            if is_translucent(&instance) {
                continue;
            }
            self.record_skinned_instance(
                slot,
                index,
                instance,
                CharacterPass::Lit,
                depth,
                instance.style,
                &mut bound_palette,
            )?;
            drawn += 1;
        }

        // Draw and triangle counts are not accumulated here: `CommandContext` already counts
        // every `draw_indexed` recorded through it, and a second tally would either
        // double-count or drift.
        self.skinned_draws_last_frame = drawn;
        Ok(drawn)
    }

    /// Records the characters the camera is looking through: each one twice, over the
    /// finished world.
    ///
    /// ```text
    /// shadow -> opaque -> skinned -> transparent -> TRANSLUCENT CHARACTERS -> particles
    /// ```
    ///
    /// After the transparent phase, so a spell effect behind the character is seen through
    /// it. First every one of them writes its depth and no colour ([`CharacterPass::DepthPrime`]),
    /// then every one of them blends at its opacity, tested against that depth and writing
    /// none ([`CharacterPass::Translucent`]) — so each pixel blends only the character's
    /// nearest surface over what is behind it. A character at opacity zero is not drawn at
    /// all. Its shadow is unaffected: the shadow stage draws it whole.
    ///
    /// The prime pass has no pixel shader (unless the material is alpha-tested and needs its
    /// cutout), so it costs a depth-only rasterization, and the blend pass shades only the
    /// nearest layer: a see-through character costs about what a solid one does.
    ///
    /// The cost to know about: a particle *behind* a translucent character is hidden by the
    /// primed depth, because particles come after. A few frames of a spark behind a
    /// see-through body is the trade for a body that does not show its own insides.
    fn draw_translucent_characters(&mut self, slot: usize) -> Result<u32, RendererError> {
        let any = self
            .skinned
            .instances()
            .iter()
            .any(|instance| is_translucent(instance) && instance.style.opacity > 0.0);
        if !any {
            return Ok(0);
        }
        self.commands
            .set_topology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        let mut drawn = 0;
        for pass in [CharacterPass::DepthPrime, CharacterPass::Translucent] {
            let mut bound_palette: Option<(u32, u32)> = None;
            for index in 0..self.skinned.len() {
                let Some(instance) = self.skinned.instances().get(index).copied() else {
                    break;
                };
                if !is_translucent(&instance) || instance.style.opacity <= 0.0 {
                    continue;
                }
                // The opacity becomes the tint's alpha, which is what the blend reads; the
                // dither is off, because blending is doing the fading.
                let mut style = instance.style;
                style.tint[3] *= style.opacity;
                style.opacity = 1.0;
                self.record_skinned_instance(
                    slot,
                    index,
                    instance,
                    pass,
                    DepthMode::Write,
                    style,
                    &mut bound_palette,
                )?;
                drawn += 1;
            }
        }
        self.translucent_character_draws_last_frame = drawn;
        Ok(drawn)
    }

    /// Records one skinned submesh through `pass`, drawn with `style`.
    #[allow(
        clippy::too_many_arguments,
        reason = "one submesh's draw: which, through which pass and depth mode, styled how, with
                  the palette the run already bound"
    )]
    fn record_skinned_instance(
        &mut self,
        slot: usize,
        index: usize,
        instance: engine_render_data::SkinnedInstance,
        pass: CharacterPass,
        depth: DepthMode,
        style: engine_render_data::InstanceStyle,
        bound_palette: &mut Option<(u32, u32)>,
    ) -> Result<(), RendererError> {
        {
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
            let variant = self
                .material_variant(instance.material)
                .with_pass(pass)
                .with_depth(depth);
            let pipeline_state = self
                .pipelines
                .skinned(self.context.device(), variant)?
                .pipeline_state()
                .clone();
            // Per draw, like the rigid batches: a character whose skin has a normal map and
            // a cloth that does not are two permutations, and the context skips the bind when
            // consecutive submeshes agree. State only - the skinned pipelines share the
            // signature the camera pass bound, and rebinding it here would discard the frame
            // constants and the shadow tables the rigid batches are still using.
            self.commands.set_pipeline_state(&pipeline_state);
            let Some(table) = self.material_table(instance.material) else {
                return Err(RendererError::InvalidConfig {
                    reason: format!(
                        "material {} has no shader-visible base colour map",
                        instance.material.index()
                    ),
                });
            };

            let range = (instance.palette_start, instance.palette_len);
            if *bound_palette != Some(range) {
                // Uploaded once for the frame in `draw_render_list`; the upload here is the
                // fallback for a palette that table does not hold.
                let address = match self.frame_palettes.address(range.0, range.1) {
                    Some(address) => address,
                    None => {
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
                        crate::palette::upload_padded(
                            palette,
                            &mut self.palette_scratch,
                            &mut self.uploads,
                            slot,
                        )?
                    }
                };
                self.commands.set_root_cbv(ROOT_SKIN_CBV, address);
                *bound_palette = Some(range);
            }

            let object = ObjectConstants::shadowed(
                instance.transform.matrix(),
                style,
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
        }
        Ok(())
    }

    /// Draws the translucent-character passes recorded in the most recent frame: two per
    /// submesh of every character the camera was looking through.
    #[must_use]
    pub fn translucent_character_draws_last_frame(&self) -> u32 {
        self.translucent_character_draws_last_frame
    }

    /// Skinned submeshes drawn by the most recent frame.
    ///
    /// Reported separately from `draw_calls_last_frame` so the cost of the character pass is
    /// attributable rather than folded into the arena's.
    #[must_use]
    pub fn skinned_draws_last_frame(&self) -> u32 {
        self.skinned_draws_last_frame
    }

    /// The shader permutation `material` asks for, with this frame's shadow state folded in.
    ///
    /// The material half was settled when the material was created; the shadow half is read
    /// here, every frame, because F6 and the settings screen change it at runtime and a
    /// variant chosen at startup would be the wrong one the moment a player does.
    fn material_variant(&self, material: MaterialHandle) -> PipelineVariant {
        let features = self
            .material_features
            .get(material.index())
            .copied()
            .unwrap_or(ShaderFeatures::ALL)
            .with_shadows(self.shadow.settings().enabled);
        // The class was settled when the material was created and cannot change afterwards,
        // so it is read here rather than passed in: both phases call this, and each gets the
        // blend and depth state its own materials asked for without the call sites having to
        // agree about which phase they are in.
        let surface = self
            .material_surfaces
            .get(material.index())
            .copied()
            .unwrap_or(crate::material::SurfaceClass::Opaque);
        PipelineVariant {
            features,
            wireframe: self.wireframe,
            surface,
            pass: crate::mesh_pipeline::CharacterPass::Lit,
            depth: DepthMode::Write,
        }
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
        if let Some(table) = self.shadow.contact_table() {
            self.commands
                .set_root_descriptor_table(ROOT_CONTACT_TABLE, table);
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

    /// Opens an upload batch: from here until [`Renderer::end_upload_batch`], every
    /// resource created queues its copy into one staging uploader and **nothing waits for
    /// the GPU**.
    ///
    /// # Why this exists
    ///
    /// Every `create_*` call used to build a staging uploader of its own, submit one copy
    /// and then wait for the queue to go idle. That is a full CPU-GPU round trip per texture
    /// and per buffer, and a level's worth of models is a hundred of them — measured at
    /// roughly 800 ms of a 1.4 s launch, carrying about a megabyte in total. The copies were
    /// never the cost; the waiting was.
    ///
    /// # What a caller has to know
    ///
    /// A resource created inside a batch is **not readable by the GPU until the batch ends**.
    /// Descriptors, handles and material tables are all fine to build meanwhile — they name
    /// the resource, they do not read it — so the rule in practice is: end the batch before
    /// the frame that draws with it. The staging memory for everything queued stays alive
    /// until then, which is why this is a load-time tool and not the per-frame path.
    ///
    /// Nesting is not supported: a second call while a batch is open is a no-op, so that a
    /// helper that batches internally can be called from inside a larger batch without
    /// closing it early.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if the uploader's command list cannot be created.
    pub fn begin_upload_batch(&mut self) -> Result<(), RendererError> {
        if self.bulk.is_none() {
            self.bulk = Some(StagingUploader::new(&self.context)?);
        }
        Ok(())
    }

    /// Submits everything the batch queued and waits once. Returns the number of copies.
    ///
    /// Harmless with no batch open, which is what makes it safe on an error path.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if a D3D12 call fails.
    pub fn end_upload_batch(&mut self) -> Result<usize, RendererError> {
        let Some(mut uploader) = self.bulk.take() else {
            return Ok(0);
        };
        let copies = uploader.flush(&self.context)?;
        log::debug!("upload batch: {copies} copies in one submission");
        crate::upload_timing::report();
        Ok(copies)
    }

    /// Whether an upload batch is open.
    #[must_use]
    pub fn is_batching_uploads(&self) -> bool {
        self.bulk.is_some()
    }

    /// Runs `queue` against the batch's uploader, or against a private one that is flushed
    /// immediately when no batch is open.
    ///
    /// The one place the two modes differ, so every `create_*` below reads the same whether
    /// it is being called at load time or in the middle of a session.
    fn queued<R>(
        &mut self,
        queue: impl FnOnce(
            &RenderDevice,
            &mut StagingUploader,
            &mut BufferSource<'_>,
        ) -> Result<R, RendererError>,
    ) -> Result<R, RendererError> {
        // Destructured rather than `self.bulk` and `self.context`, so the borrow checker can
        // see that the uploader, the device and the arena are three different fields.
        let Self {
            context,
            bulk,
            geometry,
            ..
        } = self;
        let mut buffers = BufferSource::Arena(geometry);
        match bulk.as_mut() {
            Some(uploader) => queue(context, uploader, &mut buffers),
            None => {
                let mut uploader = StagingUploader::new(context)?;
                let value = queue(context, &mut uploader, &mut buffers)?;
                uploader.flush(context)?;
                Ok(value)
            }
        }
    }

    /// Uploads a mesh and returns a handle to it.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if a D3D12 call fails.
    pub fn create_mesh(&mut self, data: &MeshData) -> Result<MeshHandle, RendererError> {
        let mesh = self.queued(|device, uploader, buffers| {
            GpuMesh::queue_into(device, uploader, buffers, data)
        })?;
        let index = self.meshes.len() as u32;
        self.meshes.push(mesh);
        Ok(MeshHandle::new(index, 1))
    }

    /// Replaces the geometry behind `handle` with `data`, keeping the handle.
    ///
    /// For geometry that changes at run time — the editor's terrain under a brush — which
    /// would otherwise be a new mesh per change and a mesh leaked per change. The old buffers
    /// are kept until the GPU has passed every frame already submitted, since any of them may
    /// still be drawing the old geometry; they are dropped at the start of a later frame.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] for a handle that names no mesh, or
    /// [`RendererError::Api`] if a D3D12 call fails.
    pub fn replace_mesh(
        &mut self,
        handle: MeshHandle,
        data: &MeshData,
    ) -> Result<(), RendererError> {
        let index = handle.index();
        if index >= self.meshes.len() {
            return Err(RendererError::InvalidConfig {
                reason: format!("mesh {index} was replaced, and {} exist", self.meshes.len()),
            });
        }
        let mesh = self.queued(|device, uploader, buffers| {
            GpuMesh::queue_into(device, uploader, buffers, data)
        })?;
        let old = std::mem::replace(&mut self.meshes[index], mesh);
        self.retired_meshes
            .push((self.frames.last_signaled_value(), old));
        Ok(())
    }

    /// Creates a mesh whose geometry is rewritten at run time, up to once per frame.
    ///
    /// `max_vertices` and `max_indices` fix the capacity for the mesh's whole life. All the
    /// GPU memory is allocated here - `frames_in_flight + 1` persistently mapped upload-heap
    /// copies - so [`Renderer::write_dynamic_mesh`] allocates nothing, which is what
    /// [`Renderer::replace_mesh`] cannot offer: it suballocates from an arena that never
    /// frees piecemeal. The mesh draws nothing until its first write. The handle works
    /// anywhere a mesh handle does; see [`crate::dynamic_mesh`] for the safety argument.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] for a zero capacity, or
    /// [`RendererError::Api`] if a D3D12 call fails.
    pub fn create_dynamic_mesh(
        &mut self,
        max_vertices: usize,
        max_indices: usize,
    ) -> Result<MeshHandle, RendererError> {
        let (dynamic, mesh) = crate::dynamic_mesh::DynamicMesh::new(
            &self.context,
            self.frames.frames_in_flight(),
            max_vertices,
            max_indices,
        )?;
        let index = self.meshes.len();
        self.meshes.push(mesh);
        self.dynamic_meshes.insert(index, dynamic);
        Ok(MeshHandle::new(index as u32, 1))
    }

    /// Replaces the geometry of a dynamic mesh.
    ///
    /// Takes slices rather than [`MeshData`] so a caller can reuse its buffers and allocate
    /// nothing per frame. An empty `indices` slice is allowed and means "draw nothing".
    /// `bounds` is the local-space sphere used for culling.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] if `handle` is not a dynamic mesh or the
    /// geometry exceeds the capacity given to [`Renderer::create_dynamic_mesh`].
    pub fn write_dynamic_mesh(
        &mut self,
        handle: MeshHandle,
        vertices: &[MeshVertex],
        indices: &[crate::mesh::Index],
        bounds: engine_core::math::Sphere,
    ) -> Result<(), RendererError> {
        let index = handle.index();
        let frame = self.frames.frame_index();
        let (Some(dynamic), Some(mesh)) = (
            self.dynamic_meshes.get_mut(&index),
            self.meshes.get_mut(index),
        ) else {
            return Err(RendererError::InvalidConfig {
                reason: format!("mesh {index} is not a dynamic mesh"),
            });
        };
        dynamic.write(mesh, frame, vertices, indices, bounds)
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
        let mesh = self.queued(|device, uploader, buffers| {
            GpuMesh::queue_skinned_into(device, uploader, buffers, data)
        })?;
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
        let identity = TextureIdentity::of(image)?;
        self.count_texture_request(image);
        // The whole point: the same image asked for twice is uploaded once. Checked before
        // anything is allocated or filtered, so a hit costs one hash of the texels and no
        // GPU work at all — no resource, no staging buffer, no descriptor, no flush.
        if let Some(existing) = self.resident_texture(&identity.key, image) {
            return Ok(existing);
        }
        let prepared = identity.prepare(image);
        self.upload_prepared_texture(image, prepared)
    }

    /// [`Renderer::create_texture`] for a texture whose CPU work [`prepare_texture`] has
    /// already done, on whatever thread did it.
    ///
    /// # Errors
    ///
    /// As [`Renderer::create_texture`].
    pub fn create_texture_prepared(
        &mut self,
        image: &TextureImage<'_>,
        prepared: PreparedTexture,
    ) -> Result<TextureHandle, RendererError> {
        if !prepared.describes(image) {
            // Prepared from a different image: do the work again rather than upload the
            // wrong picture.
            return self.create_texture(image);
        }
        self.count_texture_request(image);
        if let Some(existing) = self.resident_texture(&prepared.key, image) {
            return Ok(existing);
        }
        self.upload_prepared_texture(image, prepared)
    }

    /// The per-request counters, deduplicated hits included.
    fn count_texture_request(&mut self, image: &TextureImage<'_>) {
        self.texture_requests += 1;
        // What this request would have cost before Milestone 4: its own RGBA8 copy, with the
        // mip chain it now has. Counted per request, so the deduplicated ones are counted too
        // — they are exactly what the saving is made of.
        self.texture_bytes_naive += rgba_chain_bytes(image.width, image.height);
    }

    /// The handle of a texture with this content already resident, if there is one.
    fn resident_texture(
        &self,
        key: &TextureKey,
        image: &TextureImage<'_>,
    ) -> Option<TextureHandle> {
        let existing = self.textures_by_content.get(key)?;
        log::trace!(
            "texture: reusing {} for a {}x{} image already resident",
            existing.index(),
            image.width,
            image.height
        );
        Some(*existing)
    }

    /// Creates the resource and its view for a prepared texture and queues its levels.
    fn upload_prepared_texture(
        &mut self,
        image: &TextureImage<'_>,
        prepared: PreparedTexture,
    ) -> Result<TextureHandle, RendererError> {
        use crate::mipmap::Compression;
        use windows::Win32::Graphics::Direct3D12::{
            D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
            D3D12_SHADER_RESOURCE_VIEW_DESC, D3D12_SHADER_RESOURCE_VIEW_DESC_0,
            D3D12_SRV_DIMENSION_TEXTURE2D, D3D12_TEX2D_SRV,
        };
        use windows::Win32::Graphics::Dxgi::Common::{
            DXGI_FORMAT_BC1_UNORM, DXGI_FORMAT_BC1_UNORM_SRGB, DXGI_FORMAT_BC5_UNORM,
            DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
        };

        let PreparedTexture {
            key,
            compression,
            mip_levels,
            chain,
            compressed,
            ..
        } = prepared;

        // The *view* format carries the colour space for the uncompressed formats — the
        // texels are the same bytes either way and it is the read that linearizes — and the
        // compressed formats carry it in the format itself, because BC1 has an sRGB variant
        // and BC5 does not need one.
        let format = match (compression, image.is_srgb()) {
            (Compression::Bc1, true) => DXGI_FORMAT_BC1_UNORM_SRGB,
            (Compression::Bc1, false) => DXGI_FORMAT_BC1_UNORM,
            (Compression::Bc5, _) => DXGI_FORMAT_BC5_UNORM,
            (Compression::None, true) => DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
            (Compression::None, false) => DXGI_FORMAT_R8G8B8A8_UNORM,
        };

        let texture = crate::resource::GpuTexture::with_mips(
            &self.context,
            image.width,
            image.height,
            mip_levels,
            format,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )?;

        let levels: Vec<&[u8]> = if compressed.is_empty() {
            std::iter::once(image.texels)
                .chain(chain.iter().map(Vec::as_slice))
                .collect()
        } else {
            log::debug!(
                "texture: {}x{} {:?} compressed to {:?}, {} KB against {} KB",
                image.width,
                image.height,
                image.kind,
                compression,
                compressed.iter().map(Vec::len).sum::<usize>() / 1024,
                (image.texels.len() * 4 / 3) / 1024
            );
            compressed.iter().map(Vec::as_slice).collect()
        };

        crate::upload_timing::digest_texture(&levels);
        self.queued(|device, uploader, _| {
            uploader.queue_texture_mips(
                device,
                &texture,
                &levels,
                D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
            )
        })?;

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
                    // The whole chain. A view that named one level would leave the sampler
                    // no level to fall to, and the mips would be memory nothing reads.
                    MipLevels: mip_levels,
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
            compression,
            texture,
            format,
            descriptor,
        });
        let handle = TextureHandle(index);
        self.textures_by_content.insert(key, handle);
        self.texture_bytes += levels.iter().map(|level| level.len()).sum::<usize>();
        Ok(handle)
    }

    /// Bytes of GPU memory the resident textures occupy, and what the same images would have
    /// cost as uncompressed RGBA8 with one copy per request.
    ///
    /// The pair is the Milestone 4 saving, measured rather than estimated: deduplication
    /// removes whole copies and block compression shrinks the ones that remain.
    #[must_use]
    pub fn texture_memory(&self) -> (usize, usize) {
        (self.texture_bytes, self.texture_bytes_naive)
    }

    /// How many times a texture has been asked for, deduplicated hits included.
    #[must_use]
    pub fn texture_requests(&self) -> usize {
        self.texture_requests
    }

    /// How an uploaded texture was compressed, or `None` for a handle that names nothing.
    ///
    /// For tests and diagnostics. Compression is chosen from a texture's kind and size — see
    /// [`crate::mipmap::compression_for`] — and this is how a test asks what was chosen
    /// without a second copy of that policy.
    #[must_use]
    pub fn texture_compression(
        &self,
        texture: TextureHandle,
    ) -> Option<crate::mipmap::Compression> {
        self.textures
            .get(texture.index())
            .map(|entry| entry.compression)
    }

    /// How many mip levels an uploaded texture has, or `None` for a handle that names
    /// nothing.
    ///
    /// For tests and diagnostics: "does this texture have a chain at all" is a question
    /// about the resource rather than about the picture, and the picture cannot answer it
    /// without a camera far enough away to select a lower level.
    #[must_use]
    pub fn texture_mip_levels(&self, texture: TextureHandle) -> Option<u32> {
        self.textures
            .get(texture.index())
            .map(|entry| entry.texture.mip_levels())
    }

    /// How many distinct textures are resident on the GPU.
    ///
    /// Against [`Renderer::texture_requests`], the deduplication ratio.
    #[must_use]
    pub fn texture_count(&self) -> usize {
        self.textures.len()
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
        self.create_model_prepared(model, None)
    }

    /// [`Renderer::create_model`] with its textures' CPU work done in advance by
    /// [`prepare_model_textures`] — typically on a thread, while something else had the
    /// device. `None`, or textures prepared from a different model, does that work here.
    ///
    /// # Errors
    ///
    /// As [`Renderer::create_model`].
    pub fn create_model_prepared(
        &mut self,
        model: &engine_assets::ModelAsset,
        prepared: Option<PreparedTextures>,
    ) -> Result<ModelBinding, RendererError> {
        use engine_assets::mesh::Geometry;

        engine_assets::validate_model(model, &engine_assets::ValidationLimits::default()).map_err(
            |error| RendererError::InvalidConfig {
                reason: error.to_string(),
            },
        )?;

        let kinds = texture_kinds(model);

        // Textures first, then materials that name them, then geometry that names materials.
        //
        // Uploading is deduplicated by content inside `create_texture`, so a model whose
        // materials share one image — an ORM map used by every material of a building — pays
        // for one resource and one descriptor however many times it is named.
        //
        // Prepared textures — filtered and compressed on another thread before the model got
        // here — are used when they were prepared from this model; anything else is
        // prepared now, exactly as it always was.
        let mut prepared = prepared
            .filter(|prepared| prepared.textures.len() == model.textures.len())
            .map(|prepared| prepared.textures.into_iter().map(Some).collect::<Vec<_>>())
            .unwrap_or_default();
        let mut textures = Vec::with_capacity(model.textures.len());
        for (index, texture) in model.textures.iter().enumerate() {
            let image = TextureImage {
                width: texture.width,
                height: texture.height,
                kind: kinds[index],
                texels: &texture.texels,
            };
            let handle = match prepared.get_mut(index).and_then(Option::take) {
                Some(prepared) => self.create_texture_prepared(&image, prepared)?,
                None => self.create_texture(&image)?,
            };
            textures.push(handle);
        }

        let mut materials = Vec::with_capacity(model.materials.len());
        for material in &model.materials {
            let desc = MaterialDesc {
                base_color: material.base_color,
                roughness: material.roughness,
                metallic: material.metallic,
                emissive: material.emissive,
                // The two glTF texture parameters, which the importer reads and the shader
                // applies. They ride in what used to be `MaterialConstants`' padding.
                normal_scale: material.normal_scale,
                occlusion_strength: material.occlusion_strength,
                // Carried through to the shadow pass as well as the camera pass: a cutout
                // material that shadowed as an opaque rectangle would be worse than one
                // that did not shadow at all.
                alpha_cutoff: material.alpha_cutoff,
                // glTF's alpha mode, carried through rather than re-derived: `BLEND` becomes a
                // transparent material drawn in the sorted phase, `MASK` a cutout, `OPAQUE`
                // neither. Before the transparent phase existed the importer had to turn
                // `BLEND` into a cutout, which gave a window a hard edge instead of a fade.
                surface: material.surface.into(),
                // Imported assets are Lambert like everything else until the game asks for
                // the BRDF. The roughness, metallic and ORM map the importer read are still
                // carried on the material - opting in is one call, not a re-import.
                pbr: false,
            };
            // Every map the asset carries, not just the base colour: an importer that
            // dropped the normal and ORM maps here is what this milestone removes. A slot
            // the asset leaves empty stays `None` and resolves to that slot's shared
            // fallback inside `create_material_with_textures`.
            let resolve =
                |index: Option<usize>| index.and_then(|index| textures.get(index).copied());
            let set = MaterialTextureSet {
                base_color: resolve(material.base_color_texture),
                normal: resolve(material.normal_texture),
                orm: resolve(material.orm_texture),
                emissive: resolve(material.emissive_texture),
            };
            materials.push(self.create_material_with_textures(desc, set)?);
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
                                // Tangent and colour come across unchanged: the asset crate
                                // is where they are imported or generated, and the renderer
                                // is not entitled to a second opinion about a mesh.
                                MeshVertex::new(vertex.position, vertex.normal, vertex.uv)
                                    .with_tangent(vertex.tangent)
                                    .with_color(vertex.color)
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
                                .with_tangent(vertex.tangent)
                                .with_color(vertex.color)
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

    /// Replaces the rigid geometry of a model uploaded by [`Renderer::create_model`] with
    /// `model`'s, keeping every handle — the meshes, the materials and the binding.
    ///
    /// `model` must have the shape `binding` was made from: the same levels of detail with
    /// the same number of parts. Its materials are not looked at; the binding keeps the ones
    /// it has. This is what run-time geometry — the editor's terrain — is rebuilt with, so a
    /// change costs an upload and not a new mesh and material every time.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] when the shapes do not match or a part is
    /// skinned, or [`RendererError::Api`] if a D3D12 call fails.
    pub fn replace_model_geometry(
        &mut self,
        binding: &ModelBinding,
        model: &engine_assets::ModelAsset,
    ) -> Result<(), RendererError> {
        use engine_assets::mesh::Geometry;
        let mismatch = || RendererError::InvalidConfig {
            reason: format!(
                "'{}' does not have the levels and parts of the model it replaces",
                model.name
            ),
        };
        if model.lods.len() != binding.lods.len() {
            return Err(mismatch());
        }
        for (lod, bound) in model.lods.iter().zip(&binding.lods) {
            if lod.submeshes.len() != bound.parts.len() {
                return Err(mismatch());
            }
            for (submesh, part) in lod.submeshes.iter().zip(&bound.parts) {
                let (Geometry::Static(vertices), PartGeometry::Rigid(mesh)) =
                    (&submesh.geometry, part.geometry)
                else {
                    return Err(mismatch());
                };
                let vertices = vertices
                    .iter()
                    .map(|vertex| {
                        MeshVertex::new(vertex.position, vertex.normal, vertex.uv)
                            .with_tangent(vertex.tangent)
                            .with_color(vertex.color)
                    })
                    .collect();
                let data = MeshData::new(vertices, submesh.indices.clone())?;
                self.replace_mesh(mesh, &data)?;
            }
        }
        Ok(())
    }

    /// How many materials exist. Identical requests share one, so this counts distinct ones.
    #[must_use]
    pub fn material_count(&self) -> usize {
        self.materials.len()
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
        // The distance-field fonts: uploaded with the atlas, once.
        let font = crate::resource::GpuTexture::new(
            context,
            crate::text::ATLAS_WIDTH as u32,
            crate::text::ATLAS_HEIGHT as u32,
            DXGI_FORMAT_R8G8B8A8_UNORM,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )?;
        uploader.queue_texture(
            context,
            &font,
            &crate::text::atlas_rgba(),
            windows::Win32::Graphics::Direct3D12::D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
        )?;
        uploader.flush(context)?;

        // One slot out of the renderer's shared heap. The UI binds a single SRV that never
        // changes; what changed in Phase 5 is that it is no longer the only one.
        let atlas_descriptor = descriptors.allocate_persistent()?;
        let font_descriptor = descriptors.allocate_persistent()?;

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
        // SAFETY: as above, for the font atlas and its own slot.
        unsafe {
            context.device().CreateShaderResourceView(
                Some(crate::resource::TrackedResource::resource(&font)),
                Some(&desc),
                font_descriptor.cpu,
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
            map: None,
            map_descriptor: None,
            font,
            font_descriptor,
        })
    }

    /// Replaces the UI to draw this frame and batches it into vertices.
    ///
    /// Batching happens here rather than during recording so its cost is attributable and
    /// so `render` stays a function of data that is already prepared.
    pub fn set_ui(&mut self, list: &engine_render_data::UiDrawList) {
        self.set_ui_with(list, None);
    }

    /// [`Renderer::set_ui`] with the expansion optionally spread across `jobs`' workers.
    ///
    /// The vertex buffer is identical either way; only the time changes. Below the
    /// batcher's own size threshold the expansion stays on the calling thread whatever is
    /// passed here.
    pub fn set_ui_with(
        &mut self,
        list: &engine_render_data::UiDrawList,
        jobs: Option<&engine_core::jobs::JobSystem>,
    ) {
        let started = std::time::Instant::now();
        self.ui.list.clear();
        self.ui.list.append(list);
        self.ui.batcher.build_with(&self.ui.list, jobs);
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
        self.batch_ui_with(None);
    }

    /// [`Renderer::batch_ui`] with the expansion optionally spread across `jobs`' workers.
    pub fn batch_ui_with(&mut self, jobs: Option<&engine_core::jobs::JobSystem>) {
        let started = std::time::Instant::now();
        self.ui.batcher.build_with(&self.ui.list, jobs);
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

    /// Uploads the level's map picture for the UI to draw, replacing any before it.
    ///
    /// Straight RGBA8, drawn through a data view — the bytes reach the screen as they are, the
    /// same as the atlas — with a full mip chain filtered on the CPU, so the minimap minifies it
    /// without shimmering. Waits for the GPU to go idle first: the descriptor slot is reused,
    /// and a frame in flight may still be reading the old picture through it. That makes this a
    /// load-time call, never a per-frame one.
    ///
    /// # Errors
    ///
    /// [`RendererError::InvalidConfig`] for texels that do not match the size, or the D3D12
    /// failure creating or uploading the texture.
    pub fn set_ui_map_texture(
        &mut self,
        width: u32,
        height: u32,
        texels: &[u8],
    ) -> Result<(), RendererError> {
        use windows::Win32::Graphics::Direct3D12::{
            D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
            D3D12_SHADER_RESOURCE_VIEW_DESC, D3D12_SHADER_RESOURCE_VIEW_DESC_0,
            D3D12_SRV_DIMENSION_TEXTURE2D, D3D12_TEX2D_SRV,
        };
        use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;

        let expected = width as usize * height as usize * 4;
        if width == 0 || height == 0 || texels.len() != expected {
            return Err(RendererError::InvalidConfig {
                reason: format!(
                    "a {width}x{height} map needs {expected} bytes, got {}",
                    texels.len()
                ),
            });
        }
        self.wait_for_idle()?;
        self.ui.map = None;

        let mip_levels = crate::mipmap::level_count(width, height);
        let texture = crate::resource::GpuTexture::with_mips(
            &self.context,
            width,
            height,
            mip_levels,
            DXGI_FORMAT_R8G8B8A8_UNORM,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )?;
        let chain =
            crate::mipmap::generate(texels, width, height, crate::mipmap::TextureKind::Data);
        let mut levels: Vec<&[u8]> = Vec::with_capacity(chain.len() + 1);
        levels.push(texels);
        levels.extend(chain.iter().map(Vec::as_slice));
        self.queued(|device, uploader, _| {
            uploader.queue_texture_mips(
                device,
                &texture,
                &levels,
                D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
            )
        })?;

        let descriptor = match self.ui.map_descriptor {
            Some(descriptor) => descriptor,
            None => {
                let descriptor = self.descriptors.allocate_persistent()?;
                self.ui.map_descriptor = Some(descriptor);
                descriptor
            }
        };
        let desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
            Shader4ComponentMapping: 0x1688,
            Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                Texture2D: D3D12_TEX2D_SRV {
                    MostDetailedMip: 0,
                    MipLevels: mip_levels,
                    PlaneSlice: 0,
                    ResourceMinLODClamp: 0.0,
                },
            },
        };
        // SAFETY: FFI call. The texture is kept alive in `self.ui.map` for as long as the
        // descriptor names it, and the CPU handle addresses a slot this heap allocated.
        unsafe {
            self.context.device().CreateShaderResourceView(
                Some(crate::resource::TrackedResource::resource(&texture)),
                Some(&desc),
                descriptor.cpu,
            );
        }
        self.ui.map = Some(UiMapPicture {
            texture,
            size: (width, height),
            bytes: levels.iter().map(|level| level.len()).sum(),
        });
        Ok(())
    }

    /// Forgets the map picture: `UiCommand::MapImage` draws nothing until the next one.
    ///
    /// # Errors
    ///
    /// The fence failure, if waiting for the GPU to finish with the old picture fails.
    pub fn clear_ui_map_texture(&mut self) -> Result<(), RendererError> {
        if self.ui.map.is_some() {
            self.wait_for_idle()?;
            self.ui.map = None;
        }
        Ok(())
    }

    /// The map picture's size in texels and its GPU memory in bytes, if one is set.
    #[must_use]
    pub fn ui_map_texture(&self) -> Option<((u32, u32), usize)> {
        self.ui.map.as_ref().map(|map| (map.size, map.bytes))
    }

    /// Asks for the next frame recorded — interface, overlay and all, exactly as presented — to
    /// be kept. Collect it with [`Renderer::take_screenshot`] after that frame is presented.
    ///
    /// For visual checks at a given resolution without a person at the screen; it waits for the
    /// GPU when collected, so it is a tool, not a feature.
    ///
    /// # Errors
    ///
    /// The readback buffer's creation failure.
    pub fn request_screenshot(&mut self) -> Result<(), RendererError> {
        let (width, height) = self.swap_chain.size();
        self.screenshot = Some(crate::offscreen::Readback::for_texture(
            self.context.device(),
            width,
            height,
            crate::swapchain::BACK_BUFFER_FORMAT,
        )?);
        Ok(())
    }

    /// The frame [`Renderer::request_screenshot`] kept, once it has been presented. Waits for
    /// the GPU to finish it.
    ///
    /// # Errors
    ///
    /// The fence wait or the readback's failure.
    pub fn take_screenshot(
        &mut self,
    ) -> Result<Option<crate::offscreen::CapturedImage>, RendererError> {
        if self.recorded_frame_slot.is_some() {
            // Recorded but not yet presented: not finished, not collectable.
            return Ok(None);
        }
        let Some(readback) = self.screenshot.take() else {
            return Ok(None);
        };
        self.wait_for_idle()?;
        Ok(Some(crate::offscreen::CapturedImage {
            width: readback.width,
            height: readback.height,
            pixels: readback.pixels()?,
        }))
    }

    /// Draws `world` through `camera` into the back buffer and reads the pixels back, without
    /// presenting them.
    ///
    /// The dungeon map's bake: the *real* frame — the same culling, materials, textures,
    /// lights and shadows the game draws with — seen from a top-down orthographic camera, with
    /// no particles, no overlay and no interface on top. The frame is fenced and waited for
    /// rather than presented, so it never reaches the screen, and the next ordinary frame
    /// draws into the same back buffer as if nothing had happened.
    ///
    /// Synchronous and slow by design — it waits for the GPU — so it is a load-time or an
    /// editor action, never a per-frame one. The image is the back buffer's size.
    ///
    /// # Errors
    ///
    /// Anything [`Renderer::render_world`] can fail with, or the readback's own failure.
    pub fn capture_world(
        &mut self,
        world: &RenderWorld,
        camera: &Camera,
        lighting: Lighting,
        color: [f32; 4],
    ) -> Result<crate::offscreen::CapturedImage, RendererError> {
        let (width, height) = self.swap_chain.size();
        self.capture = Some(crate::offscreen::Readback::for_texture(
            self.context.device(),
            width,
            height,
            crate::swapchain::BACK_BUFFER_FORMAT,
        )?);
        self.build_render_list(world, camera);
        self.prepare_shadows(world, camera, lighting);
        let drawn = self.draw_render_list(camera, lighting, color);
        let capture = self.capture.take();
        drawn?;
        // Fenced rather than presented: the slot's fence covers the submission, the ring
        // moves on exactly as it would have, and the swap chain never sees the frame.
        if let Some(slot) = self.recorded_frame_slot.take() {
            self.frames.end_frame(self.context.queue(), slot)?;
        }
        self.wait_for_idle()?;
        let Some(capture) = capture else {
            return Err(RendererError::InvalidConfig {
                reason: "the capture target vanished while its frame was recorded".to_owned(),
            });
        };
        Ok(crate::offscreen::CapturedImage {
            width,
            height,
            pixels: capture.pixels()?,
        })
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
        // The map, or the atlas standing in for it: a table the signature declares is bound on
        // every draw, so a map vertex with no map reads a valid texture rather than garbage.
        let map_table = match (&self.ui.map, self.ui.map_descriptor) {
            (Some(_), Some(crate::descriptor::DescriptorHandle { gpu: Some(gpu), .. })) => gpu,
            _ => table,
        };
        self.commands
            .set_root_descriptor_table(crate::ui_pipeline::ROOT_UI_MAP_TABLE, map_table);
        let font_table = self.ui.font_descriptor.gpu.unwrap_or(table);
        self.commands
            .set_root_descriptor_table(crate::ui_pipeline::ROOT_UI_FONT_TABLE, font_table);
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

    /// What the camera fades out of its way from the next frame on, or `None` for nothing.
    ///
    /// Only entities whose style is [`engine_render_data::InstanceStyle::see_through`] are
    /// affected, and only by a dither in the pass they are already drawn in: no phase, sort
    /// or pipeline changes. Kept until it is set again.
    pub fn set_camera_fade(&mut self, fade: Option<engine_render_data::CameraFade>) {
        self.camera_fade = fade;
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

    /// Uploads the four shared fallback maps and returns them as a texture set.
    ///
    /// One 1x1 texture per slot, created once at renderer construction and pointed at by
    /// every material that leaves that slot empty. Creating one per absent map would
    /// multiply a scene's texture count by the number of maps its artists did not author,
    /// which is most of them.
    ///
    /// The texels and colour spaces come from [`MaterialTextureSlot`] rather than being
    /// written here, so the offscreen paths build the identical set from the identical
    /// definition — see [`MaterialTextureSlot::fallback_texel`] for what each one is and
    /// why.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if a D3D12 call fails.
    fn create_fallback_textures(&mut self) -> Result<MaterialTextures, RendererError> {
        let mut handles = [TextureHandle(0); MATERIAL_TEXTURE_COUNT as usize];
        for slot in MaterialTextureSlot::ALL {
            handles[slot.register() as usize] = self.create_texture(&TextureImage {
                width: 1,
                height: 1,
                kind: slot.texture_kind(),
                texels: &slot.fallback_texel(),
            })?;
        }
        Ok(MaterialTextures {
            base_color: handles[MaterialTextureSlot::BaseColor.register() as usize],
            normal: handles[MaterialTextureSlot::Normal.register() as usize],
            orm: handles[MaterialTextureSlot::Orm.register() as usize],
            emissive: handles[MaterialTextureSlot::Emissive.register() as usize],
        })
    }

    /// The renderer's shared fallback maps, for tests and diagnostics.
    ///
    /// Public because "does a material with no normal map really sample the flat normal?" is
    /// a question about the binding rather than about the picture, and answering it by
    /// reading pixels would need a lighting model that consumes the map.
    #[must_use]
    pub fn fallback_textures(&self) -> MaterialTextures {
        self.fallback_textures
    }

    /// Uploads a material with no maps of its own and returns a handle to it.
    ///
    /// Every slot resolves to its shared fallback, so the material still binds four valid
    /// descriptors — there is no such thing as an untextured material at the ABI.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] for out-of-range parameters, or
    /// [`RendererError::Api`] if a D3D12 call fails.
    pub fn create_material(&mut self, desc: MaterialDesc) -> Result<MaterialHandle, RendererError> {
        self.create_material_with_textures(desc, MaterialTextureSet::default())
    }

    /// Uploads a material that samples `texture` for its base colour.
    ///
    /// `None` binds the shared white texel. The map *multiplies* the material's base colour
    /// rather than replacing it, which is what lets one texture serve several tinted
    /// materials.
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
        self.create_material_with_textures(
            desc,
            MaterialTextureSet {
                base_color: texture,
                ..MaterialTextureSet::default()
            },
        )
    }

    /// Uploads a material with a full texture set and returns a handle to it.
    ///
    /// Any slot left `None` resolves to that slot's shared fallback, so the material's
    /// descriptor range always holds [`MATERIAL_TEXTURE_COUNT`] valid SRVs in
    /// [`MaterialTextureSlot::ALL`] order. The range is one contiguous allocation out of the
    /// persistent region, taken once here and never touched again — a texture's resource is
    /// fixed at creation, so there is nothing to invalidate the table later.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] for out-of-range parameters, a texture
    /// handle that names nothing, or a full descriptor region; or [`RendererError::Api`] if
    /// a D3D12 call fails.
    pub fn create_material_with_textures(
        &mut self,
        desc: MaterialDesc,
        textures: MaterialTextureSet,
    ) -> Result<MaterialHandle, RendererError> {
        for (slot, handle) in textures.named() {
            if handle.index() >= self.textures.len() {
                return Err(RendererError::InvalidConfig {
                    reason: format!(
                        "material names texture {} for its {} map, and {} exist",
                        handle.index(),
                        slot.name(),
                        self.textures.len()
                    ),
                });
            }
        }
        // Before `resolve`, which replaces every empty slot with a fallback and so loses
        // the one thing the permutation needs to know: whether this material actually has a
        // normal map, or was about to sample the flat texel and get its own geometric normal
        // back for 0.071 ms of tangent-frame work.
        let features = ShaderFeatures {
            pbr: desc.pbr,
            normal_map: textures.normal.is_some(),
            shadows: false,
        };
        let textures = textures.resolve(&self.fallback_textures);

        // A material is fixed at creation — its constants and its descriptor table are never
        // written again — so an identical request can share the one that exists. Without
        // this, anything rebuilt at run time leaked a four-descriptor table per rebuild: the
        // editor's terrain is rebuilt on every frame of a brush stroke, and a few seconds of
        // flattening filled the 4096-descriptor persistent region, after which nothing could
        // be uploaded at all and the ground vanished.
        if let Some(index) = (0..self.materials.len()).find(|index| {
            self.material_descs[*index] == desc
                && self.material_features[*index] == features
                && self.material_bindings[*index].textures == textures
        }) {
            return Ok(MaterialHandle::new(index as u32, 1));
        }

        // The constants first: a failure there must not leave a descriptor range allocated
        // to a material that does not exist.
        let material = self.queued(|device, uploader, buffers| {
            Material::queue_into(device, uploader, buffers, desc)
        })?;
        let range = self
            .descriptors
            .allocate_persistent_range(MATERIAL_TEXTURE_COUNT)?;
        self.write_material_table(range, &textures);

        let index = self.materials.len() as u32;
        self.materials.push(material);
        self.material_features.push(features);
        self.material_descs.push(desc);
        self.material_surfaces.push(desc.surface);
        self.material_bindings
            .push(MaterialTextureBinding { textures, range });
        self.material_tables.push(range.gpu);
        Ok(MaterialHandle::new(index, 1))
    }

    /// Writes one SRV per slot into `range`, in descriptor order.
    ///
    /// The **only** place the semantic texture set becomes positional. The view is created
    /// afresh rather than copied from the texture's standalone descriptor because D3D12's
    /// descriptor copy requires a non-shader-visible source heap, and a staging heap to
    /// enable a copy costs more than calling `CreateShaderResourceView` a second time.
    fn write_material_table(
        &self,
        range: crate::descriptor::DescriptorHandle,
        textures: &MaterialTextures,
    ) {
        use windows::Win32::Graphics::Direct3D12::{
            D3D12_CPU_DESCRIPTOR_HANDLE, D3D12_SHADER_RESOURCE_VIEW_DESC,
            D3D12_SHADER_RESOURCE_VIEW_DESC_0, D3D12_SRV_DIMENSION_TEXTURE2D, D3D12_TEX2D_SRV,
        };

        let increment = self.descriptors.increment() as usize;
        for slot in MaterialTextureSlot::ALL {
            let handle = textures.slot(slot);
            let Some(uploaded) = self.textures.get(handle.index()) else {
                // Unreachable: every handle was range-checked or came from the fallback set,
                // and both name a live texture. Skipping rather than panicking leaves the
                // descriptor as the heap found it, which the debug layer names on use.
                debug_assert!(false, "material table names texture {}", handle.index());
                continue;
            };
            let desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
                Format: uploaded.format,
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
            let cpu = D3D12_CPU_DESCRIPTOR_HANDLE {
                ptr: range.cpu.ptr + slot.register() as usize * increment,
            };
            // SAFETY: FFI call. The texture outlives the renderer, and the handle addresses
            // the `slot`-th descriptor of a range this renderer just allocated — the range
            // is MATERIAL_TEXTURE_COUNT long and `slot.register()` is below that by
            // construction.
            unsafe {
                self.context.device().CreateShaderResourceView(
                    Some(crate::resource::TrackedResource::resource(
                        &uploaded.texture,
                    )),
                    Some(&desc),
                    cpu,
                );
            }
        }
    }

    /// The texture set `material` samples, by slot.
    ///
    /// Public because the descriptor ABI is what this milestone freezes, and a test that
    /// could only observe it through pixels would be a test of the lighting model instead.
    #[must_use]
    pub fn material_textures(&self, material: MaterialHandle) -> Option<MaterialTextures> {
        self.material_bindings
            .get(material.index())
            .map(|binding| binding.textures)
    }

    /// The first heap slot of `material`'s four-descriptor table.
    ///
    /// The range is contiguous, so slot `n` of the set is at this index plus `n`. Public for
    /// the same reason as [`Self::material_textures`].
    #[must_use]
    pub fn material_descriptor_range(&self, material: MaterialHandle) -> Option<u32> {
        self.material_bindings
            .get(material.index())
            .map(|binding| binding.range.index)
    }

    /// The heap slot a texture's standalone view occupies.
    ///
    /// What lets a test say "the descriptor at `t1` of this material's table views the same
    /// resource as texture N" without a GPU capture.
    #[must_use]
    pub fn texture_descriptor(&self, texture: TextureHandle) -> Option<u32> {
        self.textures
            .get(texture.index())
            .map(|uploaded| uploaded.descriptor.index)
    }

    /// Usage of the shader-visible descriptor heap's persistent region.
    ///
    /// What answers "how many descriptors does a scene cost?" — one per uploaded texture,
    /// [`MATERIAL_TEXTURE_COUNT`] per material, one for the UI atlas, and the shadow stage's
    /// own views.
    #[must_use]
    pub fn descriptor_stats(&self) -> crate::descriptor::DescriptorStats {
        self.descriptors.persistent_stats()
    }

    /// The descriptor table to bind for `material`'s texture set.
    ///
    /// Returns `None` for a material that was never created, or whose range is somehow not
    /// shader visible — both of which are recording bugs rather than content ones, and both
    /// of which the caller turns into a named error rather than a wrong picture.
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
        // Last frame's palette addresses point into upload memory another slot owns. Every
        // recording path starts here, so none of them can bind one: a path that does not
        // fill the table falls back to uploading its own.
        self.frame_palettes.clear();

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
        for (pass, entry) in self.last_pass_ms.iter_mut().enumerate() {
            if let Some(milliseconds) = self.gpu_timer.read_pass_ms(slot, pass)? {
                *entry = Some(milliseconds);
            }
        }
        let shadow_ms = self.gpu_timer.read_pass_ms(slot, SHADOW_PASS_QUERY)?;
        self.shadow.set_gpu_ms(shadow_ms);
        if let Some(statistics) = self.pipeline_statistics.as_ref()
            && let Some(read) = statistics.read(slot)?
        {
            self.last_scene_statistics = Some(read);
        }

        // Resources whose last use the GPU has now passed. Costs a comparison per frame
        // when the queue is empty, which it usually is.
        self.pending_release.collect(self.frames.completed_value());
        let completed = self.frames.completed_value();
        self.retired_meshes
            .retain(|(last_use, _)| *last_use > completed);

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
        // The mesh pass ends here, wherever it was recorded from — the instanced path and
        // the batch-by-batch path both arrive at this function and nowhere else.
        self.gpu_timer
            .end_pass(self.commands.list(), slot, SCENE_PASS);
        if let Some(statistics) = self.pipeline_statistics.as_mut() {
            statistics.end(self.commands.list(), slot);
        }

        // Before the interface and after the scene: a blended particle has to see the opaque
        // world beneath it, and a health bar under a fireball is a health bar nobody can
        // read.
        // A capture is the level and nothing on top of it: no particles, no overlay, no UI.
        let capturing = self.capture.is_some();

        self.gpu_timer
            .begin_pass(self.commands.list(), slot, PARTICLE_PASS);
        if !capturing {
            self.draw_particles(slot)?;
        }
        self.gpu_timer
            .end_pass(self.commands.list(), slot, PARTICLE_PASS);

        // The overlay and the game UI are one pass for timing: both draw screen-space
        // triangles over the finished scene, and splitting them would say nothing the two
        // vertex counts in `CommandStats` do not already.
        self.gpu_timer
            .begin_pass(self.commands.list(), slot, UI_PASS);

        #[cfg(feature = "debug-overlay")]
        let index = slot;

        // The overlay needs its own pipeline: it draws clip-space position+colour vertices,
        // which the mesh pipeline's input layout does not describe. Binding it here rather
        // than assuming whatever the scene left bound is what makes both draw paths work.
        #[cfg(feature = "debug-overlay")]
        if !capturing {
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
        if !capturing {
            self.draw_ui(slot)?;
        }
        self.gpu_timer.end_pass(self.commands.list(), slot, UI_PASS);

        // Re-borrowed after `draw_ui`, which needs `&mut self`. The buffer is the same
        // resource either way; holding the earlier borrow across the call would only be a
        // borrow-checker argument, not a correctness one.
        let buffer = self.swap_chain.buffer(back_buffer);
        if let Some(capture) = self.capture.as_ref().or(self.screenshot.as_ref()) {
            self.commands.transition_to(
                buffer,
                windows::Win32::Graphics::Direct3D12::D3D12_RESOURCE_STATE_COPY_SOURCE,
            );
            self.commands.flush_barriers();
            capture.record_copy(
                self.commands.list(),
                crate::resource::TrackedResource::resource(buffer),
            );
        }
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
        // The march samples the depth buffer that was just replaced.
        self.shadow
            .sample_camera_depth(&self.context, self.depth.texture())?;

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

/// The renderer must be movable to the thread that records frames.
///
/// A compile check rather than a comment, and the reason three things in this crate are
/// what they are: `BufferSlice` holds an `Arc` and not an `Rc`, a resource's tracked state
/// is an atomic and not a `Cell`, and a mapped upload pointer is a [`crate::resource::MappedPtr`]
/// and not a bare `NonNull`. Any of the three reverting would break this line rather than
/// being discovered when the render thread refuses to compile.
///
/// **`Send`, deliberately not `Sync`.** One thread owns the renderer at a time. Sharing it
/// would need synchronization nothing here provides.
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<Renderer>();
};

/// Whether a skinned instance is drawn see-through: blended over the world in its own passes
/// rather than with the other characters. See `Renderer::draw_translucent_characters`.
fn is_translucent(instance: &engine_render_data::SkinnedInstance) -> bool {
    instance.style.is_faded()
}

/// Which of the camera's opaque lists a recording loop walks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CameraList {
    /// Everything the depth prepass draws.
    Visible,
    /// What the camera may dither this frame, which the prepass leaves out.
    Dithered,
}

/// Whether the camera pass may dither some of a rigid entity's pixels away this frame.
///
/// Conservative, against the shader's `apply_fade`: true for an entity with its own fade,
/// and for a see-through one whose bounds reach the camera fade's near zone (within
/// `near_end` of the eye) or its tube (within `radius` of the segment from the eye to the
/// target). Outside both the shader's opacity is exactly one and it never discards, so the
/// entity can be drawn into the depth prepass whole. An entity with no bounds is assumed to
/// be reachable.
fn may_be_dithered(
    style: engine_render_data::InstanceStyle,
    fade: Option<engine_render_data::CameraFade>,
    eye: engine_core::math::Vec3,
    bounds: Option<&engine_core::math::Sphere>,
) -> bool {
    if style.is_faded() {
        return true;
    }
    if !style.see_through {
        return false;
    }
    let Some(fade) = fade else {
        return false;
    };
    let Some(bounds) = bounds else {
        return true;
    };
    // A millimetre of slack against the shader's own float arithmetic.
    const SLACK: f32 = 1e-3;
    let to_center = bounds.center - eye;
    let near_end = fade.near_end.max(fade.near_start).max(0.0);
    if to_center.length() - bounds.radius < near_end + SLACK {
        return true;
    }
    let axis = fade.target - eye;
    let length_squared = axis.dot(axis);
    let along = if length_squared > 1e-12 {
        (to_center.dot(axis) / length_squared).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let closest = eye + axis * along;
    (bounds.center - closest).length() - bounds.radius < fade.radius.max(0.0) + SLACK
}

#[cfg(test)]
mod tests {
    use super::may_be_dithered;
    use engine_core::math::{Sphere, Vec3};
    use engine_render_data::{CameraFade, InstanceStyle};

    fn fade() -> CameraFade {
        CameraFade {
            target: Vec3::new(0.0, 1.5, 0.0),
            near_start: 0.2,
            near_end: 0.6,
            radius: 1.2,
            min_opacity: 0.25,
        }
    }

    const EYE: Vec3 = Vec3::new(0.0, 3.0, 8.0);

    fn at(center: Vec3, radius: f32) -> Sphere {
        Sphere::new(center, radius)
    }

    #[test]
    fn only_what_the_shader_could_dither_leaves_the_prepass() {
        let see_through = InstanceStyle::NEUTRAL.see_through();
        let solid = InstanceStyle::NEUTRAL;
        let between = at(Vec3::new(0.3, 2.2, 4.0), 0.5);
        let beside = at(Vec3::new(9.0, 1.0, 0.0), 0.5);

        // A see-through prop between the eye and the character, and one at the eye.
        assert!(may_be_dithered(
            see_through,
            Some(fade()),
            EYE,
            Some(&between)
        ));
        assert!(may_be_dithered(
            see_through,
            Some(fade()),
            EYE,
            Some(&at(EYE, 0.1))
        ));
        // Far to one side, or with no fade this frame, it is drawn whole.
        assert!(!may_be_dithered(
            see_through,
            Some(fade()),
            EYE,
            Some(&beside)
        ));
        assert!(!may_be_dithered(see_through, None, EYE, Some(&between)));
        // Solid geometry is never faded by the camera, wherever it is.
        assert!(!may_be_dithered(solid, Some(fade()), EYE, Some(&between)));
        // An entity's own fade always dithers it.
        let faded = InstanceStyle::NEUTRAL.with_opacity(0.5);
        assert!(may_be_dithered(faded, None, EYE, Some(&beside)));
    }

    #[test]
    fn the_test_is_conservative_against_the_shaders_tube() {
        // Sample points along and around the eye-to-target segment that the shader's tube
        // fades; a sphere of any size containing one of them must be classified dithered.
        let style = InstanceStyle::NEUTRAL.see_through();
        let fade = fade();
        let axis = fade.target - EYE;
        for step in 0..=10 {
            let along = EYE + axis * (step as f32 / 10.0);
            for offset in [
                Vec3::ZERO,
                Vec3::new(1.1, 0.0, 0.0),
                Vec3::new(0.0, -1.1, 0.0),
            ] {
                let point = along + offset;
                assert!(
                    may_be_dithered(style, Some(fade), EYE, Some(&at(point, 0.01))),
                    "a point {offset:?} off the line at step {step} is inside the tube"
                );
            }
        }
    }
}
