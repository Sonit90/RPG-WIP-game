//! The Metal device and its one command queue.

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{MTLCommandQueue, MTLCreateSystemDefaultDevice, MTLDevice, MTLGPUFamily};

use super::{Device, api_error};
use crate::adapter::AdapterInfo;
use crate::device::VideoMemory;
use crate::error::RendererError;

/// Apple's PCI vendor id, which Metal has no use for but `AdapterInfo` reports.
const APPLE_VENDOR_ID: u32 = 0x106B;

/// The system's default Metal device and the queue every frame is submitted on.
pub(crate) struct MetalDevice {
    device: Device,
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    adapter_info: AdapterInfo,
    metal3: bool,
    block_compression: bool,
}

impl MetalDevice {
    /// Opens the default device and creates the queue.
    ///
    /// # Errors
    ///
    /// [`RendererError::NoSuitableAdapter`] if there is no Metal device or it is older than
    /// Apple silicon (`Apple7`, the M1 generation, or `Mac2`), and [`RendererError::Api`] if
    /// the queue cannot be created.
    pub(crate) fn new() -> Result<Self, RendererError> {
        let Some(device) = MTLCreateSystemDefaultDevice() else {
            return Err(RendererError::NoSuitableAdapter { examined: 0 });
        };
        if !(device.supportsFamily(MTLGPUFamily::Apple7)
            || device.supportsFamily(MTLGPUFamily::Mac2))
        {
            return Err(RendererError::NoSuitableAdapter { examined: 1 });
        }
        let Some(queue) = device.newCommandQueue() else {
            return Err(api_error(
                "MTLDevice::newCommandQueue",
                "the device returned no command queue",
            ));
        };
        let working_set = device.recommendedMaxWorkingSetSize();
        let adapter_info = AdapterInfo {
            name: device.name().to_string(),
            // Unified memory: the GPU's budget is the working set the OS recommends.
            dedicated_video_memory: working_set,
            dedicated_system_memory: 0,
            shared_system_memory: 0,
            vendor_id: APPLE_VENDOR_ID,
            // The registry id identifies the device within a boot, which is all a
            // diagnostic needs; its low half is what fits.
            device_id: (device.registryID() & u64::from(u32::MAX)) as u32,
            is_software: false,
        };
        Ok(Self {
            metal3: device.supportsFamily(MTLGPUFamily::Metal3),
            block_compression: device.supportsBCTextureCompression(),
            device,
            queue,
            adapter_info,
        })
    }

    /// The device.
    pub(crate) fn device(&self) -> &Device {
        &self.device
    }

    /// The command queue.
    pub(crate) fn queue(&self) -> &ProtocolObject<dyn MTLCommandQueue> {
        &self.queue
    }

    /// Description of the adapter in use.
    pub(crate) fn adapter_info(&self) -> &AdapterInfo {
        &self.adapter_info
    }

    /// Memory this process is using, as the device reports it.
    pub(crate) fn video_memory(&self) -> VideoMemory {
        VideoMemory {
            local_usage: self.device.currentAllocatedSize() as u64,
            local_budget: self.device.recommendedMaxWorkingSetSize(),
            non_local_usage: 0,
        }
    }

    /// Name of the feature set, for the startup log and `feature_level_name`.
    pub(crate) fn feature_level_name(&self) -> &'static str {
        if self.metal3 { "Metal 3" } else { "Metal" }
    }

    /// Whether BC1/BC5 textures can be created. Every Apple silicon Mac supports them; the
    /// check is a guard rather than a branch anyone expects to take.
    pub(crate) fn supports_block_compression(&self) -> bool {
        self.block_compression
    }
}
