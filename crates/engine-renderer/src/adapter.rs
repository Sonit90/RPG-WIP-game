//! Adapter enumeration and selection.
//!
//! Selection policy lives in [`select_adapter`], which is pure and unit-tested. Nothing
//! here mentions a specific vendor or model: the engine must run on any D3D12 adapter,
//! and simply prefers the most capable one it can find.

#[cfg(windows)]
use windows::Win32::Graphics::Dxgi::{
    DXGI_ADAPTER_FLAG, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_ERROR_NOT_FOUND, IDXGIAdapter1,
    IDXGIFactory6,
};

#[cfg(windows)]
use crate::error::RendererError;

/// Description of one graphics adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterInfo {
    /// Human-readable adapter name, e.g. `AMD Radeon RX 6700 XT`.
    pub name: String,
    /// Memory dedicated to the GPU, in bytes. Zero for adapters without dedicated VRAM.
    pub dedicated_video_memory: u64,
    /// System memory dedicated to the adapter, in bytes.
    pub dedicated_system_memory: u64,
    /// System memory shared with the adapter, in bytes.
    pub shared_system_memory: u64,
    /// PCI vendor id, kept for diagnostics only — never for behaviour decisions.
    pub vendor_id: u32,
    /// PCI device id, kept for diagnostics only.
    pub device_id: u32,
    /// Whether this is a software rasterizer such as WARP.
    pub is_software: bool,
}

impl AdapterInfo {
    /// Dedicated video memory in mebibytes, for logging.
    #[must_use]
    pub fn dedicated_video_memory_mib(&self) -> u64 {
        self.dedicated_video_memory / (1024 * 1024)
    }
}

/// Picks the index of the adapter the engine should use.
///
/// Policy, in order:
///
/// 1. Hardware adapters are preferred over software rasterizers, always.
/// 2. Among hardware adapters, the one with the most dedicated video memory wins. This
///    is what distinguishes a discrete GPU from an integrated one without naming vendors.
/// 3. Ties are broken by enumeration order, which DXGI reports with the adapter driving
///    the primary output first.
/// 4. If only software adapters exist, the first one is used so the engine still runs.
///
/// Returns `None` only for an empty list.
#[must_use]
pub fn select_adapter(candidates: &[AdapterInfo]) -> Option<usize> {
    let best_hardware = candidates
        .iter()
        .enumerate()
        .filter(|(_, adapter)| !adapter.is_software)
        .max_by_key(|(index, adapter)| {
            // `max_by_key` keeps the *last* maximum, so invert the index to keep the first.
            (adapter.dedicated_video_memory, usize::MAX - index)
        })
        .map(|(index, _)| index);

    best_hardware.or(if candidates.is_empty() { None } else { Some(0) })
}

#[cfg(windows)]
/// Enumerates every DXGI adapter on the system.
///
/// Returns the COM objects alongside their descriptions so the caller can create a
/// device from the selected one without enumerating twice.
///
/// # Errors
///
/// Returns [`RendererError::Api`] if DXGI fails for a reason other than running out of
/// adapters, or if an adapter description cannot be read.
pub fn enumerate_adapters(
    factory: &IDXGIFactory6,
) -> Result<Vec<(IDXGIAdapter1, AdapterInfo)>, RendererError> {
    let mut adapters = Vec::new();
    let mut index = 0u32;

    loop {
        // SAFETY: FFI call. `factory` is a live COM object; DXGI returns
        // DXGI_ERROR_NOT_FOUND once `index` passes the last adapter, which ends the loop.
        let adapter = match unsafe { factory.EnumAdapters1(index) } {
            Ok(adapter) => adapter,
            Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(error) => return Err(RendererError::api("IDXGIFactory6::EnumAdapters1", error)),
        };

        // SAFETY: FFI call that fills a plain out-parameter owned by this function.
        let desc = unsafe { adapter.GetDesc1() }
            .map_err(|error| RendererError::api("IDXGIAdapter1::GetDesc1", error))?;

        let name_len = desc
            .Description
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(desc.Description.len());
        let info = AdapterInfo {
            name: String::from_utf16_lossy(&desc.Description[..name_len]),
            dedicated_video_memory: desc.DedicatedVideoMemory as u64,
            dedicated_system_memory: desc.DedicatedSystemMemory as u64,
            shared_system_memory: desc.SharedSystemMemory as u64,
            vendor_id: desc.VendorId,
            device_id: desc.DeviceId,
            is_software: DXGI_ADAPTER_FLAG(desc.Flags as i32) == DXGI_ADAPTER_FLAG_SOFTWARE,
        };

        adapters.push((adapter, info));
        index += 1;
    }

    Ok(adapters)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter(name: &str, vram_gib: u64, is_software: bool) -> AdapterInfo {
        AdapterInfo {
            name: name.to_owned(),
            dedicated_video_memory: vram_gib * 1024 * 1024 * 1024,
            dedicated_system_memory: 0,
            shared_system_memory: 8 * 1024 * 1024 * 1024,
            vendor_id: 0,
            device_id: 0,
            is_software,
        }
    }

    #[test]
    fn prefers_the_discrete_gpu_over_the_integrated_one() {
        let candidates = [
            adapter("integrated", 0, false),
            adapter("discrete", 12, false),
        ];
        assert_eq!(select_adapter(&candidates), Some(1));
    }

    #[test]
    fn order_does_not_matter() {
        let candidates = [
            adapter("discrete", 12, false),
            adapter("integrated", 0, false),
        ];
        assert_eq!(select_adapter(&candidates), Some(0));
    }

    #[test]
    fn never_prefers_a_software_adapter_with_large_reported_memory() {
        let candidates = [adapter("warp", 64, true), adapter("gpu", 8, false)];
        assert_eq!(select_adapter(&candidates), Some(1));
    }

    #[test]
    fn falls_back_to_software_when_that_is_all_there_is() {
        let candidates = [adapter("warp", 0, true)];
        assert_eq!(select_adapter(&candidates), Some(0));
    }

    #[test]
    fn ties_keep_enumeration_order() {
        let candidates = [
            adapter("first", 8, false),
            adapter("second", 8, false),
            adapter("third", 8, false),
        ];
        assert_eq!(select_adapter(&candidates), Some(0));
    }

    #[test]
    fn empty_list_selects_nothing() {
        assert_eq!(select_adapter(&[]), None);
    }

    #[test]
    fn reports_memory_in_mebibytes() {
        assert_eq!(
            adapter("gpu", 12, false).dedicated_video_memory_mib(),
            12288
        );
    }
}
