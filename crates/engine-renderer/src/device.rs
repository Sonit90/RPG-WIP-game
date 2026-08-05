//! DXGI factory, adapter selection, D3D12 device and the direct command queue.

use windows::Win32::Graphics::Direct3D::{
    D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_12_0,
    D3D_FEATURE_LEVEL_12_1, D3D_FEATURE_LEVEL_12_2,
};
use windows::Win32::Graphics::Direct3D12::{
    D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_COMMAND_QUEUE_DESC, D3D12_COMMAND_QUEUE_FLAG_NONE,
    D3D12_FEATURE_DATA_FEATURE_LEVELS, D3D12_FEATURE_FEATURE_LEVELS, D3D12_MESSAGE_SEVERITY_ERROR,
    D3D12CreateDevice, D3D12GetDebugInterface, ID3D12CommandQueue, ID3D12Debug, ID3D12Device,
    ID3D12InfoQueue,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, DXGI_CREATE_FACTORY_DEBUG, DXGI_CREATE_FACTORY_FLAGS,
    DXGI_FEATURE_PRESENT_ALLOW_TEARING, IDXGIFactory6,
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
pub struct GpuContext {
    factory: IDXGIFactory6,
    device: ID3D12Device,
    queue: ID3D12CommandQueue,
    adapter_info: AdapterInfo,
    feature_level: D3D_FEATURE_LEVEL,
    allow_tearing: bool,
}

impl GpuContext {
    /// Creates the factory, selects an adapter, and creates the device and queue.
    ///
    /// `enable_debug_layer` turns on the D3D12 validation layer, which is expensive and
    /// intended for debug builds only.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::NoSuitableAdapter`] if no adapter can create a device at
    /// feature level 11_0, or [`RendererError::Api`] if a DXGI/D3D12 call fails.
    pub fn new(enable_debug_layer: bool) -> Result<Self, RendererError> {
        let mut factory_flags = DXGI_CREATE_FACTORY_FLAGS(0);
        if enable_debug_layer && enable_d3d12_debug_layer() {
            factory_flags |= DXGI_CREATE_FACTORY_DEBUG;
        }

        // SAFETY: FFI call. The flags are valid and the returned interface is refcounted
        // by the `windows` crate.
        let factory: IDXGIFactory6 = unsafe { CreateDXGIFactory2(factory_flags) }
            .map_err(|error| RendererError::api("CreateDXGIFactory2", error))?;

        let adapters = enumerate_adapters(&factory)?;
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

        if enable_debug_layer {
            configure_info_queue(&device);
        }

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

        Ok(Self {
            factory,
            device,
            queue,
            adapter_info: adapter_info.clone(),
            feature_level,
            allow_tearing,
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
