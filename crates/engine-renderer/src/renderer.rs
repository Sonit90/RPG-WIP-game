//! Renderer front end: owns the device, swap chain, per-frame command recording and
//! the fence that keeps the CPU from overtaking the GPU.
//!
//! This type knows nothing about gameplay. Milestone 2 can only clear the back buffer;
//! geometry arrives in Milestone 4 through a `RenderWorld`-shaped input.

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_RESOURCE_STATE_DEPTH_WRITE, D3D12_RESOURCE_STATE_PRESENT,
    D3D12_RESOURCE_STATE_RENDER_TARGET, ID3D12CommandList,
};

use windows::core::Interface;

use crate::command::{CommandContext, CommandStats};
use crate::culling::{CullStats, Culler};
use crate::depth::{DEPTH_CLEAR, DepthBuffer};
use crate::device::{BACKEND, RenderDevice, feature_level_name};
use crate::error::RendererError;
use crate::frame::{FrameRing, SyncStats};
use crate::gpu_timer::GpuTimer;
use crate::material::{Material, MaterialDesc};
use crate::mesh::{GpuMesh, MeshData, VertexStream};
use crate::mesh_pipeline::MeshPipeline;
#[cfg(feature = "debug-overlay")]
use crate::overlay::{OverlayBuffers, OverlayGeometry};
use crate::pipeline::{ScenePipeline, Vertex};
use crate::render_list::RenderList;
use crate::resource::DeferredReleaseQueue;
use crate::scene::{
    FrameConstants, ObjectConstants, ROOT_FRAME_CBV, ROOT_MATERIAL_CBV, ROOT_OBJECT_CBV,
};
use crate::stress;
use crate::swapchain::SwapChain;
use crate::upload::UploadRing;
use crate::{AdapterInfo, RendererConfig};
use engine_core::math::Frustum;
use engine_core::time::{FrameStats, FrameStatsWindow};
use engine_render_data::Camera;
use engine_render_data::{Lighting, MaterialHandle, MeshHandle, RenderWorld};

/// Frames of `Present` history kept for its distribution.
///
/// The same window the engine's frame metrics use, so the two describe the same span of
/// time and a present percentile can be read against a frame percentile.
const PRESENT_WINDOW: usize = engine_core::metrics::DEFAULT_HISTORY;

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
    /// Vertices drawn by the most recent frame.
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
    /// Meshes created through [`Renderer::create_mesh`], indexed by handle.
    meshes: Vec<GpuMesh>,
    /// Materials created through [`Renderer::create_material`], indexed by handle.
    materials: Vec<Material>,
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
        let mut scene_scratch = Vec::new();
        stress::build_grid(1, &mut scene_scratch);
        let scene = VertexStream::upload(&context, &scene_scratch)?;

        #[cfg(feature = "debug-overlay")]
        let overlay = OverlayState {
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
            meshes: Vec::new(),
            materials: Vec::new(),
            culler: Culler::new(),
            visible: RenderList::new(),
            culling_enabled: true,
            cull_view_projection: None,
            last_cull_ms: 0.0,
            cull_scratch: Vec::new(),
            last_present_ms: 0.0,
            last_fence_signal_ms: 0.0,
            present_window: FrameStatsWindow::new(PRESENT_WINDOW),
            scene,
            scene_scratch,
            #[cfg(feature = "debug-overlay")]
            overlay,
            frames_presented: 0,
            draw_calls_last_frame: 0,
            triangles_last_frame: 0,
            uploads,
            pending_release: DeferredReleaseQueue::new(),
            recorded_frame_slot: None,
            recorded_back_buffer: 0,
            context,
        };
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
        let frame_constants =
            FrameConstants::new(camera.view_projection(), camera.position(), lighting);
        let allocation = self.uploads.write_constants(slot, &frame_constants)?;
        self.commands
            .set_root_cbv(ROOT_FRAME_CBV, allocation.gpu_address());

        // Three dense arrays read in step. Every field this loop needs was gathered while
        // culling, so nothing here reaches back into the world.
        for index in 0..self.visible.len() {
            let mesh_handle = self.visible.meshes()[index];
            let material_handle = self.visible.materials()[index];
            let Some(mesh) = self.meshes.get(mesh_handle.index()) else {
                return Err(RendererError::InvalidConfig {
                    reason: format!(
                        "render list entry {index} names mesh {} which does not exist",
                        mesh_handle.index()
                    ),
                });
            };
            let Some(material) = self.materials.get(material_handle.index()) else {
                return Err(RendererError::InvalidConfig {
                    reason: format!(
                        "render list entry {index} names material {} which does not exist",
                        material_handle.index()
                    ),
                });
            };

            let object = ObjectConstants::new(self.visible.transforms()[index].matrix());
            let allocation = self.uploads.write_constants(slot, &object)?;

            self.commands
                .set_root_cbv(ROOT_MATERIAL_CBV, material.gpu_address());
            self.commands
                .set_root_cbv(ROOT_OBJECT_CBV, allocation.gpu_address());
            self.commands.set_vertex_buffer(0, *mesh.vertex_view());
            self.commands.set_index_buffer(*mesh.index_view());
            self.commands.draw_indexed(mesh.index_count(), 1, 0);
        }

        self.finish_recording(slot, back_buffer)
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

    /// Uploads a material and returns a handle to it.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] for out-of-range parameters, or
    /// [`RendererError::Api`] if a D3D12 call fails.
    pub fn create_material(&mut self, desc: MaterialDesc) -> Result<MaterialHandle, RendererError> {
        let material = Material::new(&self.context, desc)?;
        let index = self.materials.len() as u32;
        self.materials.push(material);
        Ok(MaterialHandle::new(index, 1))
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
        let buffer = self.swap_chain.buffer(back_buffer);
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
