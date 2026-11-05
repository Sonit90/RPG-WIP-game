//! Reading a frame back to the CPU: screenshots and `capture_world`.
//!
//! The drawable's texture is blitted into a shared buffer by the same command buffer that drew
//! the frame, and once that command buffer has finished the buffer is converted to the RGBA8
//! rows `CapturedImage` carries. The layer's pixels are BGRA, so the red and blue bytes swap.

use objc2::runtime::ProtocolObject;
use objc2_metal::{
    MTLBlitCommandEncoder, MTLBuffer, MTLCommandEncoder, MTLOrigin, MTLSize, MTLTexture,
};

use super::resources::empty_buffer;
use super::{Buffer, Device};
use crate::error::RendererError;

/// Rows of the readback buffer start on this boundary, as D3D12's copy footprint requires and
/// Metal does not mind.
const ROW_ALIGNMENT: usize = 256;

/// A buffer a frame will be copied into.
pub(crate) struct Readback {
    buffer: Buffer,
    /// Width in pixels.
    pub(crate) width: u32,
    /// Height in pixels.
    pub(crate) height: u32,
    bytes_per_row: usize,
}

impl Readback {
    /// Allocates a buffer for a `width` x `height` BGRA8 image.
    ///
    /// # Errors
    ///
    /// [`RendererError::Api`] if the buffer cannot be allocated.
    pub(crate) fn new(device: &Device, width: u32, height: u32) -> Result<Self, RendererError> {
        let bytes_per_row = (width as usize * 4).next_multiple_of(ROW_ALIGNMENT);
        Ok(Self {
            buffer: empty_buffer(device, bytes_per_row * height as usize, "a readback")?,
            width,
            height,
            bytes_per_row,
        })
    }

    /// Records the copy of `texture` (this readback's size, BGRA8) into the buffer.
    pub(crate) fn record_copy(
        &self,
        encoder: &ProtocolObject<dyn MTLBlitCommandEncoder>,
        texture: &ProtocolObject<dyn MTLTexture>,
    ) {
        // SAFETY: the texture is `width` x `height` (the readback was sized from the layer it
        // was drawn into), the buffer holds `height` rows of `bytes_per_row` bytes, and both
        // stay alive until the command buffer completes because Metal retains them.
        unsafe {
            encoder.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage(
                texture,
                0,
                0,
                MTLOrigin { x: 0, y: 0, z: 0 },
                MTLSize {
                    width: self.width as usize,
                    height: self.height as usize,
                    depth: 1,
                },
                &self.buffer,
                0,
                self.bytes_per_row,
                self.bytes_per_row * self.height as usize,
            );
        }
        encoder.endEncoding();
    }

    /// The image as tightly packed RGBA8 rows. Call only after the frame's command buffer has
    /// completed.
    pub(crate) fn pixels(&self) -> Vec<u8> {
        let row_bytes = self.width as usize * 4;
        let mut pixels = Vec::with_capacity(row_bytes * self.height as usize);
        // SAFETY: the buffer is shared storage of `bytes_per_row * height` bytes and the GPU
        // has finished writing it (the caller waited for the frame), so reading it as bytes
        // is sound.
        let source = unsafe {
            std::slice::from_raw_parts(
                self.buffer.contents().as_ptr().cast::<u8>().cast_const(),
                self.bytes_per_row * self.height as usize,
            )
        };
        for row in source.chunks_exact(self.bytes_per_row) {
            for texel in row[..row_bytes].chunks_exact(4) {
                pixels.extend_from_slice(&[texel[2], texel[1], texel[0], texel[3]]);
            }
        }
        pixels
    }
}

#[cfg(test)]
mod tests {
    /// The row pitch is a multiple of the alignment and never smaller than the row.
    #[test]
    fn row_pitch_is_aligned() {
        for width in [1_usize, 63, 64, 65, 1920] {
            let pitch = (width * 4).next_multiple_of(super::ROW_ALIGNMENT);
            assert!(pitch >= width * 4);
            assert_eq!(pitch % super::ROW_ALIGNMENT, 0);
        }
    }
}
