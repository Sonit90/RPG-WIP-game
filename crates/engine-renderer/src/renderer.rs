//! Renderer front end: owns the device, swap chain, per-frame command recording and
//! the fence that keeps the CPU from overtaking the GPU.
//!
//! This type knows nothing about gameplay. Milestone 2 can only clear the back buffer;
//! geometry arrives in Milestone 4 through a `RenderWorld`-shaped input.

use std::mem::ManuallyDrop;

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_MAX_DEPTH, D3D12_MIN_DEPTH, D3D12_RESOURCE_BARRIER, D3D12_RESOURCE_BARRIER_0,
    D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES, D3D12_RESOURCE_BARRIER_FLAG_NONE,
    D3D12_RESOURCE_BARRIER_TYPE_TRANSITION, D3D12_RESOURCE_STATE_PRESENT,
    D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATES, D3D12_RESOURCE_TRANSITION_BARRIER,
    D3D12_VIEWPORT, ID3D12CommandList, ID3D12GraphicsCommandList, ID3D12Resource,
};

use windows::core::Interface;

use crate::device::{BACKEND, RenderDevice, feature_level_name};
use crate::error::RendererError;
use crate::frame::{FrameRing, SyncStats};
use crate::gpu_timer::GpuTimer;
use crate::mesh::GpuMesh;
#[cfg(feature = "debug-overlay")]
use crate::overlay::{OverlayBuffers, OverlayGeometry};
use crate::pipeline::{ScenePipeline, Vertex};
use crate::stress;
use crate::swapchain::SwapChain;
use crate::{AdapterInfo, RendererConfig};

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
    command_list: ID3D12GraphicsCommandList,
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
    /// The geometry currently being drawn.
    scene: GpuMesh,
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
        let command_list = context.create_command_list(frames.allocator(0))?;

        let execute_lists = [Some(command_list.cast::<ID3D12CommandList>().map_err(
            |error| RendererError::api("ID3D12GraphicsCommandList::cast<ID3D12CommandList>", error),
        )?)];

        let gpu_timer = GpuTimer::new(context.device(), context.queue(), frames_in_flight)?;
        let pipeline = ScenePipeline::new(context.device())?;
        let mut scene_scratch = Vec::new();
        stress::build_grid(1, &mut scene_scratch);
        let scene = GpuMesh::upload(&context, &scene_scratch)?;

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
            command_list,
            execute_lists,
            gpu_timer,
            last_gpu_frame_ms: None,
            pipeline,
            scene,
            scene_scratch,
            #[cfg(feature = "debug-overlay")]
            overlay,
            frames_presented: 0,
            draw_calls_last_frame: 0,
            triangles_last_frame: 0,
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
        self.scene = GpuMesh::upload(&self.context, &self.scene_scratch)?;

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

        // The back buffer to draw into is the swap chain's business, and is deliberately
        // not assumed to equal the frame slot.
        let back_buffer = self.swap_chain.current_back_buffer_index();

        // SAFETY: FFI call. The list is closed at this point (either from `new` or from
        // the previous frame), which is what Reset requires, and the allocator was just
        // reset by the ring after proving the GPU was done with it.
        unsafe { self.command_list.Reset(self.frames.allocator(slot), None) }
            .map_err(|error| RendererError::api("ID3D12GraphicsCommandList::Reset", error))?;

        self.gpu_timer.begin_frame(&self.command_list, slot);

        let buffer = self.swap_chain.buffer(back_buffer);
        let rtv = self.swap_chain.rtv_handle(back_buffer);
        let index = slot;

        let to_render_target = transition_barrier(
            buffer,
            D3D12_RESOURCE_STATE_PRESENT,
            D3D12_RESOURCE_STATE_RENDER_TARGET,
        );
        let (width, height) = self.swap_chain.size();
        let viewport = viewport_for(width, height);
        let scissor = scissor_for(width, height);

        // SAFETY: FFI calls recording into the open list. Every referenced object — the
        // barrier array, viewport, scissor, RTV handle, pipeline objects and vertex
        // buffer view — is owned by this renderer or lives on the stack for the duration
        // of the call.
        unsafe {
            self.command_list.ResourceBarrier(&[to_render_target]);
            self.command_list
                .OMSetRenderTargets(1, Some(&rtv), false, None);
            self.command_list.ClearRenderTargetView(rtv, &color, None);

            self.command_list.RSSetViewports(&[viewport]);
            self.command_list.RSSetScissorRects(&[scissor]);
            self.command_list
                .SetGraphicsRootSignature(self.pipeline.root_signature());
            self.command_list
                .SetPipelineState(self.pipeline.pipeline_state());
            self.command_list
                .IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            self.command_list
                .IASetVertexBuffers(0, Some(&[*self.scene.view()]));
            self.command_list
                .DrawInstanced(self.scene.vertex_count(), 1, 0, 0);
        }

        self.draw_calls_last_frame = 1;
        self.triangles_last_frame = u64::from(self.scene.triangle_count());

        // The overlay reuses the root signature, pipeline state and topology bound above;
        // only the vertex buffer changes. It is drawn last so it sits over the scene.
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
                    // SAFETY: FFI calls on the open list. The view refers to this slot's
                    // upload buffer, which lives as long as the renderer.
                    unsafe {
                        self.command_list.IASetVertexBuffers(0, Some(&[view]));
                        self.command_list.DrawInstanced(count, 1, 0, 0);
                    }
                    self.overlay.drawn_vertices = count;
                    self.draw_calls_last_frame += 1;
                    self.triangles_last_frame += u64::from(count) / 3;
                }
            }
        }

        let to_present = transition_barrier(
            buffer,
            D3D12_RESOURCE_STATE_RENDER_TARGET,
            D3D12_RESOURCE_STATE_PRESENT,
        );
        // SAFETY: as above; the list is still open and owned by this renderer.
        unsafe {
            self.command_list.ResourceBarrier(&[to_present]);
        }

        // The closing timestamp brackets the frame's own work; the resolve that follows it
        // is a GPU-side copy into the readback buffer.
        self.gpu_timer.end_frame(&self.command_list, slot);

        // SAFETY: FFI call closing the list recorded above.
        unsafe { self.command_list.Close() }
            .map_err(|error| RendererError::api("ID3D12GraphicsCommandList::Close", error))?;

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

        self.swap_chain.present()?;
        // Signalled after Present, so the slot's fence value covers the whole submission;
        // this also advances the ring to the next slot.
        self.frames.end_frame(self.context.queue(), slot)?;
        self.frames_presented += 1;
        Ok(())
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
        let allocator = self.frames.allocator(0);
        // SAFETY: FFI call. The GPU is idle, which is exactly Reset's precondition.
        unsafe { allocator.Reset() }
            .map_err(|error| RendererError::api("ID3D12CommandAllocator::Reset", error))?;
        // SAFETY: FFI call. The list is closed here (every path either closes it or has
        // never opened it), which is what Reset requires.
        unsafe { self.command_list.Reset(allocator, None) }
            .map_err(|error| RendererError::api("ID3D12GraphicsCommandList::Reset", error))?;
        // SAFETY: FFI call closing the list that was just reset and holds no commands.
        unsafe { self.command_list.Close() }
            .map_err(|error| RendererError::api("ID3D12GraphicsCommandList::Close", error))
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

