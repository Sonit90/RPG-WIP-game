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
pub mod device;
pub mod error;
pub mod gpu_timer;
pub mod offscreen;
pub mod pipeline;
pub mod renderer;
pub mod shader;
pub mod swapchain;
pub mod sync;

pub use adapter::{AdapterInfo, select_adapter};
pub use device::BACKEND;
pub use error::RendererError;
pub use pipeline::{TRIANGLE_VERTICES, Vertex};
pub use renderer::Renderer;
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
}

impl Default for RendererConfig {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            vsync: false,
            enable_debug_layer: cfg!(debug_assertions),
            break_on_validation_error: cfg!(debug_assertions),
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
