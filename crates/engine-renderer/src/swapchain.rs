//! Flip-model swap chain, its back buffers and their render-target views.

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_CPU_DESCRIPTOR_HANDLE, D3D12_DESCRIPTOR_HEAP_TYPE_RTV, D3D12_RESOURCE_STATE_PRESENT,
    ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_UNSPECIFIED, DXGI_FORMAT, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    DXGI_MWA_NO_ALT_ENTER, DXGI_PRESENT, DXGI_PRESENT_ALLOW_TEARING, DXGI_SCALING_NONE,
    DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING,
    DXGI_SWAP_EFFECT_FLIP_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGISwapChain3,
};

use windows::core::Interface;

use crate::descriptor::{CpuDescriptorHeap, DescriptorHandle};
use crate::device::RenderDevice;
use crate::error::RendererError;
use crate::resource::GpuTexture;

/// Number of back buffers, and therefore of frames the CPU may run ahead.
///
/// Three keeps the CPU from stalling on the GPU at high frame rates while costing one
/// extra buffer of memory; two would serialize more often at 240+ FPS.
pub const FRAME_COUNT: usize = 3;

/// Back-buffer format. 8-bit UNORM is enough for Phase 0; HDR is out of scope.
pub const BACK_BUFFER_FORMAT: DXGI_FORMAT = DXGI_FORMAT_R8G8B8A8_UNORM;

/// The swap chain, its back buffers and the RTV heap describing them.
pub struct SwapChain {
    swap_chain: IDXGISwapChain3,
    /// Kept alive because the views live in it. Never read after creation — the handles
    /// in `rtvs` address it, and dropping it would invalidate every one of them.
    #[expect(
        dead_code,
        reason = "ownership only: the heap must outlive the handles that point into it"
    )]
    rtv_heap: CpuDescriptorHeap,
    /// One render-target view per back buffer, allocated once.
    rtvs: Vec<DescriptorHandle>,
    buffers: Vec<GpuTexture>,
    width: u32,
    height: u32,
    allow_tearing: bool,
    vsync: bool,
}

impl SwapChain {
    /// Creates a flip-discard swap chain for `hwnd` at the given size.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if any DXGI or D3D12 call fails.
    pub fn new(
        context: &RenderDevice,
        hwnd: HWND,
        width: u32,
        height: u32,
        vsync: bool,
    ) -> Result<Self, RendererError> {
        let allow_tearing = context.allow_tearing();
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: width,
            Height: height,
            Format: BACK_BUFFER_FORMAT,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: FRAME_COUNT as u32,
            Scaling: DXGI_SCALING_NONE,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_UNSPECIFIED,
            Flags: swap_chain_flags(allow_tearing).0 as u32,
        };

        // SAFETY: FFI call. The queue and factory are live, `hwnd` belongs to a window
        // owned by the caller, and `desc` outlives the call.
        let swap_chain1 = unsafe {
            context
                .factory()
                .CreateSwapChainForHwnd(context.queue(), hwnd, &desc, None, None)
        }
        .map_err(|error| RendererError::api("IDXGIFactory6::CreateSwapChainForHwnd", error))?;