/// Viewport covering the whole back buffer.
///
/// Pure, so the mapping from swap-chain size to viewport is testable without a GPU.
#[must_use]
pub fn viewport_for(width: u32, height: u32) -> D3D12_VIEWPORT {
    D3D12_VIEWPORT {
        TopLeftX: 0.0,
        TopLeftY: 0.0,
        Width: width as f32,
        Height: height as f32,
        MinDepth: D3D12_MIN_DEPTH,
        MaxDepth: D3D12_MAX_DEPTH,
    }
}

/// Scissor rectangle covering the whole back buffer.
#[must_use]
pub fn scissor_for(width: u32, height: u32) -> RECT {
    RECT {
        left: 0,
        top: 0,
        right: width as i32,
        bottom: height as i32,
    }
}

/// Builds a resource transition barrier.
///
/// Shared with [`crate::offscreen`], which records the same kind of transition.
///
/// The `pResource` field is a non-owning copy of the interface pointer: `ManuallyDrop`
/// keeps the barrier from releasing a reference it never took, and the resource is kept
/// alive by the swap chain for the whole recording.
pub(crate) fn transition_barrier(
    resource: &ID3D12Resource,
    before: D3D12_RESOURCE_STATES,
    after: D3D12_RESOURCE_STATES,
) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                // SAFETY: copies the raw interface pointer without changing its refcount.
                // The copy is wrapped in ManuallyDrop, so it is never released, and the
                // real reference is owned by the swap chain for longer than this barrier.
                pResource: unsafe { std::mem::transmute_copy(resource) },
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: before,
                StateAfter: after,
            }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_covers_the_whole_back_buffer() {
        let viewport = viewport_for(1920, 1080);
        assert_eq!((viewport.TopLeftX, viewport.TopLeftY), (0.0, 0.0));
        assert_eq!((viewport.Width, viewport.Height), (1920.0, 1080.0));
        assert_eq!(viewport.MinDepth, D3D12_MIN_DEPTH);
        assert_eq!(viewport.MaxDepth, D3D12_MAX_DEPTH);
    }

    #[test]
    fn scissor_covers_the_whole_back_buffer() {
        let scissor = scissor_for(1280, 720);
        assert_eq!((scissor.left, scissor.top), (0, 0));
        assert_eq!((scissor.right, scissor.bottom), (1280, 720));
    }

    #[test]
    fn viewport_and_scissor_agree_after_a_resize() {
        for (width, height) in [(640, 480), (1920, 1080), (3840, 2160)] {
            let viewport = viewport_for(width, height);
            let scissor = scissor_for(width, height);
            assert_eq!(viewport.Width as i32, scissor.right);
            assert_eq!(viewport.Height as i32, scissor.bottom);
        }
    }
}
