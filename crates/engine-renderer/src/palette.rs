//! Bone palettes, uploaded once a frame and bound by every pass that draws a character.
//!
//! # Why this exists
//!
//! A skinned character is drawn by up to a dozen passes a frame: every shadow cascade and
//! local shadow view it falls in, the camera's depth prepass, the camera pass itself, and the
//! two translucent-character passes when the camera looks through it. Each of those used to
//! build the character's palette — every bone as a column-major matrix, padded with the
//! identity to the shader's [`MAX_PALETTE_BONES`] — and copy all 8 KB of it into the frame's
//! upload memory for itself. The data was the same every time: one character, one pose, one
//! frame. The dungeon uploaded 106 palettes a frame for 24 characters (850 KB of writes), the
//! graveyard 160 for 38, and it was the largest single cost in recording a frame
//! (docs/PERFORMANCE_AUDIT.md).
//!
//! So a frame uploads each distinct palette once, before anything is recorded, and every
//! pass binds the address it got. What the GPU reads is unchanged: the same 8 KB, padded the
//! same way, at a different offset of the same ring.

use engine_core::math::Mat4;
use engine_render_data::MAX_PALETTE_BONES;
#[cfg(windows)]
use engine_render_data::SkinnedDrawList;

#[cfg(windows)]
use crate::error::RendererError;
#[cfg(windows)]
use crate::upload::{CONSTANT_BUFFER_ALIGNMENT, UploadRing};

/// Bytes one palette occupies on the GPU: the shader's whole declared array, so a root
/// constant-buffer view of it never lets the GPU read past its allocation.
pub const PALETTE_BYTES: usize = MAX_PALETTE_BONES * size_of::<[f32; 16]>();

/// This frame's uploaded palettes, by the range of the frame's palette they came from.
#[derive(Debug, Default)]
pub struct FramePalettes {
    /// `(first bone, bone count, GPU address)`, in upload order. A few dozen at most — one
    /// per character on screen or casting into a shadow — so a scan is the right lookup.
    entries: Vec<(u32, u32, u64)>,
    #[cfg(windows)]
    /// The padded palette being written, kept so uploading allocates nothing per frame.
    scratch: Vec<[f32; 16]>,
}

impl FramePalettes {
    /// An empty table.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Forgets last frame's addresses, which point into upload memory being reused.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    #[cfg(windows)]
    /// Uploads every palette `list`'s draws name that this frame has not uploaded yet.
    ///
    /// A draw whose range lies outside the list's palette is skipped here and reported by the
    /// pass that draws it, which already knows how to say so.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError`] if the upload ring is out of space.
    pub fn upload_all(
        &mut self,
        list: &SkinnedDrawList,
        uploads: &mut UploadRing,
        slot: usize,
    ) -> Result<(), RendererError> {
        for (index, instance) in list.instances().iter().enumerate() {
            if self
                .address(instance.palette_start, instance.palette_len)
                .is_some()
            {
                continue;
            }
            let Some(palette) = list.palette_of(index) else {
                continue;
            };
            let address = upload_padded(palette, &mut self.scratch, uploads, slot)?;
            self.entries
                .push((instance.palette_start, instance.palette_len, address));
        }
        Ok(())
    }

    /// Where this frame uploaded the palette starting at bone `start` and `len` bones long.
    #[must_use]
    pub fn address(&self, start: u32, len: u32) -> Option<u64> {
        self.entries
            .iter()
            .find(|entry| entry.0 == start && entry.1 == len)
            .map(|entry| entry.2)
    }

    /// How many palettes this frame has uploaded.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether this frame has uploaded none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Fills `scratch` with `palette`, padded with the identity to [`MAX_PALETTE_BONES`].
///
/// The backend-neutral half of [`upload_padded`]: what the GPU reads, before it is copied
/// anywhere. A palette longer than the array is cut to it, exactly as the shader would read it.
pub(crate) fn pad_palette(palette: &[Mat4], scratch: &mut Vec<[f32; 16]>) {
    scratch.clear();
    scratch.extend(palette.iter().map(Mat4::to_cols_array));
    scratch.resize(MAX_PALETTE_BONES, Mat4::IDENTITY.to_cols_array());
}

#[cfg(windows)]
/// Writes `palette`, padded with the identity to [`MAX_PALETTE_BONES`], into this frame's
/// upload memory and returns its GPU address.
///
/// The identity for the bones the character does not have: zeroes would collapse any vertex
/// that reached them to the origin, and the identity leaves it in bind pose, which is wrong in
/// a way that is visible and bounded. A palette longer than the array is cut to it, exactly as
/// the shader would read it.
///
/// # Errors
///
/// Returns [`RendererError`] if the upload ring is out of space.
pub fn upload_padded(
    palette: &[Mat4],
    scratch: &mut Vec<[f32; 16]>,
    uploads: &mut UploadRing,
    slot: usize,
) -> Result<u64, RendererError> {
    pad_palette(palette, scratch);
    let allocation = uploads.allocate(slot, PALETTE_BYTES, CONSTANT_BUFFER_ALIGNMENT)?;
    allocation.write(scratch)?;
    Ok(allocation.gpu_address())
}
