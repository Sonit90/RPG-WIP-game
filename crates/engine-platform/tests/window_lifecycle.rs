//! Integration tests that drive a real `HWND`.
//!
//! The windows are created hidden so the test run does not steal focus. They still
//! require an interactive desktop session; see `docs/testing.md`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use engine_core::input::{InputEvent, InputState, KeyCode, MouseButton};
use engine_platform::window::query_client_size;
use engine_platform::{Window, WindowConfig, WindowEvent};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_CLOSE, WM_SIZE};

fn hwnd_of(window: &Window) -> HWND {
    HWND(window.raw_handle() as *mut std::ffi::c_void)
}

#[test]
fn creates_a_window_with_the_requested_client_size() {
    let config = WindowConfig::new("size test", 800, 600).with_visible(false);
    let window = Window::new(&config).expect("window creation must succeed");

    assert_eq!(window.client_size(), (800, 600));
    assert_eq!(
        query_client_size(&window).expect("GetClientRect must succeed"),
        (800, 600),
        "the OS client area must match the configured size exactly"
    );
    assert_ne!(window.raw_handle(), 0);
    assert!(!window.is_minimized());
}

#[test]
fn a_fullscreen_window_covers_the_primary_monitor() {
    // Hidden, so the test run does not black out the desktop. The size is decided at
    // creation regardless of visibility.
    let config = WindowConfig::new("fullscreen test", 640, 480)
        .with_visible(false)
        .with_fullscreen(true);
    let window = Window::new(&config).expect("window creation must succeed");

    let (width, height) = window.client_size();
    let queried = query_client_size(&window).expect("GetClientRect must succeed");
    assert_eq!(
        (width, height),
        queried,
        "the cached client size must match what the OS reports"
    );
    assert!(
        width >= 640 && height >= 480,
        "fullscreen must ignore the requested size, got {width}x{height}"
    );
    assert_eq!(
        (width, height),
        primary_monitor_size(),
        "a borderless fullscreen window is exactly the monitor size"
    );
}

/// Primary monitor size, read independently of the platform crate's own helper.
fn primary_monitor_size() -> (u32, u32) {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
    // SAFETY: FFI calls that read a global system value and cannot fail.
    unsafe {
        (
            GetSystemMetrics(SM_CXSCREEN) as u32,
            GetSystemMetrics(SM_CYSCREEN) as u32,
        )
    }
}

#[test]
fn rejects_an_invalid_configuration_before_touching_win32() {
    let config = WindowConfig::new("too small", 1, 1);
    assert!(Window::new(&config).is_err());
}

#[test]
fn close_request_reaches_the_event_queue() {
    let config = WindowConfig::new("close test", 640, 480).with_visible(false);
    let mut window = Window::new(&config).expect("window creation must succeed");
    let hwnd = hwnd_of(&window);

    // SAFETY: `hwnd` belongs to a live window owned by this test; SendMessageW invokes
    // the window procedure synchronously on this thread, which is the thread that
    // created the window.
    unsafe { SendMessageW(hwnd, WM_CLOSE, None, None) };

    let mut events = Vec::new();
    window.pump_events();
    while let Some(event) = window.poll_event() {
        events.push(event);
    }

    assert!(
        events.contains(&WindowEvent::CloseRequested),
        "expected CloseRequested, got {events:?}"
    );
    // WM_CLOSE must not destroy the window: the application decides.
    assert_ne!(window.raw_handle(), 0);
}

#[test]
fn resize_updates_cached_size_and_emits_one_event() {
    let config = WindowConfig::new("resize test", 640, 480).with_visible(false);
    let mut window = Window::new(&config).expect("window creation must succeed");
    let hwnd = hwnd_of(&window);

    const SIZE_RESTORED: usize = 0;
    for height in [500_isize, 600, 700] {
        let lparam = (height << 16) | 1024;
        // SAFETY: as above — synchronous dispatch to our own window procedure with a
        // well-formed WM_SIZE payload.
        unsafe {
            SendMessageW(
                hwnd,
                WM_SIZE,
                Some(WPARAM(SIZE_RESTORED)),
                Some(LPARAM(lparam)),
            )
        };
    }

    let mut resizes = Vec::new();
    while let Some(event) = window.poll_event() {
        if let WindowEvent::Resized { width, height } = event {
            resizes.push((width, height));
        }
    }

    assert_eq!(window.client_size(), (1024, 700));
    assert_eq!(
        resizes,
        vec![(1024, 700)],
        "consecutive resizes must coalesce into the final size"
    );
}

/// Drains the window's queue into a vector of engine input events.
fn drain_input(window: &mut Window) -> Vec<InputEvent> {
    let mut events = Vec::new();
    while let Some(event) = window.poll_event() {
        if let Some(input) = event.as_input() {
            events.push(input);
        }
    }
    events
}

