//! Command recording: the only place the engine talks to a D3D12 command list.
//!
//! ## What this wraps, and what it does not
//!
//! Only the operations the engine actually records. There is no attempt to mirror the
//! D3D12 command list API — a wrapper method that exists "for completeness" is code with
//! no caller, no test and no reason to be correct.
//!
//! What the wrapper adds over the raw list is the part worth centralizing:
//!
//! * **Barrier batching.** Transitions accumulate and are submitted as one array before
//!   the first command that depends on them. Each `ResourceBarrier` call has a fixed
//!   driver-side cost, so one call with four barriers beats four calls with one.
//! * **Redundant state elimination.** Setting the pipeline state, root signature or
//!   primitive topology it already has is a call into the driver that changes nothing.
//!   The context remembers what is bound and skips those.
//! * **Statistics.** Draw calls, triangles, pipeline changes and barriers are counted
//!   where they happen, rather than being estimated by the caller.
//!
//! The renderer was measured as CPU-bound in Milestone 2, so per-call overhead on this
//! path is the frame time. That is why the redundancy checks are here rather than left to
//! each caller to remember.
//!
//! ## Barriers must be flushed before they are needed
//!
//! A pending barrier that has not been submitted has not happened. Every method that
//! depends on resource state — clear, draw, close — flushes first. Callers therefore
//! cannot forget to, which is the point: forgetting produces a validation error at best
//! and a read of an untransitioned resource at worst.

use std::mem::ManuallyDrop;

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_CPU_DESCRIPTOR_HANDLE, D3D12_INDEX_BUFFER_VIEW, D3D12_RESOURCE_BARRIER,
    D3D12_RESOURCE_BARRIER_0, D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
    D3D12_RESOURCE_BARRIER_FLAG_NONE, D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
    D3D12_RESOURCE_STATES, D3D12_RESOURCE_TRANSITION_BARRIER, D3D12_VERTEX_BUFFER_VIEW,
    D3D12_VIEWPORT, ID3D12CommandAllocator, ID3D12GraphicsCommandList, ID3D12PipelineState,
    ID3D12Resource, ID3D12RootSignature,
};
use windows::core::Interface;

use crate::device::RenderDevice;
use crate::error::RendererError;
use crate::resource::TrackedResource;

/// Barriers batched before the array is submitted.
///
/// A frame transitions a handful of resources: the back buffer in and out, a depth buffer,
/// and later a few render targets. Sixteen is well above that, and the buffer is allocated
/// once so the recording path never does.
const MAX_PENDING_BARRIERS: usize = 16;

/// What one frame's recording cost, counted where it happened.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CommandStats {
    /// Draw calls issued.
    pub draw_calls: u32,
    /// Draw calls that drew more than one instance.
    pub instanced_draws: u32,
    /// Instances drawn, summed across draws.
    pub instances: u64,
    /// Triangles submitted, assuming a triangle list.
    pub triangles: u64,
    /// Pipeline state objects actually bound.
    pub pipeline_changes: u32,
    /// Pipeline binds skipped because the state was already bound.
    pub redundant_pipeline_binds: u32,
    /// Resource barriers submitted.
    pub barriers: u32,
    /// `ResourceBarrier` calls made. Fewer than `barriers` means batching worked.
    pub barrier_flushes: u32,
    /// Transitions skipped because the resource was already in the requested state.
    pub redundant_transitions: u32,
}

impl CommandStats {
    /// Mean barriers per `ResourceBarrier` call.
    ///
    /// One means no batching happened; higher is the point of batching.
    #[must_use]
    pub fn barriers_per_flush(&self) -> f64 {
        if self.barrier_flushes == 0 {
            0.0
        } else {
            f64::from(self.barriers) / f64::from(self.barrier_flushes)
        }
    }
}

/// The pipeline objects currently bound, by identity.
///
/// COM interface pointers are compared as addresses: two handles to the same object have
/// the same address, and that is exactly the question being asked — "is this already
/// bound?". Nothing is dereferenced.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct BindingState {
    root_signature: usize,
    pipeline: usize,
    /// The topology constant's value. Zero is `UNDEFINED`, which a reset list has, so the
    /// default doubles as "nothing bound".
    topology: usize,
}

