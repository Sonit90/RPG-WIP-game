//! The device layer: DXGI factory, adapter selection, the D3D12 device, the direct queue,
//! and the creation of every D3D12 object the renderer owns.
//!
//! Everything that calls a `ID3D12Device::Create*` method lives here. That is the point of
//! the layer: object creation is where `unsafe`, `HRESULT` mapping and the parameters that
//! must agree across objects (command list type, descriptor heap flags) all meet, and
//! spreading it across the renderer means auditing it in a dozen places instead of one.
//!
//! Nothing above `engine-renderer` sees a D3D12 interface. `engine-core` has no graphics
//! dependency at all; the application holds a [`crate::Renderer`], not a device.

use windows::Win32::Graphics::Direct3D::{
    D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_12_0,
    D3D_FEATURE_LEVEL_12_1, D3D_FEATURE_LEVEL_12_2,
};
use windows::Win32::Graphics::Direct3D12::{
    D3D12_CLEAR_VALUE, D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_COMMAND_QUEUE_DESC,
    D3D12_COMMAND_QUEUE_FLAG_NONE, D3D12_CPU_DESCRIPTOR_HANDLE, D3D12_DESCRIPTOR_HEAP_DESC,
    D3D12_DESCRIPTOR_HEAP_FLAG_NONE, D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
    D3D12_DESCRIPTOR_HEAP_TYPE, D3D12_FEATURE_DATA_FEATURE_LEVELS, D3D12_FEATURE_FEATURE_LEVELS,
    D3D12_FENCE_FLAG_NONE, D3D12_HEAP_FLAG_NONE, D3D12_HEAP_PROPERTIES, D3D12_HEAP_TYPE,
    D3D12_HEAP_TYPE_DEFAULT, D3D12_MESSAGE_SEVERITY_ERROR, D3D12_RESOURCE_DESC,
    D3D12_RESOURCE_DIMENSION_BUFFER, D3D12_RESOURCE_DIMENSION_TEXTURE2D, D3D12_RESOURCE_FLAGS,
    D3D12_RESOURCE_STATES, D3D12_TEXTURE_LAYOUT_ROW_MAJOR, D3D12_TEXTURE_LAYOUT_UNKNOWN,
    D3D12CreateDevice, D3D12GetDebugInterface, ID3D12CommandAllocator, ID3D12CommandQueue,
    ID3D12Debug, ID3D12DescriptorHeap, ID3D12Device, ID3D12Fence, ID3D12GraphicsCommandList,
    ID3D12InfoQueue, ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, DXGI_CREATE_FACTORY_DEBUG, DXGI_CREATE_FACTORY_FLAGS,
    DXGI_FEATURE_PRESENT_ALLOW_TEARING, DXGI_MEMORY_SEGMENT_GROUP_LOCAL,
    DXGI_MEMORY_SEGMENT_GROUP_NON_LOCAL, DXGI_QUERY_VIDEO_MEMORY_INFO, IDXGIAdapter3,
    IDXGIFactory6,
};

use windows::core::Interface;

use crate::adapter::{AdapterInfo, enumerate_adapters, select_adapter};
use crate::error::RendererError;

/// Backend name reported at startup.
pub const BACKEND: &str = "Direct3D 12";

/// Lowest feature level the engine will run on.
const MINIMUM_FEATURE_LEVEL: D3D_FEATURE_LEVEL = D3D_FEATURE_LEVEL_11_0;

/// Feature levels probed at startup, highest first.
const PROBED_FEATURE_LEVELS: [D3D_FEATURE_LEVEL; 5] = [
    D3D_FEATURE_LEVEL_12_2,
    D3D_FEATURE_LEVEL_12_1,
    D3D_FEATURE_LEVEL_12_0,
    D3D_FEATURE_LEVEL_11_1,
    D3D_FEATURE_LEVEL_11_0,
];

/// Device, queue and the DXGI objects they came from.
pub struct RenderDevice {
    factory: IDXGIFactory6,
    device: ID3D12Device,
    queue: ID3D12CommandQueue,
    adapter_info: AdapterInfo,
    /// The adapter, for asking what video memory the process is using. `None` on a system
    /// whose DXGI predates `IDXGIAdapter3`, where the question simply has no answer.
    adapter: Option<IDXGIAdapter3>,
    feature_level: D3D_FEATURE_LEVEL,
    allow_tearing: bool,
}

