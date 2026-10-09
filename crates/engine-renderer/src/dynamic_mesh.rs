//! Meshes whose geometry is rewritten at run time, up to once per frame.
//!
//! # Why not `replace_mesh`
//!
//! [`Renderer::replace_mesh`](crate::Renderer::replace_mesh) suballocates from the geometry
//! arena, which never frees piecemeal: called every frame it would leak the arena. A dynamic
//! mesh instead owns a few persistently mapped upload-heap buffers, allocated once, that the
//! CPU overwrites and the GPU reads in place - the same shape as the overlay's vertex buffer.
//!
//! # Which copy a write may touch
//!
//! A mesh keeps `frames_in_flight + 1` copies of its vertex and index buffers. Each *write*
//! that lands in a new frame advances to the next copy; further writes in the same frame
//! reuse the current one, because a frame that has not ended has not been submitted.
//!
//! This is safe by counting. Writes happen in strictly increasing frames, so the copy a
//! write is about to overwrite was last pointed at by the write `frames_in_flight + 1` writes
//! earlier. That copy stopped being the mesh's geometry when the *next* write happened, at
//! least `frames_in_flight + 1 - 1` frames ago, and the last frame that could have recorded it
//! is the one before that write. The frame ring only lets the CPU run `frames_in_flight`
//! frames ahead of the GPU, so a frame that old has been proven complete. Rotating per
//! write rather than per frame number matters: a mesh left untouched for several frames
//! keeps being drawn from its current copy, and a frame-number scheme could land the next
//! write on exactly that copy while still in flight.
//!
//! No fence is consulted; the argument rests on the ring's bound alone, which is also what
//! the per-frame upload memory relies on.

use std::sync::Arc;

use engine_core::math::Sphere;

use crate::device::RenderDevice;
use crate::error::RendererError;
use crate::mesh::{GpuMesh, Index, MeshVertex};
use crate::resource::{BufferSlice, GpuBuffer, MappedPtr};

/// Which of a dynamic mesh's copies the next write goes to.
///
/// Pure bookkeeping, separated from the buffers so the rotation rule can be tested without a
/// device.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CopyRotation {
    copies: usize,
    current: usize,
    /// The frame of the latest write, or `None` before the first.
    last_write_frame: Option<u64>,
}

impl CopyRotation {
    /// A rotation over `copies` buffers; `copies` is clamped to at least one.
    pub(crate) fn new(copies: usize) -> Self {
        Self {
            copies: copies.max(1),
            current: 0,
            last_write_frame: None,
        }
    }

    /// The copy a write in `frame` goes to, recording that the write happened.
    pub(crate) fn next_for_write(&mut self, frame: u64) -> usize {
        match self.last_write_frame {
            // Nothing has ever been drawn from any copy.
            None => {}
            // Same frame, so not yet submitted: overwrite in place.
            Some(last) if last == frame => {}
            Some(_) => self.current = (self.current + 1) % self.copies,
        }
        self.last_write_frame = Some(frame);
        self.current
    }
}

/// One upload-heap copy of a dynamic mesh's geometry.
struct BufferPair {
    vertices: BufferSlice,
    vertex_ptr: MappedPtr,
    indices: BufferSlice,
    index_ptr: MappedPtr,
}

/// The CPU side of a dynamic mesh: its copies and which one is live.
///
/// The [`GpuMesh`] it feeds lives in the renderer's mesh list like any other, so the handle
/// works everywhere a mesh handle does.
pub(crate) struct DynamicMesh {
    copies: Vec<BufferPair>,
    rotation: CopyRotation,
    max_vertices: usize,
    max_indices: usize,
}

