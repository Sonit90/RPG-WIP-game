//! GPU/CPU synchronization: one fence plus one auto-reset event.
//!
//! Two distinct waits exist and they must not be confused:
//!
//! * **Frame pacing** — before reusing frame slot `i`, wait for the fence value that was
//!   signalled the last time slot `i` was submitted. With `FRAME_COUNT` slots the CPU is
//!   only blocked when it is a whole lap ahead of the GPU.
//! * **Idle** — signal, then wait for that exact value. Required before destroying or
//!   resizing resources the GPU may still be reading, and before shutdown. This one
//!   fully serializes CPU and GPU, so it must never appear in the per-frame path.

use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Graphics::Direct3D12::{ID3D12CommandQueue, ID3D12Fence};
use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};

use crate::device::RenderDevice;
use crate::error::RendererError;

/// A fence and the event used to block on it.
pub struct GpuFence {
    fence: ID3D12Fence,
    event: WaitEvent,
    next_value: u64,
}

/// The auto-reset event a fence wait blocks on.
///
/// # Why this exists rather than a bare `HANDLE`
///
/// A `HANDLE` wraps a raw pointer and is therefore not `Send`, but a Win32 event handle is
/// a process-wide kernel object: any thread in the process may wait on it or signal it, and
/// which thread created it is not part of its contract. The renderer is moved to the thread
/// that records frames, so this states that in the type system instead of blocking the move.
///
/// # Safety
///
/// The invariant this asserts:
///
/// * the handle comes from `CreateEventW` in this process and is owned solely by the
///   [`GpuFence`] that holds it, which closes it in `Drop`;
/// * exactly one thread waits on it at a time — the one that owns the renderer. An
///   auto-reset event woken by two waiters would hand the wake to one of them, and this
///   type is deliberately **not** `Sync` so that cannot be arranged by accident.
struct WaitEvent(HANDLE);

// SAFETY: see the invariant above. A Win32 event is owned by the process, not by a thread.
unsafe impl Send for WaitEvent {}

impl GpuFence {
    /// Creates a fence starting at value 0 and its wait event.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if `CreateFence` or `CreateEventW` fails.
    pub fn new(device: &RenderDevice) -> Result<Self, RendererError> {
        let fence = device.create_fence(0)?;

        // SAFETY: FFI call creating an unnamed, auto-reset, initially unsignalled event.
        // The handle is closed in `Drop`.
        let event = unsafe { CreateEventW(None, false, false, None) }
            .map_err(|error| RendererError::api("CreateEventW", error))?;

        Ok(Self {
            fence,
            event: WaitEvent(event),
            next_value: 1,
        })
    }

    /// Signals the queue with a fresh value and returns it.
    ///
    /// The value is monotonic, so a later signal always compares greater.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if `ID3D12CommandQueue::Signal` fails.
    pub fn signal(&mut self, queue: &ID3D12CommandQueue) -> Result<u64, RendererError> {
        let value = self.next_value;
        // SAFETY: FFI call. Both COM objects are live; the fence outlives the submission
        // because it is owned by the renderer that owns the queue.
        unsafe { queue.Signal(&self.fence, value) }
            .map_err(|error| RendererError::api("ID3D12CommandQueue::Signal", error))?;
        self.next_value += 1;
        Ok(value)
    }

    /// Blocks the calling thread until the fence reaches `value`.
    ///
    /// Returns immediately if the GPU already passed it, which is the common case during
    /// frame pacing and costs one interlocked read.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if `SetEventOnCompletion` fails or the wait does
    /// not complete successfully.
    pub fn wait(&self, value: u64) -> Result<(), RendererError> {
        // SAFETY: FFI call reading the fence's current value.
        if unsafe { self.fence.GetCompletedValue() } >= value {
            return Ok(());
        }
        // SAFETY: FFI call. `self.event` is a live auto-reset event owned by this struct
        // and is only waited on by this thread.
        unsafe { self.fence.SetEventOnCompletion(value, self.event.0) }
            .map_err(|error| RendererError::api("ID3D12Fence::SetEventOnCompletion", error))?;
        // SAFETY: FFI call blocking on the event signalled above.
        let wait = unsafe { WaitForSingleObject(self.event.0, INFINITE) };
        if wait != WAIT_OBJECT_0 {
            return Err(RendererError::api(
                "WaitForSingleObject",
                windows::core::Error::from_thread(),
            ));
        }
        Ok(())
    }

    /// Signals and waits, leaving the GPU idle with respect to this queue.
    ///
    /// Fully serializing. Use before resizing or destroying GPU-visible resources and at
    /// shutdown — never per frame.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if the signal or the wait fails.
    pub fn wait_for_idle(&mut self, queue: &ID3D12CommandQueue) -> Result<(), RendererError> {
        let value = self.signal(queue)?;
        self.wait(value)
    }

    /// The value the latest [`Fence::signal`] asked the GPU to reach; zero before any.
    #[must_use]
    pub fn last_signaled_value(&self) -> u64 {
        self.next_value.saturating_sub(1)
    }

    /// Highest fence value the GPU has passed.
    #[must_use]
    pub fn completed_value(&self) -> u64 {
        // SAFETY: FFI call reading a live fence.
        unsafe { self.fence.GetCompletedValue() }
    }
}

impl Drop for GpuFence {
    fn drop(&mut self) {
        // SAFETY: the event handle was created in `new`, is owned solely by this struct
        // and is not waited on any more.
        if let Err(error) = unsafe { CloseHandle(self.event.0) } {
            log::warn!("CloseHandle(fence event) failed: {error}");
        }
    }
}