/// Video memory this process is using, as the OS's memory manager reports it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VideoMemory {
    /// Bytes resident in the adapter's own memory.
    pub local_usage: u64,
    /// How much of the adapter's memory the OS will currently let this process use.
    pub local_budget: u64,
    /// Bytes of system memory the adapter is using on this process's behalf.
    pub non_local_usage: u64,
}

impl RenderDevice {
    /// Creates the factory, selects an adapter, and creates the device and queue.
    ///
    /// `enable_debug_layer` turns on the D3D12 validation layer, which is expensive and
    /// intended for debug builds only. `break_on_error` additionally breaks into the
    /// debugger on a validation error; it has no effect without the layer.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::NoSuitableAdapter`] if no adapter can create a device at
    /// feature level 11_0, or [`RendererError::Api`] if a DXGI/D3D12 call fails.
    pub fn new(enable_debug_layer: bool, break_on_error: bool) -> Result<Self, RendererError> {
        let mut factory_flags = DXGI_CREATE_FACTORY_FLAGS(0);
        if enable_debug_layer && enable_d3d12_debug_layer() {
            factory_flags |= DXGI_CREATE_FACTORY_DEBUG;
        }

        #[cfg(feature = "startup-timing")]
        let started = std::time::Instant::now();
        // SAFETY: FFI call. The flags are valid and the returned interface is refcounted
        // by the `windows` crate.
        let factory: IDXGIFactory6 = unsafe { CreateDXGIFactory2(factory_flags) }
            .map_err(|error| RendererError::api("CreateDXGIFactory2", error))?;
        #[cfg(feature = "startup-timing")]
        let after_factory = std::time::Instant::now();

        let adapters = enumerate_adapters(&factory)?;
        #[cfg(feature = "startup-timing")]
        let after_adapters = std::time::Instant::now();
        let examined = adapters.len();
        let Some(index) = select_adapter(
            &adapters
                .iter()
                .map(|(_, info)| info.clone())
                .collect::<Vec<_>>(),
        ) else {
            return Err(RendererError::NoSuitableAdapter { examined });
        };

        for (position, (_, info)) in adapters.iter().enumerate() {
            log::debug!(
                "adapter {position}: {} ({} MiB dedicated, software={}){}",
                info.name,
                info.dedicated_video_memory_mib(),
                info.is_software,
                if position == index {
                    " <- selected"
                } else {
                    ""
                }
            );
        }

        let (adapter, adapter_info) = &adapters[index];

        let mut device: Option<ID3D12Device> = None;
        // SAFETY: FFI call. `adapter` is a live COM object and `device` is a valid
        // out-parameter that D3D12 fills on success.
        unsafe { D3D12CreateDevice(adapter, MINIMUM_FEATURE_LEVEL, &mut device) }
            .map_err(|error| RendererError::api("D3D12CreateDevice", error))?;
        let Some(device) = device else {
            return Err(RendererError::NoSuitableAdapter { examined });
        };

        if enable_debug_layer && break_on_error {
            configure_info_queue(&device);
        }

        #[cfg(feature = "startup-timing")]
        let after_device = std::time::Instant::now();
        let feature_level = highest_feature_level(&device);

        let queue_desc = D3D12_COMMAND_QUEUE_DESC {
            Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
            Flags: D3D12_COMMAND_QUEUE_FLAG_NONE,
            ..Default::default()
        };
        // SAFETY: FFI call with a fully initialized descriptor that outlives the call.
        let queue: ID3D12CommandQueue = unsafe { device.CreateCommandQueue(&queue_desc) }
            .map_err(|error| RendererError::api("ID3D12Device::CreateCommandQueue", error))?;

        let allow_tearing = query_tearing_support(&factory);
        #[cfg(feature = "startup-timing")]
        log::info!(
            "[startup] D3D12: factory {:.1} ms, {} adapter(s) enumerated {:.1} ms, \
             D3D12CreateDevice {:.1} ms, queue and caps {:.1} ms",
            (after_factory - started).as_secs_f64() * 1e3,
            adapters.len(),
            (after_adapters - after_factory).as_secs_f64() * 1e3,
            (after_device - after_adapters).as_secs_f64() * 1e3,
            after_device.elapsed().as_secs_f64() * 1e3,
        );

        Ok(Self {
            factory,
            device,
            queue,
            adapter_info: adapter_info.clone(),
            adapter: adapter.cast().ok(),
            feature_level,
            allow_tearing,
        })
    }