impl DynamicMesh {
    /// Allocates `frames_in_flight + 1` copies and returns them with the empty [`GpuMesh`]
    /// that draws from them.
    pub(crate) fn new(
        device: &RenderDevice,
        frames_in_flight: usize,
        max_vertices: usize,
        max_indices: usize,
    ) -> Result<(Self, GpuMesh), RendererError> {
        if max_vertices == 0 || max_indices == 0 {
            return Err(RendererError::InvalidConfig {
                reason: format!(
                    "a dynamic mesh needs room for geometry, got {max_vertices} vertices and \
                     {max_indices} indices"
                ),
            });
        }
        let copy_count = frames_in_flight + 1;
        let mut copies = Vec::with_capacity(copy_count);
        for _ in 0..copy_count {
            let vertex_buffer =
                GpuBuffer::new_upload(device, max_vertices * size_of::<MeshVertex>())?;
            let vertex_ptr = vertex_buffer.map_persistent()?;
            let index_buffer = GpuBuffer::new_upload(device, max_indices * size_of::<Index>())?;
            let index_ptr = index_buffer.map_persistent()?;
            copies.push(BufferPair {
                vertices: BufferSlice::whole(Arc::new(vertex_buffer)),
                vertex_ptr,
                indices: BufferSlice::whole(Arc::new(index_buffer)),
                index_ptr,
            });
        }
        let mesh = GpuMesh::empty_over(copies[0].vertices.clone(), copies[0].indices.clone());
        Ok((
            Self {
                copies,
                rotation: CopyRotation::new(copy_count),
                max_vertices,
                max_indices,
            },
            mesh,
        ))
    }

    /// Writes the geometry into the copy for `frame` and re-aims `mesh` at it.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] if the geometry exceeds the capacity given at
    /// creation. Nothing is modified in that case.
    pub(crate) fn write(
        &mut self,
        mesh: &mut GpuMesh,
        frame: u64,
        vertices: &[MeshVertex],
        indices: &[Index],
        bounds: Sphere,
    ) -> Result<(), RendererError> {
        if vertices.len() > self.max_vertices || indices.len() > self.max_indices {
            return Err(RendererError::InvalidConfig {
                reason: format!(
                    "dynamic mesh write of {} vertices and {} indices exceeds its capacity of \
                     {} and {}",
                    vertices.len(),
                    indices.len(),
                    self.max_vertices,
                    self.max_indices
                ),
            });
        }

        let index = self.rotation.next_for_write(frame);
        let copy = &self.copies[index];
        // SAFETY: each mapping covers `max_vertices` / `max_indices` elements and the lengths
        // were checked against them above, so both writes stay inside their allocations.
        // Source and destination are distinct (caller memory versus mapped GPU memory). The
        // GPU is not reading this copy: see the module docs for why the rotation guarantees
        // every frame that could have read it is complete.
        unsafe {
            std::ptr::copy_nonoverlapping(
                vertices.as_ptr(),
                copy.vertex_ptr.as_ptr().cast::<MeshVertex>(),
                vertices.len(),
            );
            std::ptr::copy_nonoverlapping(
                indices.as_ptr(),
                copy.index_ptr.as_ptr().cast::<Index>(),
                indices.len(),
            );
        }

        // An empty index list means "draw nothing", whatever the vertices say.
        let index_count = indices.len();
        mesh.repoint(
            copy.vertices.clone(),
            copy.indices.clone(),
            vertices.len() as u32,
            index_count as u32,
            bounds,
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_in_one_frame_share_a_copy() {
        let mut rotation = CopyRotation::new(4);
        let first = rotation.next_for_write(7);
        assert_eq!(rotation.next_for_write(7), first);
    }

    #[test]
    fn each_new_frame_moves_to_the_next_copy_and_wraps() {
        let mut rotation = CopyRotation::new(3);
        let picks: Vec<usize> = (0..7).map(|frame| rotation.next_for_write(frame)).collect();
        assert_eq!(picks, [0, 1, 2, 0, 1, 2, 0]);
    }

    #[test]
    fn a_gap_in_writes_never_reuses_the_live_copy() {
        let mut rotation = CopyRotation::new(4);
        let live = rotation.next_for_write(0);
        // Four frames later the frame number would land back on the same slot; the copy
        // frames 1..4 kept drawing from must not be the one overwritten.
        assert_ne!(rotation.next_for_write(4), live);
    }

    #[test]
    fn a_single_copy_rotation_does_not_panic() {
        let mut rotation = CopyRotation::new(0);
        assert_eq!(rotation.next_for_write(1), 0);
        assert_eq!(rotation.next_for_write(2), 0);
    }
}
