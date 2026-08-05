//! Renderer front end: owns the device, swap chain, per-frame command recording and
//! the fence that keeps the CPU from overtaking the GPU.
//!
//! This type knows nothing about gameplay. Milestone 2 can only clear the back buffer;
//! geometry arrives in Milestone 4 through a `RenderWorld`-shaped input.

use std::mem::ManuallyDrop;

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_RESOURCE_BARRIER, D3D12_RESOURCE_BARRIER_0,
    D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES, D3D12_RESOURCE_BARRIER_FLAG_NONE,
    D3D12_RESOURCE_BARRIER_TYPE_TRANSITION, D3D12_RESOURCE_STATE_PRESENT,
    D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATES, D3D12_RESOURCE_TRANSITION_BARRIER,
    ID3D12CommandAllocator, ID3D12CommandList, ID3D12GraphicsCommandList, ID3D12Resource,
};

use windows::core::Interface;

use crate::device::{BACKEND, GpuContext, feature_level_name};
use crate::error::RendererError;
use crate::swapchain::{FRAME_COUNT, SwapChain};
use crate::sync::GpuFence;
use crate::{AdapterInfo, RendererConfig};

/// Per-frame-slot resources.
struct Frame {
    allocator: ID3D12CommandAllocator,
    /// Fence value signalled after this slot's work was submitted. The slot may not be
    /// reused until the GPU has passed it.
    fence_value: u64,
}

/// D3D12 renderer.
pub struct Renderer {
    context: GpuContext,
    swap_chain: SwapChain,
    frames: Vec<Frame>,
    command_list: ID3D12GraphicsCommandList,
    /// Pre-built argument for `ExecuteCommandLists`, so submitting a frame performs no
    /// allocation and no `QueryInterface`.
    execute_lists: [Option<ID3D12CommandList>; 1],
    fence: GpuFence,
    frames_presented: u64,
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

        let context = GpuContext::new(config.enable_debug_layer)?;
        let hwnd = HWND(window_handle as *mut std::ffi::c_void);
        let swap_chain = SwapChain::new(&context, hwnd, config.width, config.height, config.vsync)?;

        let mut frames = Vec::with_capacity(FRAME_COUNT);
        for _ in 0..FRAME_COUNT {
            // SAFETY: FFI call on a live device with a valid command list type.
            let allocator: ID3D12CommandAllocator = unsafe {
                context
                    .device()
                    .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)
            }
            .map_err(|error| RendererError::api("ID3D12Device::CreateCommandAllocator", error))?;
            frames.push(Frame {
                allocator,
                fence_value: 0,
            });
        }

        // SAFETY: FFI call. The allocator is live and owned by `frames`; passing `None`
        // for the initial pipeline state is valid for a list that only clears.
        let command_list: ID3D12GraphicsCommandList = unsafe {
            context.device().CreateCommandList(
                0,
                D3D12_COMMAND_LIST_TYPE_DIRECT,
                &frames[0].allocator,
                None,
            )
        }
        .map_err(|error| RendererError::api("ID3D12Device::CreateCommandList", error))?;

        // A freshly created list is open; close it so the per-frame path can always
        // start with Reset.
        // SAFETY: FFI call on the list created immediately above, which records nothing.
        unsafe { command_list.Close() }
            .map_err(|error| RendererError::api("ID3D12GraphicsCommandList::Close", error))?;

        let execute_lists = [Some(command_list.cast::<ID3D12CommandList>().map_err(
            |error| RendererError::api("ID3D12GraphicsCommandList::cast<ID3D12CommandList>", error),
        )?)];

        let fence = GpuFence::new(context.device())?;

        let renderer = Self {
            context,
            swap_chain,
            frames,
            command_list,
            execute_lists,
            fence,
            frames_presented: 0,
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

    /// Name of the highest supported feature level, e.g. `12_1`.
    #[must_use]
    pub fn feature_level_name(&self) -> &'static str {
        feature_level_name(self.context.feature_level())
    }

    /// Records and submits a frame that clears the back buffer, then presents it.
    ///
    /// Performs no heap allocation: the command list, its allocators and the submission
    /// argument are all created once in [`Renderer::new`].
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if recording, submission or presentation fails.
    pub fn render_clear(&mut self, color: [f32; 4]) -> Result<(), RendererError> {
        let index = self.swap_chain.current_back_buffer_index();

        // Frame pacing: this slot's previous submission must have completed before its
        // allocator can be reset. Only blocks when the CPU is a full lap ahead.
        self.fence.wait(self.frames[index].fence_value)?;

        let allocator = &self.frames[index].allocator;
        // SAFETY: FFI call. The GPU is done with this allocator, as enforced by the wait
        // above, which is exactly the precondition Reset requires.
        unsafe { allocator.Reset() }
            .map_err(|error| RendererError::api("ID3D12CommandAllocator::Reset", error))?;
        // SAFETY: FFI call. The list is closed at this point (either from `new` or from
        // the previous frame), which is what Reset requires.
        unsafe { self.command_list.Reset(allocator, None) }
            .map_err(|error| RendererError::api("ID3D12GraphicsCommandList::Reset", error))?;

        let buffer = self.swap_chain.buffer(index);
        let rtv = self.swap_chain.rtv_handle(index);

        let to_render_target = transition_barrier(
            buffer,
            D3D12_RESOURCE_STATE_PRESENT,
            D3D12_RESOURCE_STATE_RENDER_TARGET,
        );
        // SAFETY: FFI calls recording into the open list. The barrier array lives on the
        // stack for the duration of the call, and `rtv` points into our own RTV heap.
        unsafe {
            self.command_list.ResourceBarrier(&[to_render_target]);
            self.command_list
                .OMSetRenderTargets(1, Some(&rtv), false, None);
            self.command_list.ClearRenderTargetView(rtv, &color, None);
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

        self.swap_chain.present()?;

        self.frames[index].fence_value = self.fence.signal(self.context.queue())?;
        self.frames_presented += 1;
        Ok(())
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
        self.swap_chain.resize(&self.context, width, height)?;

        // After ResizeBuffers every slot is free again; forget the old fence values so
        // the next frames do not wait on submissions that referenced destroyed buffers.
        let completed = self.fence.completed_value();
        for frame in &mut self.frames {
            frame.fence_value = completed;
        }
        Ok(())
    }

    /// Blocks until the GPU has finished all submitted work.
    ///
    /// Fully serializing — for shutdown and resize only, never per frame.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if the fence signal or wait fails.
    pub fn wait_for_idle(&mut self) -> Result<(), RendererError> {
        self.fence.wait_for_idle(self.context.queue())
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
        log::info!("renderer shut down after {} frames", self.frames_presented);
    }
}

/// Builds a resource transition barrier.
///
/// The `pResource` field is a non-owning copy of the interface pointer: `ManuallyDrop`
/// keeps the barrier from releasing a reference it never took, and the resource is kept
/// alive by the swap chain for the whole recording.
fn transition_barrier(
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