    /// Video memory in use, or `None` if the adapter cannot say.
    ///
    /// A kernel call: cheap, but meant for a report interval, not for every frame.
    #[must_use]
    pub fn video_memory(&self) -> Option<VideoMemory> {
        let adapter = self.adapter.as_ref()?;
        let mut local = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
        let mut non_local = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
        // SAFETY: FFI calls on a live adapter, each with a valid out-parameter that outlives
        // the call. Node 0 is the only node of a single-GPU adapter.
        unsafe {
            adapter
                .QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut local)
                .ok()?;
            adapter
                .QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_NON_LOCAL, &mut non_local)
                .ok()?;
        }
        Some(VideoMemory {
            local_usage: local.CurrentUsage,
            local_budget: local.Budget,
            non_local_usage: non_local.CurrentUsage,
        })
    }

    /// The DXGI factory the device came from.
    #[must_use]
    pub fn factory(&self) -> &IDXGIFactory6 {
        &self.factory
    }

    /// The D3D12 device.
    #[must_use]
    pub fn device(&self) -> &ID3D12Device {
        &self.device
    }

    /// The direct command queue all rendering work is submitted to.
    #[must_use]
    pub fn queue(&self) -> &ID3D12CommandQueue {
        &self.queue
    }

    /// Description of the selected adapter.
    #[must_use]
    pub fn adapter_info(&self) -> &AdapterInfo {
        &self.adapter_info
    }

    /// Highest feature level the device reports support for.
    #[must_use]
    pub fn feature_level(&self) -> D3D_FEATURE_LEVEL {
        self.feature_level
    }

    /// Whether the display path supports tearing (uncapped presentation).
    #[must_use]
    pub fn allow_tearing(&self) -> bool {
        self.allow_tearing
    }

    /// Creates a command allocator for the direct queue.
    ///
    /// One allocator per frame slot: an allocator's memory cannot be reset while the GPU
    /// is still executing commands recorded from it, so slots need their own.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if the device rejects the request.
    pub fn create_command_allocator(&self) -> Result<ID3D12CommandAllocator, RendererError> {
        // SAFETY: FFI call on a live device with a valid command list type constant.
        unsafe {
            self.device
                .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)
        }
        .map_err(|error| RendererError::api("ID3D12Device::CreateCommandAllocator", error))
    }

    /// Creates a direct command list, already closed.
    ///
    /// D3D12 hands back an *open* list, which every caller then has to remember to close
    /// before the first frame — the per-frame path always begins with `Reset`, which
    /// requires a closed list. Closing it here removes that trap from every call site.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if creation or the initial close fails.
    pub fn create_command_list(
        &self,
        allocator: &ID3D12CommandAllocator,
    ) -> Result<ID3D12GraphicsCommandList, RendererError> {
        // SAFETY: FFI call. The allocator is a live object owned by the caller, and
        // passing `None` for the initial pipeline state is valid — the recorded frame sets
        // its own pipeline before drawing.
        let list: ID3D12GraphicsCommandList = unsafe {
            self.device
                .CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, allocator, None)
        }
        .map_err(|error| RendererError::api("ID3D12Device::CreateCommandList", error))?;

        // SAFETY: FFI call on the list created immediately above, which has recorded
        // nothing.
        unsafe { list.Close() }
            .map_err(|error| RendererError::api("ID3D12GraphicsCommandList::Close", error))?;

        Ok(list)
    }

    /// Creates a descriptor heap.
    ///
    /// `shader_visible` must be false for RTV and DSV heaps — D3D12 rejects the
    /// combination — and true for the CBV/SRV/UAV heap the shaders sample from.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if the device rejects the descriptor.
    pub fn create_descriptor_heap(
        &self,
        kind: D3D12_DESCRIPTOR_HEAP_TYPE,
        capacity: u32,
        shader_visible: bool,
    ) -> Result<ID3D12DescriptorHeap, RendererError> {
        let desc = D3D12_DESCRIPTOR_HEAP_DESC {
            Type: kind,
            NumDescriptors: capacity,
            Flags: if shader_visible {
                D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE
            } else {
                D3D12_DESCRIPTOR_HEAP_FLAG_NONE
            },
            NodeMask: 0,
        };
        // SAFETY: FFI call with a fully initialized descriptor that outlives the call.
        unsafe { self.device.CreateDescriptorHeap(&desc) }
            .map_err(|error| RendererError::api("ID3D12Device::CreateDescriptorHeap", error))
    }

    /// Size in bytes of one descriptor of `kind` on this device.
    ///
    /// Hardware-specific, so it must be queried rather than assumed; it is the stride for
    /// stepping through a heap by hand.
    #[must_use]
    pub fn descriptor_size(&self, kind: D3D12_DESCRIPTOR_HEAP_TYPE) -> u32 {
        // SAFETY: FFI call on a live device with a valid heap type constant. It cannot
        // fail; the return value is a size.
        unsafe { self.device.GetDescriptorHandleIncrementSize(kind) }
    }

    /// Creates a committed buffer of `byte_size` bytes on `heap_type`.
    ///
    /// `DEFAULT` is device-local memory the GPU reads at full speed but the CPU cannot
    /// touch; `UPLOAD` is CPU-visible memory the GPU reads across the bus. Geometry
    /// belongs in the first, staging and per-frame data in the second — a vertex buffer
    /// left on an upload heap turns a benchmark into a measurement of bus bandwidth.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if the device rejects the request, or
    /// [`RendererError::InvalidConfig`] if it succeeds without producing a resource.
    pub fn create_buffer(
        &self,
        byte_size: usize,
        heap_type: D3D12_HEAP_TYPE,
        initial_state: D3D12_RESOURCE_STATES,
    ) -> Result<ID3D12Resource, RendererError> {
        let heap_properties = D3D12_HEAP_PROPERTIES {
            Type: heap_type,
            ..Default::default()
        };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Width: byte_size as u64,
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_UNKNOWN,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            ..Default::default()
        };

        let mut buffer: Option<ID3D12Resource> = None;
        // SAFETY: FFI call with fully initialized descriptors that outlive it, and a valid
        // out-parameter. The resource is refcounted by the `windows` crate from here on.
        unsafe {
            self.device.CreateCommittedResource(
                &heap_properties,
                D3D12_HEAP_FLAG_NONE,
                &desc,
                initial_state,
                None,
                &mut buffer,
            )
        }
        .map_err(|error| RendererError::api("ID3D12Device::CreateCommittedResource", error))?;

        buffer.ok_or_else(|| RendererError::InvalidConfig {
            reason: "buffer creation produced no resource".to_owned(),
        })
    }

    /// Creates a committed 2D texture.
    ///
    /// `clear_value` must be `Some` for a render target or depth-stencil texture and
    /// `None` otherwise: D3D12 warns when an optimized clear value is supplied for a
    /// resource that cannot be cleared, and clearing with a value other than the one the
    /// resource was created with is slower on some hardware.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if the device rejects the request, or
    /// [`RendererError::InvalidConfig`] if it succeeds without producing a resource.
    // A thin wrapper over `CreateCommittedResource`, so its parameters are D3D12's: size,
    // mip count, format, flags, state and an optional clear value. Grouping them into a
    // struct would be a second description of `D3D12_RESOURCE_DESC` to keep in step with the
    // first.
    #[allow(
        clippy::too_many_arguments,
        reason = "these are D3D12's own parameters"
    )]
    pub fn create_texture_2d(
        &self,
        width: u32,
        height: u32,
        mip_levels: u32,
        format: DXGI_FORMAT,
        flags: D3D12_RESOURCE_FLAGS,
        initial_state: D3D12_RESOURCE_STATES,
        clear_value: Option<D3D12_CLEAR_VALUE>,
    ) -> Result<ID3D12Resource, RendererError> {
        if width == 0 || height == 0 {
            return Err(RendererError::InvalidConfig {
                reason: format!("texture size {width}x{height} has a zero dimension"),
            });
        }

        let heap_properties = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_DEFAULT,
            ..Default::default()
        };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Alignment: 0,
            Width: u64::from(width),
            Height: height,
            DepthOrArraySize: 1,
            // The count the caller asked for. Zero would mean "as many as fit", which is
            // D3D12's own shorthand and deliberately not used: the uploader has to know how
            // many subresources it is filling, and a resource that decided that for itself
            // would be one the CPU side could only ask about after the fact.
            MipLevels: mip_levels.max(1) as u16,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            Flags: flags,
        };

        let mut texture: Option<ID3D12Resource> = None;
        // SAFETY: FFI call with fully initialized descriptors that outlive it, and a valid
        // out-parameter. `clear_value` is borrowed for the duration of the call only.
        unsafe {
            self.device.CreateCommittedResource(
                &heap_properties,
                D3D12_HEAP_FLAG_NONE,
                &desc,
                initial_state,
                clear_value.as_ref().map(std::ptr::from_ref),
                &mut texture,
            )
        }
        .map_err(|error| RendererError::api("ID3D12Device::CreateCommittedResource", error))?;

        texture.ok_or_else(|| RendererError::InvalidConfig {
            reason: "texture creation produced no resource".to_owned(),
        })
    }

    /// Creates a render-target view for `resource` at `handle`.
    pub fn create_render_target_view(
        &self,
        resource: &ID3D12Resource,
        handle: D3D12_CPU_DESCRIPTOR_HANDLE,
    ) {
        // SAFETY: FFI call. `None` for the description uses the resource's own format, and
        // the handle addresses a slot in a heap the caller owns.
        unsafe { self.device.CreateRenderTargetView(resource, None, handle) };
    }

    /// Creates a depth-stencil view for `resource` at `handle`.
    pub fn create_depth_stencil_view(
        &self,
        resource: &ID3D12Resource,
        handle: D3D12_CPU_DESCRIPTOR_HANDLE,
    ) {
        // SAFETY: FFI call. `None` for the description uses the resource's own format, and
        // the handle addresses a slot in a heap the caller owns.
        unsafe { self.device.CreateDepthStencilView(resource, None, handle) };
    }

    /// Creates a fence starting at `initial_value`.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if the device rejects the request.
    pub fn create_fence(&self, initial_value: u64) -> Result<ID3D12Fence, RendererError> {
        // SAFETY: FFI call on a live device; `D3D12_FENCE_FLAG_NONE` is valid.
        unsafe {
            self.device
                .CreateFence(initial_value, D3D12_FENCE_FLAG_NONE)
        }
        .map_err(|error| RendererError::api("ID3D12Device::CreateFence", error))
    }
}