impl BindingState {
    /// Records a binding, returning whether it changed anything.
    fn bind(current: &mut usize, requested: usize) -> bool {
        if *current == requested {
            false
        } else {
            *current = requested;
            true
        }
    }

    /// Forgets everything, for the start of a command list.
    ///
    /// A reset list has no state, so remembering what the previous one had bound would
    /// skip a bind that the driver needs.
    fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Records commands into one D3D12 command list.
pub struct CommandContext {
    list: ID3D12GraphicsCommandList,
    /// Transitions recorded but not yet submitted. Allocated once.
    pending: Vec<D3D12_RESOURCE_BARRIER>,
    bound: BindingState,
    stats: CommandStats,
    /// Whether the list is open for recording.
    recording: bool,
}

impl CommandContext {
    /// Creates a context around a new, closed command list.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if the list cannot be created.
    pub fn new(
        device: &RenderDevice,
        allocator: &ID3D12CommandAllocator,
    ) -> Result<Self, RendererError> {
        Ok(Self {
            list: device.create_command_list(allocator)?,
            pending: Vec::with_capacity(MAX_PENDING_BARRIERS),
            bound: BindingState::default(),
            stats: CommandStats::default(),
            recording: false,
        })
    }

    /// Opens the list for recording against `allocator`, clearing per-frame state.
    ///
    /// The caller must already have proven the GPU is done with `allocator` — that is the
    /// frame ring's job, not this one's.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if `Reset` fails.
    pub fn begin(&mut self, allocator: &ID3D12CommandAllocator) -> Result<(), RendererError> {
        // SAFETY: FFI call. The list is closed here — either from creation or from the
        // previous frame's `close` — which is what Reset requires, and the allocator's
        // work has completed, which the frame ring established before calling.
        unsafe { self.list.Reset(allocator, None) }
            .map_err(|error| RendererError::api("ID3D12GraphicsCommandList::Reset", error))?;

        self.pending.clear();
        self.bound.clear();
        self.stats = CommandStats::default();
        self.recording = true;
        Ok(())
    }

    /// Queues a transition of `resource` from `before` to `after`.
    ///
    /// Nothing is submitted until the barriers are flushed, which happens automatically
    /// before the next command that depends on resource state.
    ///
    /// A transition whose states are equal is dropped: D3D12 rejects it, and it would
    /// describe a state change that is not one.
    pub fn transition(
        &mut self,
        resource: &ID3D12Resource,
        before: D3D12_RESOURCE_STATES,
        after: D3D12_RESOURCE_STATES,
    ) {
        if before == after {
            return;
        }
        if self.pending.len() == self.pending.capacity() {
            // Full: submit what is queued rather than growing the buffer mid-frame.
            self.flush_barriers();
        }
        self.pending
            .push(transition_barrier(resource, before, after));
    }

    /// Queues a transition of a tracked resource into `after`.
    ///
    /// The "before" state comes from the resource, so it cannot be stated wrongly. A
    /// resource already in `after` produces no barrier at all — which is the other half of
    /// "avoid blindly issuing barriers every frame": the check is a comparison, not a
    /// driver call.
    ///
    /// The resource's state is updated when the barrier is *queued*, not when it is
    /// flushed. That is correct because barriers are submitted in order and before any
    /// command that depends on them, so by the time anything observes the resource it is
    /// in the recorded state.
    pub fn transition_to<R: TrackedResource + ?Sized>(
        &mut self,
        resource: &R,
        after: D3D12_RESOURCE_STATES,
    ) {
        let before = resource.state();
        if before == after {
            self.stats.redundant_transitions += 1;
            return;
        }
        self.transition(resource.resource(), before, after);
        resource.set_state(after);
    }

