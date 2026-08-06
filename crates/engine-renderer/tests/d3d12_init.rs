//! GPU integration tests: device creation, swap chain, presentation, resize.
//!
//! These need a real D3D12 adapter. On a machine without one they skip with a message
//! rather than fail, so the suite stays runnable on hardware the engine does not target.
//! The windows they create are hidden, so the run steals no focus.
//!
//! **Every test that touches the GPU takes [`gpu_lock`] first.** `cargo test` runs test
//! functions on separate threads, and presenting concurrently from several D3D12 devices
//! in one process crashes with an access violation inside the driver — reproduced with
//! as few as two threads, with the validation layer enabled and reporting nothing, and
//! independently of window visibility, message pumping and object teardown order. The
//! engine itself only ever has one device on one thread, so serializing the tests
//! restores the conditions the code is actually written for. See `docs/testing.md`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Mutex, MutexGuard, PoisonError};

use engine_platform::{Window, WindowConfig};
use engine_renderer::device::{RenderDevice, feature_level_name};
use engine_renderer::{Renderer, RendererConfig, RendererError};

/// Serializes GPU work across test threads.
static GPU: Mutex<()> = Mutex::new(());

/// Takes the process-wide GPU lock.
///
/// Poisoning is ignored on purpose: one failing test must not cascade into every later
/// test reporting a lock error instead of its own result.
fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Renderer configuration used by the tests.
///
/// The validation layer is on so misuse surfaces, but breaking is off: a test process
/// has no debugger attached, so a break would kill the run instead of reporting.
fn test_config(width: u32, height: u32) -> RendererConfig {
    RendererConfig::new(width, height)
        .with_debug_layer(true)
        .with_break_on_validation_error(false)
}