/// Formats a feature level constant as the name used in D3D documentation.
#[must_use]
pub fn feature_level_name(level: D3D_FEATURE_LEVEL) -> &'static str {
    match level {
        D3D_FEATURE_LEVEL_12_2 => "12_2",
        D3D_FEATURE_LEVEL_12_1 => "12_1",
        D3D_FEATURE_LEVEL_12_0 => "12_0",
        D3D_FEATURE_LEVEL_11_1 => "11_1",
        D3D_FEATURE_LEVEL_11_0 => "11_0",
        _ => "unknown",
    }
}

/// Turns on the D3D12 validation layer. Returns whether it was enabled.
fn enable_d3d12_debug_layer() -> bool {
    let mut debug: Option<ID3D12Debug> = None;
    // SAFETY: FFI call filling a valid out-parameter. The interface is absent when the
    // optional "Graphics Tools" Windows feature is not installed, which is not an error.
    if unsafe { D3D12GetDebugInterface(&mut debug) }.is_err() {
        log::warn!("D3D12 debug layer unavailable (install the Graphics Tools feature)");
        return false;
    }
    let Some(debug) = debug else {
        return false;
    };
    // SAFETY: FFI call on a live COM object; enabling validation has no preconditions.
    unsafe { debug.EnableDebugLayer() };
    log::info!("D3D12 debug layer enabled");
    true
}