    /// Submits every queued barrier as one call.
    ///
    /// Called automatically before commands that depend on resource state; public because
    /// a caller that knows it is about to do unwrapped work may need to force it.
    pub fn flush_barriers(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        // SAFETY: FFI call on the open list. Every barrier refers to a resource owned by
        // the caller for longer than this recording, and the array lives until the call
        // returns.
        unsafe { self.list.ResourceBarrier(&self.pending) };

        self.stats.barriers += self.pending.len() as u32;
        self.stats.barrier_flushes += 1;
        self.pending.clear();
    }

    /// Binds a render target, flushing any pending transitions first.
    pub fn set_render_target(&mut self, rtv: D3D12_CPU_DESCRIPTOR_HANDLE) {
        self.flush_barriers();
        // SAFETY: FFI call on the open list with a handle from a heap the caller owns.
        unsafe { self.list.OMSetRenderTargets(1, Some(&rtv), false, None) };
    }

    /// Clears a render target, flushing any pending transitions first.
    ///
    /// The flush matters: clearing a resource still in `PRESENT` state is exactly the bug
    /// the automatic flush exists to prevent.
    pub fn clear_render_target(&mut self, rtv: D3D12_CPU_DESCRIPTOR_HANDLE, color: [f32; 4]) {
        self.flush_barriers();
        // SAFETY: FFI call on the open list; `color` lives for the duration of the call.
        unsafe { self.list.ClearRenderTargetView(rtv, &color, None) };
    }

    /// Sets the viewport and scissor rectangle to cover `width` x `height`.
    pub fn set_viewport(&mut self, width: u32, height: u32) {
        let viewport = viewport_for(width, height);
        let scissor = scissor_for(width, height);
        // SAFETY: FFI calls on the open list; both structures live across the calls.
        unsafe {
            self.list.RSSetViewports(&[viewport]);
            self.list.RSSetScissorRects(&[scissor]);
        }
    }

    /// Binds a root signature and pipeline state, skipping either if already bound.
    ///
    /// Redundant binds are counted rather than silently dropped, so a caller that issues
    /// them can see it in the statistics.
    pub fn set_pipeline(
        &mut self,
        root_signature: &ID3D12RootSignature,
        pipeline: &ID3D12PipelineState,
    ) {
        let root_id = root_signature.as_raw() as usize;
        let pipeline_id = pipeline.as_raw() as usize;

        let root_changed = BindingState::bind(&mut self.bound.root_signature, root_id);
        if root_changed {
            // SAFETY: FFI call on the open list with a live root signature.
            unsafe { self.list.SetGraphicsRootSignature(root_signature) };
        }

        if BindingState::bind(&mut self.bound.pipeline, pipeline_id) {
            // SAFETY: FFI call on the open list with a live pipeline state object.
            unsafe { self.list.SetPipelineState(pipeline) };
            self.stats.pipeline_changes += 1;
        } else {
            self.stats.redundant_pipeline_binds += 1;
        }
    }

    /// Sets the primitive topology, skipping it if already set.
    pub fn set_topology(&mut self, topology: D3D_PRIMITIVE_TOPOLOGY) {
        // The constant's value is only ever compared, never interpreted.
        if BindingState::bind(&mut self.bound.topology, topology.0 as usize) {
            // SAFETY: FFI call on the open list with a valid topology constant.
            unsafe { self.list.IASetPrimitiveTopology(topology) };
        }
    }

    /// Binds one vertex buffer view to `slot`.
    pub fn set_vertex_buffer(&mut self, slot: u32, view: D3D12_VERTEX_BUFFER_VIEW) {
        // SAFETY: FFI call on the open list. The view refers to a buffer the caller keeps
        // alive, and the array lives for the duration of the call.
        unsafe { self.list.IASetVertexBuffers(slot, Some(&[view])) };
    }

    /// Draws `vertex_count` vertices, `instance_count` times.
    ///
    /// Flushes pending barriers first: a draw that reads a resource still in its old state
    /// is undefined.
    pub fn draw(&mut self, vertex_count: u32, instance_count: u32) {
        self.flush_barriers();
        // SAFETY: FFI call on the open list. The bound pipeline and vertex buffers were
        // set by the caller and outlive the submission.
        unsafe {
            self.list.DrawInstanced(vertex_count, instance_count, 0, 0);
        }

        self.stats.draw_calls += 1;
        self.stats.instances += u64::from(instance_count);
        self.stats.triangles += u64::from(vertex_count / 3) * u64::from(instance_count);
        if instance_count > 1 {
            self.stats.instanced_draws += 1;
        }
    }