/// Creates a GPU context, or `None` if this machine has no usable D3D12 adapter.
fn gpu_context() -> Option<RenderDevice> {
    match RenderDevice::new(false, false) {
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
    let _gpu = gpu_lock();
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
    let _gpu = gpu_lock();
    if gpu_context().is_none() {
        return;
    }
    let window = hidden_window("present test", 640, 480);
    let config = test_config(640, 480);
    let mut renderer =
        Renderer::new(window.raw_handle(), &config).expect("renderer creation must succeed");

    assert_eq!(renderer.size(), (640, 480));

    const FRAMES: u64 = 10;
    for index in 0..FRAMES {
        let shade = index as f32 / FRAMES as f32;
        renderer
            .render([shade, 0.0, 1.0 - shade, 1.0])
            .expect("frame must record");
        renderer.present().expect("frame must present");
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
    let _gpu = gpu_lock();
    if gpu_context().is_none() {
        return;
    }
    let window = hidden_window("resize test", 800, 600);
    let config = test_config(800, 600);
    let mut renderer =
        Renderer::new(window.raw_handle(), &config).expect("renderer creation must succeed");

    renderer.render([0.0, 0.0, 0.0, 1.0]).unwrap();
    renderer.present().unwrap();
    renderer.resize(1024, 768).expect("resize must succeed");
    assert_eq!(renderer.size(), (1024, 768));

    for _ in 0..4 {
        renderer
            .render([0.0, 1.0, 0.0, 1.0])
            .expect("rendering must continue after a resize");
        renderer
            .present()
            .expect("present must continue after a resize");
    }
    assert_eq!(renderer.frames_presented(), 5);
}

#[test]
fn minimize_sized_resize_is_ignored() {
    let _gpu = gpu_lock();
    if gpu_context().is_none() {
        return;
    }
    let window = hidden_window("zero resize test", 640, 480);
    let config = test_config(640, 480);
    let mut renderer =
        Renderer::new(window.raw_handle(), &config).expect("renderer creation must succeed");

    renderer
        .resize(0, 0)
        .expect("a zero resize must be a no-op");
    assert_eq!(renderer.size(), (640, 480));
}

#[test]
fn present_without_a_recorded_frame_is_an_error() {
    let _gpu = gpu_lock();
    if gpu_context().is_none() {
        return;
    }
    let window = hidden_window("present order test", 640, 480);
    let config = test_config(640, 480);
    let mut renderer =
        Renderer::new(window.raw_handle(), &config).expect("renderer creation must succeed");

    assert!(
        renderer.present().is_err(),
        "presenting before recording must be rejected"
    );

    renderer.render([0.0; 4]).expect("frame must record");
    renderer.present().expect("frame must present");
    assert!(
        renderer.present().is_err(),
        "the same frame must not be presented twice"
    );
    assert_eq!(renderer.frames_presented(), 1);
}

#[test]
fn resize_drops_a_frame_recorded_against_the_old_buffers() {
    let _gpu = gpu_lock();
    if gpu_context().is_none() {
        return;
    }
    let window = hidden_window("resize drop test", 640, 480);
    let config = test_config(640, 480);
    let mut renderer =
        Renderer::new(window.raw_handle(), &config).expect("renderer creation must succeed");

    renderer.render([0.0; 4]).expect("frame must record");
    renderer.resize(800, 600).expect("resize must succeed");

    assert!(
        renderer.present().is_err(),
        "a frame recorded against destroyed buffers must not be presented"
    );
    assert_eq!(renderer.frames_presented(), 0);

    renderer.render([0.0; 4]).expect("frame must record");
    renderer.present().expect("frame must present after resize");
    assert_eq!(renderer.frames_presented(), 1);
}

/// Regression test for an access violation: `ResizeBuffers` was called while the command
/// list still referenced the old back buffers. It only crashed under concurrent GPU work,
/// so this test hammers the resize path with rendering interleaved.
#[test]
fn repeated_resizes_between_frames_stay_stable() {
    let _gpu = gpu_lock();
    if gpu_context().is_none() {
        return;
    }
    let window = hidden_window("resize stress test", 640, 480);
    let config = test_config(640, 480);
    let mut renderer =
        Renderer::new(window.raw_handle(), &config).expect("renderer creation must succeed");

    for step in 0..24_u32 {
        let width = 320 + (step % 8) * 64;
        let height = 240 + (step % 5) * 48;
        renderer
            .resize(width, height)
            .expect("resize must succeed repeatedly");
        assert_eq!(renderer.size(), (width, height));

        renderer.render([0.1, 0.2, 0.3, 1.0]).expect("record");
        renderer.present().expect("present");
    }

    assert_eq!(renderer.frames_presented(), 24);
    renderer.wait_for_idle().expect("GPU must go idle");
}

#[test]
fn drawing_the_triangle_reports_one_draw_call_and_one_triangle() {
    let _gpu = gpu_lock();
    if gpu_context().is_none() {
        return;
    }
    let window = hidden_window("triangle test", 640, 480);
    let config = test_config(640, 480);
    let mut renderer =
        Renderer::new(window.raw_handle(), &config).expect("renderer creation must succeed");

    assert_eq!(renderer.draw_calls_last_frame(), 0, "nothing drawn yet");

    for _ in 0..3 {
        renderer
            .render([0.0, 0.0, 0.0, 1.0])
            .expect("frame must record");
        renderer.present().expect("frame must present");
        assert_eq!(renderer.draw_calls_last_frame(), 1);
        assert_eq!(renderer.triangles_last_frame(), 1);
    }

    // The validation layer is enabled for these tests, so an invalid pipeline state,
    // root signature or vertex buffer binding would have produced an error by now.
    renderer.wait_for_idle().expect("GPU must go idle");
}

#[test]
fn gpu_frame_time_is_measured_with_timestamps() {
    let _gpu = gpu_lock();
    if gpu_context().is_none() {
        return;
    }
    let window = hidden_window("gpu timing test", 640, 480);
    let config = test_config(640, 480);
    let mut renderer =
        Renderer::new(window.raw_handle(), &config).expect("renderer creation must succeed");

    assert!(
        renderer.timestamp_frequency() > 0,
        "the queue must report a timestamp frequency"
    );
    assert!(
        renderer.last_gpu_frame_ms().is_none(),
        "no GPU time can exist before the first frame completes"
    );

    // A slot's timestamps are only readable when that slot is reused, which takes
    // FRAME_COUNT frames; a few more give the value time to appear.
    for _ in 0..(engine_renderer::FRAME_COUNT * 3) {
        renderer
            .render([0.0, 0.0, 0.0, 1.0])
            .expect("frame must record");
        renderer.present().expect("frame must present");
    }

    let gpu_ms = renderer
        .last_gpu_frame_ms()
        .expect("GPU time must be available after a full lap of frame slots");

    assert!(
        gpu_ms.is_finite() && gpu_ms > 0.0,
        "GPU time was {gpu_ms} ms"
    );
    assert!(
        gpu_ms < 1000.0,
        "clearing and drawing one triangle cannot take {gpu_ms} ms"
    );
    eprintln!(
        "GPU frame time: {gpu_ms:.4} ms at {} ticks/s",
        renderer.timestamp_frequency()
    );

    renderer.wait_for_idle().expect("GPU must go idle");
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
    let _gpu = gpu_lock();
    let window = hidden_window("zero size test", 640, 480);
    let error = Renderer::new(window.raw_handle(), &RendererConfig::new(0, 1080))
        .err()
        .expect("a zero dimension must be rejected");
    assert!(matches!(error, RendererError::InvalidConfig { .. }));
}
