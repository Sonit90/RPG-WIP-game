//! GPU integration tests for Milestone 2: device creation, swap chain, presentation.
//!
//! These need a real D3D12 adapter. On a machine without one they skip with a message
//! rather than fail, so the suite stays runnable on hardware the engine does not target.
//! The windows they create are hidden, so the run steals no focus.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use engine_platform::{Window, WindowConfig};
use engine_renderer::device::{GpuContext, feature_level_name};
use engine_renderer::{Renderer, RendererConfig, RendererError};

/// Creates a GPU context, or `None` if this machine has no usable D3D12 adapter.
fn gpu_context() -> Option<GpuContext> {
    match GpuContext::new(false) {
        Ok(context) => Some(context),
        Err(RendererError::NoSuitableAdapter { examined }) => {
            eprintln!("SKIP: no D3D12 adapter on this machine ({examined} examined)");
            None
        }
        Err(error) => panic!("unexpected D3D12 initialization failure: {error}"),
    }
}

/// Creates a hidden window sized `width` x `height`.
fn hidden_window(title: &str, width: u32, height: u32) -> Window {
    let config = WindowConfig::new(title, width, height).with_visible(false);
    Window::new(&config).expect("hidden window creation must succeed")
}

#[test]
fn device_initializes_and_reports_a_usable_adapter() {
    let Some(context) = gpu_context() else {
        return;
    };

    let info = context.adapter_info();
    assert!(!info.name.trim().is_empty(), "adapter must report a name");

    let level = feature_level_name(context.feature_level());
    assert_ne!(level, "unknown", "feature level must be recognised");
    assert!(
        level >= "11_0",
        "engine requires at least feature level 11_0, got {level}"
    );

    eprintln!(
        "adapter: {} ({} MiB dedicated), feature level {level}, tearing={}",
        info.name,
        info.dedicated_video_memory_mib(),
        context.allow_tearing()
    );
}

#[test]
fn renderer_presents_frames_and_shuts_down_cleanly() {
    if gpu_context().is_none() {
        return;
    }
    let window = hidden_window("present test", 640, 480);
    let config = RendererConfig::new(640, 480).with_debug_layer(false);
    let mut renderer =
        Renderer::new(window.raw_handle(), &config).expect("renderer creation must succeed");

    assert_eq!(renderer.size(), (640, 480));

    const FRAMES: u64 = 10;
    for index in 0..FRAMES {
        let shade = index as f32 / FRAMES as f32;
        renderer
            .render_clear([shade, 0.0, 1.0 - shade, 1.0])
            .expect("frame must render and present");
    }

    assert_eq!(renderer.frames_presented(), FRAMES);

    // More frames than back buffers, so the frame-pacing wait was exercised at least
    // once per slot rather than only on fresh allocators.
    assert!(FRAMES > engine_renderer::FRAME_COUNT as u64);

    renderer.wait_for_idle().expect("GPU must go idle");
    drop(renderer);
}

#[test]
fn resize_changes_the_back_buffer_size_and_keeps_rendering() {
    if gpu_context().is_none() {
        return;
    }
    let window = hidden_window("resize test", 800, 600);
    let config = RendererConfig::new(800, 600).with_debug_layer(false);
    let mut renderer =
        Renderer::new(window.raw_handle(), &config).expect("renderer creation must succeed");

    renderer.render_clear([0.0, 0.0, 0.0, 1.0]).unwrap();
    renderer.resize(1024, 768).expect("resize must succeed");
    assert_eq!(renderer.size(), (1024, 768));

    for _ in 0..4 {
        renderer
            .render_clear([0.0, 1.0, 0.0, 1.0])
            .expect("rendering must continue after a resize");
    }
    assert_eq!(renderer.frames_presented(), 5);
}

#[test]
fn minimize_sized_resize_is_ignored() {
    if gpu_context().is_none() {
        return;
    }
    let window = hidden_window("zero resize test", 640, 480);
    let config = RendererConfig::new(640, 480).with_debug_layer(false);
    let mut renderer =
        Renderer::new(window.raw_handle(), &config).expect("renderer creation must succeed");

    renderer
        .resize(0, 0)
        .expect("a zero resize must be a no-op");
    assert_eq!(renderer.size(), (640, 480));
}

#[test]
fn rejects_a_null_window_handle_without_touching_the_gpu() {
    let error = Renderer::new(0, &RendererConfig::new(640, 480))
        .err()
        .expect("a null handle must be rejected");
    assert!(matches!(error, RendererError::InvalidConfig { .. }));
}

#[test]
fn rejects_a_zero_sized_back_buffer() {
    let window = hidden_window("zero size test", 640, 480);
    let error = Renderer::new(window.raw_handle(), &RendererConfig::new(0, 1080))
        .err()
        .expect("a zero dimension must be rejected");
    assert!(matches!(error, RendererError::InvalidConfig { .. }));
}
