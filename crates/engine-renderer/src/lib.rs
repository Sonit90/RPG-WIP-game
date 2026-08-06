//! Direct3D 12 renderer.
//!
//! The renderer owns GPU resources and nothing else. It must never learn gameplay
//! concepts — no player, ability, damage, buff, debuff, DPS or enemy type may appear in
//! this crate. Data reaches it as rendering data (Milestone 4 onward).
//!
//! It also does not depend on the platform crate: a window arrives as an opaque `isize`
//! handle, so the dependency arrow points only downward.
//!
//! Phase 0 / Milestone 2 scope: device, queue, swap chain, render targets, fences, and a
//! frame that clears and presents. No pipeline state, no geometry, no GPU timing yet.

pub mod adapter;
pub mod bench;
pub mod camera;
pub mod command;
pub mod depth;
pub mod descriptor;
pub mod device;
pub mod error;
#[cfg(feature = "debug-overlay")]
pub mod font;
pub mod frame;
pub mod gpu_timer;
pub mod instancing;
pub mod material;
pub mod mesh;
pub mod offscreen;
#[cfg(feature = "debug-overlay")]
pub mod overlay;
pub mod pipeline;
pub mod renderer;
pub mod resource;
pub mod shader;
pub mod stress;
pub mod swapchain;
pub mod sync;
pub mod upload;

pub use adapter::{AdapterInfo, select_adapter};
pub use bench::{BenchHarness, BenchResult, DrawMode};
pub use camera::{Camera, DEFAULT_FAR, DEFAULT_FOV_Y, DEFAULT_NEAR, Projection};
pub use device::BACKEND;
pub use error::RendererError;
pub use frame::{DEFAULT_FRAMES_IN_FLIGHT, MAX_FRAMES_IN_FLIGHT, SyncStats};
pub use instancing::{InstanceData, UNIT_TRIANGLE};
pub use material::{Material, MaterialConstants, MaterialDesc};
pub use mesh::{GpuMesh, Index, MeshData, MeshVertex, VertexStream};
pub use pipeline::{TRIANGLE_VERTICES, Vertex};
pub use renderer::Renderer;
pub use stress::STRESS_LEVELS;
pub use swapchain::{BACK_BUFFER_FORMAT, FRAME_COUNT};

/// How the renderer should be created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RendererConfig {
    /// Back-buffer width in pixels.
    pub width: u32,
    /// Back-buffer height in pixels.
    pub height: u32,
    /// Whether presentation waits for vertical blank.
    ///
    /// Off by default: a vsync-locked swap chain measures the monitor, not the engine.
    pub vsync: bool,
    /// Whether to enable the D3D12 validation layer.
    ///
    /// Defaults to on in debug builds and off in release builds, because validation
    /// costs several times the CPU frame time.
    pub enable_debug_layer: bool,
    /// Whether a validation error should break into the debugger.
    ///
    /// Useful while developing, but a test process has no debugger attached, so tests
    /// enable the layer with this off and rely on the returned `HRESULT`s instead.
    pub break_on_validation_error: bool,
    /// Frames the CPU may run ahead of the GPU.
    ///
    /// One serializes CPU and GPU and is useful for isolating GPU cost; the default of
    /// three overlaps them and absorbs a frame that runs long. Bounded by the swap chain's
    /// buffer count — queueing more frames than there are back buffers cannot help,
    /// because `Present` blocks once every buffer is spoken for.
    pub frames_in_flight: usize,
    /// Bytes of upload memory reserved per frame slot.
    ///
    /// Holds one frame's constants, instance transforms and dynamic vertices. The default
    /// is a starting point, not a measurement: `Renderer::upload_peak_bytes` reports what
    /// a real scene actually uses, and the number should be chosen from that.
    pub upload_bytes_per_frame: usize,
}

/// Default per-frame upload capacity: 4 MiB.
///
/// Enough for roughly 65,000 instance transforms at 64 bytes each, which is well past any
/// scene the renderer can currently build.
pub const DEFAULT_UPLOAD_BYTES_PER_FRAME: usize = 4 * 1024 * 1024;

impl Default for RendererConfig {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            vsync: false,
            enable_debug_layer: cfg!(debug_assertions),
            break_on_validation_error: cfg!(debug_assertions),
            frames_in_flight: frame::DEFAULT_FRAMES_IN_FLIGHT,
            upload_bytes_per_frame: DEFAULT_UPLOAD_BYTES_PER_FRAME,
        }
    }
}

impl RendererConfig {
    /// Creates a configuration for the given back-buffer size.
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            ..Self::default()
        }
    }

    /// Returns the configuration with vsync overridden.
    #[must_use]
    pub fn with_vsync(mut self, vsync: bool) -> Self {
        self.vsync = vsync;
        self
    }

    /// Returns the configuration with the number of frames in flight overridden.
    ///
    /// Out-of-range values are rejected when the renderer is created, not silently
    /// clamped: a caller asking for eight frames in flight has a wrong expectation, and
    /// quietly giving them three would hide it.
    #[must_use]
    pub fn with_frames_in_flight(mut self, frames: usize) -> Self {
        self.frames_in_flight = frames;
        self
    }

    /// Returns the configuration with the debug layer overridden.
    ///
    /// Disabling the layer also disables breaking on validation errors, since there is
    /// nothing left to report them.
    #[must_use]
    pub fn with_debug_layer(mut self, enable: bool) -> Self {
        self.enable_debug_layer = enable;
        self.break_on_validation_error &= enable;
        self
    }

    /// Returns the configuration with debugger breaks on validation errors overridden.
    #[must_use]
    pub fn with_break_on_validation_error(mut self, enable: bool) -> Self {
        self.break_on_validation_error = enable;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_1080p_without_vsync() {
        let config = RendererConfig::default();
        assert_eq!((config.width, config.height), (1920, 1080));
        assert!(
            !config.vsync,
            "vsync would cap the measurements at the refresh rate"
        );
    }

    #[test]
    fn debug_layer_follows_the_build_profile() {
        assert_eq!(
            RendererConfig::default().enable_debug_layer,
            cfg!(debug_assertions)
        );
    }

    #[test]
    fn builders_override_defaults() {
        let config = RendererConfig::new(1280, 720)
            .with_vsync(true)
            .with_debug_layer(false);
        assert_eq!((config.width, config.height), (1280, 720));
        assert!(config.vsync);
        assert!(!config.enable_debug_layer);
    }
}