/// Makes the debug layer break the process on validation errors.
///
/// Best-effort: the info queue only exists when the debug layer is active.
fn configure_info_queue(device: &ID3D12Device) {
    let Ok(info_queue) = device.cast::<ID3D12InfoQueue>() else {
        return;
    };
    // SAFETY: FFI call on a live COM object with a valid severity constant. A failure
    // only means validation errors will not break, which is not worth aborting over.
    if let Err(error) = unsafe { info_queue.SetBreakOnSeverity(D3D12_MESSAGE_SEVERITY_ERROR, true) }
    {
        log::debug!("SetBreakOnSeverity failed: {error}");
    }
}

/// Asks the device which of [`PROBED_FEATURE_LEVELS`] it actually supports.
fn highest_feature_level(device: &ID3D12Device) -> D3D_FEATURE_LEVEL {
    let mut data = D3D12_FEATURE_DATA_FEATURE_LEVELS {
        NumFeatureLevels: PROBED_FEATURE_LEVELS.len() as u32,
        pFeatureLevelsRequested: PROBED_FEATURE_LEVELS.as_ptr(),
        MaxSupportedFeatureLevel: MINIMUM_FEATURE_LEVEL,
    };
    // SAFETY: FFI call. `data` is fully initialized, the pointer it carries refers to a
    // 'static array, and the size matches the struct D3D12 expects for this feature.
    let result = unsafe {
        device.CheckFeatureSupport(
            D3D12_FEATURE_FEATURE_LEVELS,
            std::ptr::from_mut(&mut data).cast(),
            size_of::<D3D12_FEATURE_DATA_FEATURE_LEVELS>() as u32,
        )
    };
    match result {
        Ok(()) => data.MaxSupportedFeatureLevel,
        Err(error) => {
            log::debug!("CheckFeatureSupport(FEATURE_LEVELS) failed: {error}");
            MINIMUM_FEATURE_LEVEL
        }
    }
}