        // Alt+Enter fullscreen is handled by the engine later, not by DXGI's default
        // behaviour, which would resize the swap chain behind our back.
        // SAFETY: FFI call on the live factory with the window it just used.
        if let Err(error) = unsafe {
            context
                .factory()
                .MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER)
        } {
            log::debug!("MakeWindowAssociation failed: {error}");
        }

        let swap_chain: IDXGISwapChain3 = swap_chain1
            .cast()
            .map_err(|error| RendererError::api("IDXGISwapChain1::cast<IDXGISwapChain3>", error))?;

        // One RTV per back buffer, from the engine descriptor allocator rather than a
        // heap this module manages itself.
        let mut rtv_heap =
            CpuDescriptorHeap::new(context, D3D12_DESCRIPTOR_HEAP_TYPE_RTV, FRAME_COUNT as u32)?;
        let mut rtvs = Vec::with_capacity(FRAME_COUNT);
        for _ in 0..FRAME_COUNT {
            rtvs.push(rtv_heap.allocate()?);
        }

        let mut this = Self {
            swap_chain,
            rtv_heap,
            rtvs,
            buffers: Vec::with_capacity(FRAME_COUNT),
            width,
            height,
            allow_tearing,
            vsync,
        };
        this.create_render_targets(context)?;

        log::info!(
            "swap chain: {width}x{height}, {FRAME_COUNT} buffers, flip-discard, \
             tearing={allow_tearing}, vsync={vsync}"
        );

        Ok(this)
    }

    /// Index of the back buffer the next frame must render into.
    #[must_use]
    pub fn current_back_buffer_index(&self) -> usize {
        // SAFETY: FFI call on a live swap chain; the value is always < FRAME_COUNT.
        (unsafe { self.swap_chain.GetCurrentBackBufferIndex() }) as usize
    }

    /// The back buffer at `index`, with its tracked resource state.
    ///
    /// A back buffer is in `PRESENT` when DXGI hands it over and must be back in `PRESENT`
    /// before it is presented again, so its state is tracked like any other resource
    /// rather than assumed by the render path.
    #[must_use]
    pub fn buffer(&self, index: usize) -> &GpuTexture {
        &self.buffers[index]
    }

    /// CPU descriptor handle of the render-target view at `index`.
    #[must_use]
    pub fn rtv_handle(&self, index: usize) -> D3D12_CPU_DESCRIPTOR_HANDLE {
        self.rtvs[index].cpu
    }

    /// Current back-buffer size in pixels.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Whether presentation waits for vertical blank.
    #[must_use]
    pub fn vsync(&self) -> bool {
        self.vsync
    }

    /// Presents the current back buffer.
    ///
    /// With vsync off and tearing supported, `Present(0, ALLOW_TEARING)` returns without
    /// waiting for the display, which is what allows frame rates above the refresh rate
    /// to be measured at all.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if `Present` reports a failing `HRESULT`.
    pub fn present(&self) -> Result<(), RendererError> {
        let (interval, flags) = if self.vsync {
            (1, DXGI_PRESENT(0))
        } else if self.allow_tearing {
            (0, DXGI_PRESENT_ALLOW_TEARING)
        } else {
            (0, DXGI_PRESENT(0))
        };
        // SAFETY: FFI call on a live swap chain. Present1 with no dirty rects or scroll
        // parameters behaves exactly like Present.
        unsafe { self.swap_chain.Present(interval, flags) }
            .ok()
            .map_err(|error| RendererError::api("IDXGISwapChain3::Present", error))
    }

    /// Resizes the back buffers.
    ///
    /// The caller **must** have waited for the GPU to go idle first: `ResizeBuffers`
    /// fails while any back buffer is still referenced by in-flight command lists.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if `ResizeBuffers` or view recreation fails.
    pub fn resize(
        &mut self,
        context: &RenderDevice,
        width: u32,
        height: u32,
    ) -> Result<(), RendererError> {
        if (width, height) == (self.width, self.height) {
            return Ok(());
        }
        if width == 0 || height == 0 {
            return Err(RendererError::InvalidConfig {
                reason: format!("swap chain size {width}x{height} has a zero dimension"),
            });
        }

        // Every reference to a back buffer must be released before ResizeBuffers.
        self.buffers.clear();

        // SAFETY: FFI call on a live swap chain whose buffers are no longer referenced by
        // this process and are not in use by the GPU (the caller waited for idle).
        unsafe {
            self.swap_chain.ResizeBuffers(
                FRAME_COUNT as u32,
                width,
                height,
                BACK_BUFFER_FORMAT,
                DXGI_SWAP_CHAIN_FLAG(swap_chain_flags(self.allow_tearing).0),
            )
        }
        .map_err(|error| RendererError::api("IDXGISwapChain3::ResizeBuffers", error))?;

        self.width = width;
        self.height = height;
        self.create_render_targets(context)?;
        log::debug!("swap chain resized to {width}x{height}");
        Ok(())
    }

    /// Fetches the back buffers and writes one RTV per buffer into the heap.
    fn create_render_targets(&mut self, context: &RenderDevice) -> Result<(), RendererError> {
        self.buffers.clear();
        for index in 0..FRAME_COUNT {
            // SAFETY: FFI call. `index` is below the buffer count the swap chain was
            // created with, so the call cannot address a non-existent buffer.
            let buffer: ID3D12Resource = unsafe { self.swap_chain.GetBuffer(index as u32) }
                .map_err(|error| RendererError::api("IDXGISwapChain3::GetBuffer", error))?;
            context.create_render_target_view(&buffer, self.rtv_handle(index));

            // DXGI hands over a back buffer in PRESENT, which is also the state it must be
            // in to be presented again. Adopting it with that state is what lets the
            // render path transition it by destination alone.
            self.buffers.push(GpuTexture::adopt(
                buffer,
                self.width,
                self.height,
                BACK_BUFFER_FORMAT,
                D3D12_RESOURCE_STATE_PRESENT,
            ));
        }
        Ok(())
    }
}

/// Swap-chain creation flags for the current tearing capability.
fn swap_chain_flags(allow_tearing: bool) -> DXGI_SWAP_CHAIN_FLAG {
    if allow_tearing {
        DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING
    } else {
        DXGI_SWAP_CHAIN_FLAG(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tearing_capability_controls_the_creation_flag() {
        assert_eq!(swap_chain_flags(true), DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING);
        assert_eq!(swap_chain_flags(false).0, 0);
    }

    #[test]
    fn frame_count_allows_the_cpu_to_run_ahead() {
        const { assert!(FRAME_COUNT >= 2, "flip model requires at least two buffers") };
    }
}
