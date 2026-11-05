//! The debug overlay's GPU side: a cached vertex buffer per frame slot.
//!
//! The developer HUD changes a few times a second and the frame runs thousands of times a
//! second, so its geometry is uploaded to a slot's buffer only when the text changed, and
//! each slot keeps its copy until then. The dynamic layer (nameplates) is rebuilt every frame
//! and goes through the upload ring instead.

use objc2_metal::MTLBuffer;

use super::resources::empty_buffer;
use super::{Buffer, Device};
use crate::error::RendererError;
use crate::overlay::OverlayGeometry;
use crate::pipeline::Vertex;

/// Most vertices a cached buffer holds: the overlay's own cap of 32,768 quads of 6 vertices.
const MAX_VERTICES: usize = 32_768 * 6;

/// One slot's cached vertex buffer.
struct SlotBuffer {
    buffer: Buffer,
    /// Vertices currently in the buffer.
    count: usize,
}

/// Debug overlay state owned by the renderer.
pub(crate) struct OverlayState {
    pub(crate) geometry: OverlayGeometry,
    /// Geometry rebuilt from scratch every frame: nameplates and anything else anchored to
    /// something that moves.
    pub(crate) dynamic: OverlayGeometry,
    /// Whether the overlay is drawn. Runtime switch on top of the compile-time feature.
    pub(crate) enabled: bool,
    /// Slots whose buffer no longer matches the current geometry.
    pub(crate) stale: Vec<bool>,
    /// Vertices drawn by the most recent frame, both layers together.
    pub(crate) drawn_vertices: u32,
    slots: Vec<Option<SlotBuffer>>,
}

impl OverlayState {
    /// Creates the state for `slot_count` frame slots. No GPU memory is allocated until the
    /// first text is drawn.
    pub(crate) fn new(slot_count: usize) -> Self {
        Self {
            geometry: OverlayGeometry::new(),
            dynamic: OverlayGeometry::new(),
            enabled: true,
            stale: vec![true; slot_count],
            drawn_vertices: 0,
            slots: (0..slot_count).map(|_| None).collect(),
        }
    }

    /// Copies the current geometry into `slot`'s buffer if it is stale, and returns the buffer
    /// with its vertex count, or `None` when there is nothing to draw.
    ///
    /// # Errors
    ///
    /// [`RendererError::Api`] if a buffer cannot be allocated.
    pub(crate) fn cached(
        &mut self,
        device: &Device,
        slot: usize,
    ) -> Result<Option<(&Buffer, u32)>, RendererError> {
        let (Some(stale), Some(entry)) = (self.stale.get_mut(slot), self.slots.get_mut(slot))
        else {
            return Ok(None);
        };
        if *stale {
            let vertices = self.geometry.vertices();
            let count = vertices.len().min(MAX_VERTICES);
            if count == 0 {
                *entry = None;
            } else {
                let bytes = count * size_of::<Vertex>();
                let reusable = entry
                    .as_ref()
                    .is_some_and(|slot| slot.buffer.length() >= bytes);
                if !reusable {
                    // Room for the whole cap once, so the buffer is not reallocated as the
                    // text grows line by line.
                    *entry = Some(SlotBuffer {
                        buffer: empty_buffer(
                            device,
                            MAX_VERTICES * size_of::<Vertex>(),
                            "the overlay vertex buffer",
                        )?,
                        count: 0,
                    });
                }
                if let Some(target) = entry.as_mut() {
                    // SAFETY: the buffer is shared storage of at least `bytes` bytes (checked
                    // or just allocated at the cap), `count` vertices are `bytes` readable
                    // bytes of `vertices`, and the GPU is not reading this slot's buffer: the
                    // frame ring waited for the slot's previous frame before recording.
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            vertices.as_ptr().cast::<u8>(),
                            target.buffer.contents().as_ptr().cast::<u8>(),
                            bytes,
                        );
                    }
                    target.count = count;
                }
            }
            *stale = false;
        }
        Ok(entry
            .as_ref()
            .filter(|slot| slot.count > 0)
            .map(|slot| (&slot.buffer, slot.count as u32)))
    }
}
