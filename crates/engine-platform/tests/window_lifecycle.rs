//! Integration tests that drive a real `HWND`.
//!
//! The windows are created hidden so the test run does not steal focus. They still
//! require an interactive desktop session; see `docs/testing.md`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

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

#[test]
fn key_presses_reach_the_event_queue_and_auto_repeat_is_filtered() {
    use engine_platform::Key;
    use windows::Win32::UI::WindowsAndMessaging::WM_KEYDOWN;

    let config = WindowConfig::new("key test", 640, 480).with_visible(false);
    let mut window = Window::new(&config).expect("window creation must succeed");
    let hwnd = hwnd_of(&window);

    const VK_F5: usize = 0x74;
    const PREVIOUS_STATE_DOWN: isize = 1 << 30;

    // A fresh press, then two auto-repeats, then a second fresh press.
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
        SendMessageW(hwnd, WM_KEYDOWN, Some(WPARAM(VK_F5)), Some(LPARAM(0)));
    }

    let mut presses = 0;
    while let Some(event) = window.poll_event() {
        if event == WindowEvent::KeyPressed(Key::Function(5)) {
            presses += 1;
        }
    }

    assert_eq!(
        presses, 2,
        "two physical presses must produce two events, with auto-repeat filtered out"
    );
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