    /// Binds an index buffer.
    pub fn set_index_buffer(&mut self, view: D3D12_INDEX_BUFFER_VIEW) {
        // SAFETY: FFI call on the open list. The view refers to a buffer the caller keeps
        // alive, and it lives for the duration of the call.
        unsafe { self.list.IASetIndexBuffer(Some(&view)) };
    }

    /// Draws `index_count` indices, `instance_count` times.
    ///
    /// `start_instance` selects where per-instance data begins, which is what lets the
    /// benchmark issue N single-instance draws against one N-instance draw over the same
    /// buffer.
    ///
    /// Flushes pending barriers first: a draw that reads a resource still in its old state
    /// is undefined.
    pub fn draw_indexed(&mut self, index_count: u32, instance_count: u32, start_instance: u32) {
        self.flush_barriers();
        // SAFETY: FFI call on the open list. The bound pipeline, vertex buffers and index
        // buffer were set by the caller and outlive the submission.
        unsafe {
            self.list
                .DrawIndexedInstanced(index_count, instance_count, 0, 0, start_instance);
        }

        self.stats.draw_calls += 1;
        self.stats.instances += u64::from(instance_count);
        self.stats.triangles += u64::from(index_count / 3) * u64::from(instance_count);
        if instance_count > 1 {
            self.stats.instanced_draws += 1;
        }
    }

    /// Closes the list, flushing any queued barriers first.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if `Close` fails.
    pub fn close(&mut self) -> Result<(), RendererError> {
        self.flush_barriers();
        // SAFETY: FFI call closing the list recorded above.
        unsafe { self.list.Close() }
            .map_err(|error| RendererError::api("ID3D12GraphicsCommandList::Close", error))?;
        self.recording = false;
        Ok(())
    }

    /// The underlying list.
    ///
    /// For the few things not wrapped — the GPU timer records timestamps and resolves
    /// queries directly. Wrapping those would mean this module knowing about query heaps,
    /// which is not its job.
    #[must_use]
    pub fn list(&self) -> &ID3D12GraphicsCommandList {
        &self.list
    }

    /// Statistics for the frame recorded so far.
    #[must_use]
    pub fn stats(&self) -> CommandStats {
        self.stats
    }

    /// Whether the list is currently open.
    #[must_use]
    pub fn is_recording(&self) -> bool {
        self.recording
    }

    /// Barriers queued but not yet submitted.
    #[must_use]
    pub fn pending_barriers(&self) -> usize {
        self.pending.len()
    }

    /// Resets the list to an empty, closed state, releasing every resource reference it
    /// holds.
    ///
    /// A recorded list keeps references to everything it touched until it is reset. That
    /// matters before `ResizeBuffers`, which requires that no reference to a back buffer
    /// survives anywhere.
    ///
    /// The caller must have waited for the GPU to go idle.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if the reset or close fails.
    pub fn reset_to_empty(
        &mut self,
        allocator: &ID3D12CommandAllocator,
    ) -> Result<(), RendererError> {
        // SAFETY: FFI call. The GPU is idle, which is Reset's precondition.
        unsafe { allocator.Reset() }
            .map_err(|error| RendererError::api("ID3D12CommandAllocator::Reset", error))?;
        self.begin(allocator)?;
        self.close()
    }
}

/// Builds a resource transition barrier.
///
/// The `pResource` field is a non-owning copy of the interface pointer: `ManuallyDrop`
/// keeps the barrier from releasing a reference it never took, and the caller owns the
/// resource for longer than the barrier lives.
pub(crate) fn transition_barrier(
    resource: &ID3D12Resource,
    before: D3D12_RESOURCE_STATES,
    after: D3D12_RESOURCE_STATES,
) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                // SAFETY: copies the raw interface pointer without changing its refcount.
                // The copy is wrapped in ManuallyDrop, so it is never released, and the
                // real reference is owned by the caller for longer than this barrier.
                pResource: unsafe { std::mem::transmute_copy(resource) },
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: before,
                StateAfter: after,
            }),
        },
    }
}

