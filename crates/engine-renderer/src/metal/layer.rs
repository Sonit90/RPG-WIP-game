//! The `CAMetalLayer` the window presents through.

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::NSView;
use objc2_core_foundation::CGSize;
use objc2_metal::MTLPixelFormat;
use objc2_quartz_core::{CALayer, CAMetalDrawable, CAMetalLayer};

use super::{Device, api_error};
use crate::error::RendererError;

/// The pixel format of the drawables.
///
/// BGRA rather than RGBA because that is the order `CAMetalLayer` accepts; the shaders cannot
/// tell. Not an sRGB format: like the D3D12 back buffer, it holds display-ready values the
/// shaders already encoded.
pub(crate) const COLOR_FORMAT: MTLPixelFormat = MTLPixelFormat::BGRA8Unorm;

/// How many drawables the layer keeps in rotation: the swap chain's buffer count.
const DRAWABLE_COUNT: usize = 3;

/// The layer attached to the window's view and the size of its drawables.
pub(crate) struct MetalLayer {
    layer: Retained<CAMetalLayer>,
    size: (u32, u32),
}

impl MetalLayer {
    /// Attaches a new `CAMetalLayer` to the `NSView` at `view` and sizes it in pixels.
    ///
    /// `view` is the platform layer's raw handle: the address of the window's `NSView`, which
    /// must stay alive for the renderer's life. Setting a view's layer is AppKit work; call this
    /// on the main thread.
    ///
    /// # Errors
    ///
    /// [`RendererError::InvalidConfig`] for a null handle.
    pub(crate) fn new(
        view: isize,
        device: &Device,
        width: u32,
        height: u32,
        vsync: bool,
    ) -> Result<Self, RendererError> {
        if view == 0 {
            return Err(RendererError::InvalidConfig {
                reason: "window handle is null".to_owned(),
            });
        }
        // SAFETY: the platform layer documents the handle as the address of the window's
        // live `NSView`, and the caller keeps the window alive for the renderer's life. The
        // reference does not outlive this function.
        let view: &NSView = unsafe { &*(view as *const NSView) };

        let layer = CAMetalLayer::layer();
        layer.setDevice(Some(device));
        layer.setPixelFormat(COLOR_FORMAT);
        // Screenshots blit out of the drawable, which a framebuffer-only texture forbids.
        layer.setFramebufferOnly(false);
        layer.setMaximumDrawableCount(DRAWABLE_COUNT);
        layer.setDisplaySyncEnabled(vsync);
        layer.setDrawableSize(CGSize {
            width: f64::from(width),
            height: f64::from(height),
        });
        // The layer is sized in pixels; the scale tells the compositor how many of them make
        // a point so it does not resample.
        let scale = view
            .window()
            .map_or(1.0, |window| window.backingScaleFactor());
        layer.setContentsScale(scale);

        let base: &CALayer = &layer;
        view.setWantsLayer(true);
        view.setLayer(Some(base));
        Ok(Self {
            layer,
            size: (width, height),
        })
    }

    /// Current drawable size in pixels.
    pub(crate) fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Whether presentation waits for vertical blank.
    pub(crate) fn vsync(&self) -> bool {
        self.layer.displaySyncEnabled()
    }

    /// Changes the drawable size. The caller has waited for the GPU.
    pub(crate) fn resize(&mut self, width: u32, height: u32) {
        self.layer.setDrawableSize(CGSize {
            width: f64::from(width),
            height: f64::from(height),
        });
        self.size = (width, height);
    }

    /// The next drawable, which blocks while all of them are in use.
    ///
    /// # Errors
    ///
    /// [`RendererError::Api`] if the layer produced none (the system gives up after a second).
    pub(crate) fn next_drawable(
        &self,
    ) -> Result<Retained<ProtocolObject<dyn CAMetalDrawable>>, RendererError> {
        self.layer.nextDrawable().ok_or_else(|| {
            api_error(
                "CAMetalLayer::nextDrawable",
                "the layer produced no drawable (the window may be occluded or the display asleep)",
            )
        })
    }
}