#[test]
fn key_presses_reach_the_event_queue_and_auto_repeat_is_marked() {
    use windows::Win32::UI::WindowsAndMessaging::{WM_KEYDOWN, WM_KEYUP};

    let config = WindowConfig::new("key test", 640, 480).with_visible(false);
    let mut window = Window::new(&config).expect("window creation must succeed");
    let hwnd = hwnd_of(&window);

    const VK_F5: usize = 0x74;
    const PREVIOUS_STATE_DOWN: isize = 1 << 30;

    // A fresh press, two auto-repeats, a release, then a second fresh press.
    // SAFETY: `hwnd` belongs to a live window owned by this test, and SendMessageW
    // dispatches synchronously on the thread that created it.
    unsafe {
        SendMessageW(hwnd, WM_KEYDOWN, Some(WPARAM(VK_F5)), Some(LPARAM(0)));
        SendMessageW(
            hwnd,
            WM_KEYDOWN,
            Some(WPARAM(VK_F5)),
            Some(LPARAM(PREVIOUS_STATE_DOWN)),
        );
        SendMessageW(
            hwnd,
            WM_KEYDOWN,
            Some(WPARAM(VK_F5)),
            Some(LPARAM(PREVIOUS_STATE_DOWN)),
        );
        SendMessageW(hwnd, WM_KEYUP, Some(WPARAM(VK_F5)), Some(LPARAM(0)));
        SendMessageW(hwnd, WM_KEYDOWN, Some(WPARAM(VK_F5)), Some(LPARAM(0)));
    }

    let events = drain_input(&mut window);
    assert_eq!(
        events,
        vec![
            InputEvent::KeyDown {
                key: KeyCode::F5,
                repeat: false
            },
            InputEvent::KeyDown {
                key: KeyCode::F5,
                repeat: true
            },
            InputEvent::KeyDown {
                key: KeyCode::F5,
                repeat: true
            },
            InputEvent::KeyUp { key: KeyCode::F5 },
            InputEvent::KeyDown {
                key: KeyCode::F5,
                repeat: false
            },
        ],
        "auto-repeat must be marked, not dropped: a held key is still held"
    );
}

#[test]
fn a_held_key_is_reported_as_down_by_the_engine_input_state() {
    use windows::Win32::UI::WindowsAndMessaging::{WM_KEYDOWN, WM_KEYUP};

    let config = WindowConfig::new("held key test", 640, 480).with_visible(false);
    let mut window = Window::new(&config).expect("window creation must succeed");
    let hwnd = hwnd_of(&window);

    const VK_W: usize = 0x57;

    let mut state = InputState::new();

    // SAFETY: see the test above.
    unsafe { SendMessageW(hwnd, WM_KEYDOWN, Some(WPARAM(VK_W)), Some(LPARAM(0))) };
    state.begin_frame();
    for event in drain_input(&mut window) {
        state.apply(event);
    }
    assert!(state.is_key_down(KeyCode::W));
    assert!(state.was_key_pressed(KeyCode::W));

    // A frame with no messages: still held, no longer a new press.
    state.begin_frame();
    assert!(state.is_key_down(KeyCode::W));
    assert!(!state.was_key_pressed(KeyCode::W));

    // SAFETY: see the test above.
    unsafe { SendMessageW(hwnd, WM_KEYUP, Some(WPARAM(VK_W)), Some(LPARAM(0))) };
    state.begin_frame();
    for event in drain_input(&mut window) {
        state.apply(event);
    }
    assert!(!state.is_key_down(KeyCode::W));
    assert!(state.was_key_released(KeyCode::W));
}

#[test]
fn mouse_messages_translate_to_engine_input_events() {
    use windows::Win32::UI::WindowsAndMessaging::{
        WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONDOWN,
    };

    let config = WindowConfig::new("mouse test", 640, 480).with_visible(false);
    let mut window = Window::new(&config).expect("window creation must succeed");
    let hwnd = hwnd_of(&window);

    // x = 200, y = 300 packed into LPARAM; one wheel notch away from the user.
    let position = LPARAM((300_isize << 16) | 200);

    // SAFETY: see the key test above.
    unsafe {
        SendMessageW(hwnd, WM_MOUSEMOVE, Some(WPARAM(0)), Some(position));
        SendMessageW(hwnd, WM_LBUTTONDOWN, Some(WPARAM(0)), Some(position));
        SendMessageW(hwnd, WM_RBUTTONDOWN, Some(WPARAM(0)), Some(position));
        SendMessageW(hwnd, WM_LBUTTONUP, Some(WPARAM(0)), Some(position));
        SendMessageW(hwnd, WM_MOUSEWHEEL, Some(WPARAM(120 << 16)), Some(position));
    }

    let events = drain_input(&mut window);
    assert_eq!(
        events,
        vec![
            InputEvent::MouseMoved { x: 200, y: 300 },
            InputEvent::MouseButtonDown {
                button: MouseButton::Left
            },
            InputEvent::MouseButtonDown {
                button: MouseButton::Right
            },
            InputEvent::MouseButtonUp {
                button: MouseButton::Left
            },
            InputEvent::MouseWheel { delta: 1.0 },
        ]
    );

    let mut state = InputState::new();
    state.begin_frame();
    for event in events {
        state.apply(event);
    }
    assert_eq!(state.mouse_position(), (200, 300));
    assert_eq!(state.mouse_delta(), (200, 300));
    assert!(state.is_mouse_button_down(MouseButton::Right));
    assert!(state.was_mouse_button_pressed(MouseButton::Right));
    // Left went down and back up inside one frame, so it is neither held nor a visible
    // transition — the same frame-granularity limit the key tests document.
    assert!(!state.is_mouse_button_down(MouseButton::Left));
    assert!(!state.was_mouse_button_released(MouseButton::Left));
    assert!((state.wheel_delta() - 1.0).abs() < 1e-6);
}

#[test]
fn dropping_the_window_does_not_panic_and_releases_the_handle() {
    let config = WindowConfig::new("shutdown test", 320, 240).with_visible(false);
    let window = Window::new(&config).expect("window creation must succeed");
    let hwnd = hwnd_of(&window);
    drop(window);

    // SAFETY: IsWindow accepts any handle value, including a destroyed one.
    let alive = unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindow(Some(hwnd)) }.as_bool();
    assert!(!alive, "the HWND must be destroyed when Window is dropped");
}