/// Viewport covering `width` x `height`.
///
/// Pure, so the mapping from target size to viewport is testable without a GPU.
#[must_use]
pub fn viewport_for(width: u32, height: u32) -> D3D12_VIEWPORT {
    use windows::Win32::Graphics::Direct3D12::{D3D12_MAX_DEPTH, D3D12_MIN_DEPTH};
    D3D12_VIEWPORT {
        TopLeftX: 0.0,
        TopLeftY: 0.0,
        Width: width as f32,
        Height: height as f32,
        MinDepth: D3D12_MIN_DEPTH,
        MaxDepth: D3D12_MAX_DEPTH,
    }
}

/// Scissor rectangle covering `width` x `height`.
#[must_use]
pub fn scissor_for(width: u32, height: u32) -> RECT {
    RECT {
        left: 0,
        top: 0,
        right: width as i32,
        bottom: height as i32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binding_state_reports_only_real_changes() {
        let mut bound = BindingState::default();

        assert!(BindingState::bind(&mut bound.pipeline, 0x1000));
        assert!(
            !BindingState::bind(&mut bound.pipeline, 0x1000),
            "binding the same object twice must be recognized as redundant"
        );
        assert!(BindingState::bind(&mut bound.pipeline, 0x2000));
        assert!(!BindingState::bind(&mut bound.pipeline, 0x2000));
    }

    #[test]
    fn clearing_binding_state_forces_the_next_bind() {
        let mut bound = BindingState::default();
        assert!(BindingState::bind(&mut bound.pipeline, 0x1000));

        // A reset command list has no state, so a remembered binding would skip a bind the
        // driver actually needs.
        bound.clear();
        assert!(
            BindingState::bind(&mut bound.pipeline, 0x1000),
            "after a reset the same object must be bound again"
        );
    }

    #[test]
    fn each_binding_slot_is_independent() {
        let mut bound = BindingState::default();
        assert!(BindingState::bind(&mut bound.root_signature, 0x10));
        assert!(
            BindingState::bind(&mut bound.pipeline, 0x10),
            "the pipeline slot must not be confused with the root signature slot"
        );
    }

    #[test]
    fn statistics_start_at_zero() {
        let stats = CommandStats::default();
        assert_eq!(stats.draw_calls, 0);
        assert_eq!(stats.barriers_per_flush(), 0.0);
    }

    #[test]
    fn barriers_per_flush_reports_the_batching_ratio() {
        let batched = CommandStats {
            barriers: 8,
            barrier_flushes: 2,
            ..CommandStats::default()
        };
        assert!((batched.barriers_per_flush() - 4.0).abs() < 1e-12);

        let unbatched = CommandStats {
            barriers: 4,
            barrier_flushes: 4,
            ..CommandStats::default()
        };
        assert!(
            (unbatched.barriers_per_flush() - 1.0).abs() < 1e-12,
            "one barrier per call means no batching happened"
        );
    }

    #[test]
    fn viewport_and_scissor_cover_the_whole_target() {
        let viewport = viewport_for(1920, 1080);
        assert_eq!((viewport.TopLeftX, viewport.TopLeftY), (0.0, 0.0));
        assert_eq!((viewport.Width, viewport.Height), (1920.0, 1080.0));

        let scissor = scissor_for(1920, 1080);
        assert_eq!((scissor.left, scissor.top), (0, 0));
        assert_eq!((scissor.right, scissor.bottom), (1920, 1080));
    }

    #[test]
    fn viewport_and_scissor_agree_at_every_size() {
        for (width, height) in [(640, 480), (1920, 1080), (3840, 2160)] {
            let viewport = viewport_for(width, height);
            let scissor = scissor_for(width, height);
            assert_eq!(viewport.Width as i32, scissor.right);
            assert_eq!(viewport.Height as i32, scissor.bottom);
        }
    }
}
