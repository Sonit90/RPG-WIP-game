//! Metal backend for Apple Silicon.
//!
//! The same renderer as the Direct3D 12 one in `crate::renderer`, with the same public API,
//! the same CPU half (culling, grouping, batching, light clusters, shadow planning, UI
//! batching) and a different GPU half. What differs is mostly vocabulary:
//!
//! ```text
//! D3D12                              Metal
//! command list + allocator           one MTLCommandBuffer per frame (frames.rs)
//! root signature + descriptor heap   buffer / texture argument slots (shaders/metal/BINDINGS.md)
//! upload heap ring                   one shared MTLBuffer per frame slot (upload.rs)
//! swap chain + Present               CAMetalLayer + presentDrawable (layer.rs, renderer.rs)
//! PSO                                MTLRenderPipelineState, cached per variant (pipelines.rs)
//! fence                              command-buffer completion handlers (frames.rs)
//! ```
//!
//! Apple Silicon has unified memory, so there is no staging: every buffer and texture is
//! created in shared storage and written by the CPU directly.
//!
//! # Threading
//!
//! [`Renderer::new`] attaches a `CAMetalLayer` to the window's `NSView`, which AppKit wants
//! done on the main thread; call it from there. Afterwards the renderer, like the D3D12 one,
//! is owned by one thread at a time.

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSError;
use objc2_metal::{MTLBuffer, MTLCommandBuffer, MTLDepthStencilState, MTLDevice, MTLTexture};

use crate::error::{ApiError, RendererError};

mod content;
mod device;
mod draw;
mod frames;
mod layer;
mod library;
#[cfg(feature = "debug-overlay")]
mod overlay;
mod particles;
mod pipelines;
mod readback;
mod recorder;
mod renderer;
mod resources;
mod shadow;
mod ui;
mod upload;

pub use renderer::{Renderer, RendererStartup};

/// A Metal device, as the bindings hand it out.
pub(crate) type Device = Retained<ProtocolObject<dyn MTLDevice>>;
/// A Metal buffer.
pub(crate) type Buffer = Retained<ProtocolObject<dyn MTLBuffer>>;
/// A Metal texture.
pub(crate) type Texture = Retained<ProtocolObject<dyn MTLTexture>>;
/// A Metal command buffer.
pub(crate) type CommandBuffer = Retained<ProtocolObject<dyn MTLCommandBuffer>>;
/// A Metal depth-stencil state.
pub(crate) type DepthState = Retained<ProtocolObject<dyn MTLDepthStencilState>>;

/// Wraps a Metal failure with the name of the call that produced it.
pub(crate) fn api_error(call: &'static str, message: impl Into<String>) -> RendererError {
    RendererError::api(call, ApiError::new(message))
}

/// The text of an `NSError`, for [`api_error`].
pub(crate) fn describe(error: &NSError) -> String {
    error.localizedDescription().to_string()
}

/// A value's bytes, for copying into GPU memory.
///
/// For `repr(C)` plain-data types only (the constant structs, the vertex types, `f32` and `u32`
/// arrays): the view includes any padding, which the shaders' layouts account for.
pub(crate) fn bytes_of<T: Copy>(value: &T) -> &[u8] {
    // SAFETY: `value` is a live, initialized `T` and the slice covers exactly its
    // `size_of::<T>()` bytes. `T: Copy` rules out owned resources; the callers only pass
    // `repr(C)` data without pointers.
    unsafe { std::slice::from_raw_parts(std::ptr::from_ref(value).cast::<u8>(), size_of::<T>()) }
}

/// A slice's bytes, for copying into GPU memory. See [`bytes_of`].
pub(crate) fn slice_bytes<T: Copy>(values: &[T]) -> &[u8] {
    // SAFETY: the slice is live and initialized, and the view covers exactly its
    // `size_of_val` bytes; see `bytes_of` for the restriction on `T`.
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast::<u8>(), size_of_val(values)) }
}