/// Reports whether `DXGI_PRESENT_ALLOW_TEARING` may be used on this system.
///
/// Tearing support is what makes uncapped presentation possible in a window; without it
/// the swap chain would block on the display refresh and the frame-rate measurements
/// would describe the monitor, not the engine.
fn query_tearing_support(factory: &IDXGIFactory6) -> bool {
    let mut allow_tearing: u32 = 0;
    // SAFETY: FFI call. The out-parameter is a `u32` and the size passed matches it,
    // which is what DXGI expects for this feature query.
    let result = unsafe {
        factory.CheckFeatureSupport(
            DXGI_FEATURE_PRESENT_ALLOW_TEARING,
            std::ptr::from_mut(&mut allow_tearing).cast(),
            size_of::<u32>() as u32,
        )
    };
    match result {
        Ok(()) => allow_tearing != 0,
        Err(error) => {
            log::debug!("CheckFeatureSupport(ALLOW_TEARING) failed: {error}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_level_names_match_documentation() {
        assert_eq!(feature_level_name(D3D_FEATURE_LEVEL_12_1), "12_1");
        assert_eq!(feature_level_name(D3D_FEATURE_LEVEL_11_0), "11_0");
        assert_eq!(feature_level_name(D3D_FEATURE_LEVEL(0)), "unknown");
    }

    #[test]
    fn probed_levels_are_ordered_high_to_low() {
        let mut previous = i32::MAX;
        for level in PROBED_FEATURE_LEVELS {
            assert!(level.0 < previous, "{} is out of order", level.0);
            previous = level.0;
        }
        assert_eq!(
            PROBED_FEATURE_LEVELS[PROBED_FEATURE_LEVELS.len() - 1],
            MINIMUM_FEATURE_LEVEL
        );
    }
}

#[cfg(test)]
mod thread_safety_probe {
    /// Whether a D3D12 device handle can be moved between threads at all.
    ///
    /// Asked as a compile check rather than assumed: whether the startup path may create the
    /// device on a worker thread while the main thread loads content depends on it.
    #[test]
    fn a_device_handle_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<windows::Win32::Graphics::Direct3D12::ID3D12Device>();
        assert_send_sync::<windows::Win32::Graphics::Dxgi::IDXGIFactory6>();
    }
}
